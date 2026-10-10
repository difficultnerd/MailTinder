//! T-606 reject-undo tests (SW-05 AC1 to AC5 and `AC4a`, UN-01 AC1/AC3, INV-6,
//! ASVS V9.2.1).
//!
//! Service integration: everything runs on `FakeMailbox`, the in-memory store,
//! keys, scheduler and app folder, and the virtual clock. The `reject` swipe is
//! produced by the real `swipe` service; undo goes through `undo`, which
//! dispatches a reject to `undo_reject`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_lossless,
    clippy::assert_is_empty
)]

use std::sync::Arc;

use api::error::ApiError;
use api::routes::feed::ClassificationPayload;
use api::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use api::sealed::{SealedTokens, TokenType};
use api::services::swipe::{swipe, UndoPayload};
use api::services::undo::undo;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use domain::undo::SwipeRecord;
use domain::user_state::{HistoryAction, HistoryOutcome, UserState};
use domain::{
    derive_swipe_ids, Classification, EmailAddress, HeaderFacts, JobId, LabelSet, MailboxId,
    MessageClass, MessageId, Provider, ProviderSubjectId, SenderKey, SwipeAction, SwipeOutcome,
    UnsubscribeOptions, UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, JobRecord, KeyService, MailError, MailboxRecord, Precondition, Rng,
    ServerStore, SessionHash, SessionRecordId, UserRecord, WrappedKey,
};
use testkit::scheduler::SchedulerEvent;
use testkit::{claim_and_send, fake_ports, Fakes, MailOp, RecordingEgress, SeedMessage};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const BASE: i64 = 1_790_000_000;
const LIST_ID: &str = "news.example.com";
const ONE_CLICK: &str = "https://lists.example.com/unsub?u=1";

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn at(offset_s: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(BASE + offset_s).unwrap()
}

fn classification(class: MessageClass) -> Classification {
    Classification {
        class,
        bulk_score: 10,
        bulk_reason: "fixed".to_owned(),
        confidence: None,
        probabilities: None,
    }
}

fn label_set(ids: &[&str]) -> LabelSet {
    LabelSet::from_ids(ids.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>())
}

/// A mailing list with DKIM-covered one-click unsubscribe: a reject queues a
/// job and creates a rule.
fn one_click_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(Url::parse(ONE_CLICK).unwrap()),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some(LIST_ID.to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// A bulk look-alike with no `List-Unsubscribe`: a reject creates a rule but no
/// job (CL-01 AC2).
fn bulk_no_header_facts() -> HeaderFacts {
    HeaderFacts {
        precedence_bulk: true,
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// A personal message: a reject creates no rule and no job.
fn notice_facts() -> HeaderFacts {
    HeaderFacts {
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// A suspect message: a reject reports spam and trashes (SW-03 AC3).
fn suspect_facts() -> HeaderFacts {
    HeaderFacts {
        from_authenticated: false,
        reply_to_mismatch: true,
        ..HeaderFacts::default()
    }
}

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    primary: MailboxId,
}

async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
    sub: &str,
    email: &str,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new(sub)?;
    let address = EmailAddress::parse(email)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes.store.users().get(&user).await?.ok_or("user")?;
    let aad = Aad {
        user,
        scope: mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let sealed = fakes
        .keys
        .seal(
            &user,
            &user_record.record.wrapped_data_key,
            &aad,
            address.as_str().as_bytes(),
        )
        .await?;
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: subject,
                email_address: Ciphertext(sealed),
                status: domain::MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

impl World {
    async fn new() -> Result<World, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        let app = app_state(Arc::new(ports), Arc::new(config()?));
        let user = UserId::new(fakes.rng.uuid_v4());
        let wrapped = fakes.keys.new_user_key(&user).await?;
        fakes
            .store
            .users()
            .put(
                &UserRecord {
                    user_id: user,
                    created_at: fakes.clock.now(),
                    is_admin: false,
                    wrapped_data_key: wrapped,
                    experiments_consent_version: None,
                    experiments_opted_in_at: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        let primary = seed_mailbox(&fakes, user, "sub-a", "a@example.com").await?;
        let token = "refresh-sub-a".to_owned();
        app.tokens
            .store_refresh_token(&app, &user, &primary, Sensitive::new(token.clone()))
            .await?;
        fakes
            .identity
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(World {
            app,
            fakes,
            user,
            primary,
        })
    }

    fn session(&self, n: u128) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    fn seed(&self, sender: &str, facts: HeaderFacts, offset_s: i64) -> MessageId {
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {sender}"),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts,
                preview_text: format!("Preview at {offset_s}"),
                internal_date: at(offset_s),
                labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
            },
        )
    }

    async fn wrapped(&self) -> Result<WrappedKey, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("user")?
            .record
            .wrapped_data_key)
    }

    fn sealer(&self) -> SealedTokens {
        SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        )
    }

    async fn classification_token(
        &self,
        session: &AuthedSession,
        message_id: &str,
        class: MessageClass,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = ClassificationPayload {
            mailbox_id: self.primary.0,
            message_id: message_id.to_owned(),
            header_rules: classification(class),
            classifier_id: HEADER_RULES_ID.to_owned(),
            issued_at: self.app.ports.clock.now(),
            bakeoff: None,
        };
        Ok(self
            .sealer()
            .seal(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                self.app.ports.clock.now() + Duration::seconds(3600),
                &payload,
            )
            .await?)
    }

    /// Reject `id` under idempotency key `key`.
    async fn reject(
        &self,
        session: &AuthedSession,
        key: u128,
        id: &MessageId,
    ) -> Result<SwipeResultDto, Box<dyn std::error::Error>> {
        let req = SwipeRequest {
            mailbox_id: self.primary.0,
            message_id: id.as_str().to_owned(),
            action: ActionDto::Reject,
            category_id: None,
            new_category_name: None,
            classification_token: self
                .classification_token(session, id.as_str(), MessageClass::Notice)
                .await?,
        };
        Ok(swipe(&self.app, session, Uuid::from_u128(key), req).await?)
    }

    async fn undo(
        &self,
        session: &AuthedSession,
        token: &str,
    ) -> Result<domain::UndoResponse, ApiError> {
        undo(&self.app, session, token).await
    }

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(&self.user)
            .await?
            .state)
    }

    async fn jobs(&self) -> Result<Vec<JobRecord>, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .jobs()
            .by_mailbox(&self.primary)
            .await?
            .into_iter()
            .map(|v| v.record)
            .collect())
    }

    fn labels_of(&self, id: &MessageId) -> Option<LabelSet> {
        self.fakes.mailbox.labels_of(&self.primary, id)
    }

    fn job_id(&self, key: u128) -> JobId {
        derive_swipe_ids(&self.user, Uuid::from_u128(key)).job_id
    }
}

// ---------------------------------------------------------------------------
// SW-05 AC1: a reject is fully reversed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac1_undo_reject_restores_exact_labels() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("bulk", bulk_no_header_facts(), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("TRASH")));

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(!undone.unsubscribe_already_sent);
    // Exact previous labels, not a guessed set.
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert_eq!(state.totals.senders_silenced, 0);
    assert!(state.rules.is_empty(), "the rule this swipe made is gone");
    assert!(state.recent_swipes.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC2: before the due time the job, task and rule go
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac2_undo_before_due_cancels_job_task_and_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w.reject(&session, 1, &id).await?;
    let job = w.job_id(1);
    let due_at = result.unsubscribe_due_at.ok_or("a job was queued")?;
    assert_eq!(w.jobs().await?.len(), 1);

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(!undone.unsubscribe_already_sent);
    // The Cloud Task was deleted only after the conditional cancel won.
    assert!(
        w.fakes
            .scheduler
            .events()
            .iter()
            .any(|e| matches!(e, SchedulerEvent::Cancelled(..))),
        "the task is cancelled"
    );
    assert!(w.jobs().await?.is_empty(), "the job record is gone");
    let state = w.state().await?;
    assert!(state.rules.is_empty(), "the rule is gone");
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);

    // Advance past the due time and run the stand-in: nothing is ever sent.
    w.fakes.clock.set(due_at + Duration::seconds(60));
    let egress = RecordingEgress::new();
    assert!(!claim_and_send(&*w.fakes.store, &egress, &job).await);
    assert_eq!(egress.requests(), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC3: after the send, undo reports it already went
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac3_undo_after_send_reports_already_sent() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w.reject(&session, 1, &id).await?;
    let job = w.job_id(1);

    // The runner claims and sends first (at the job's due time).
    let egress = RecordingEgress::new();
    assert!(claim_and_send(&*w.fakes.store, &egress, &job).await);
    assert_eq!(egress.requests(), 1);

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(
        undone.unsubscribe_already_sent,
        "the client is told it already went"
    );
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    let state = w.state().await?;
    assert!(state.rules.is_empty(), "the rule is still removed");
    Ok(())
}

#[tokio::test]
async fn sw_05_ac3_undo_after_feed_collected_job_reports_sent() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    let job = w.job_id(1);
    // A Feed load collected the job after it ran: the record is gone.
    w.fakes
        .store
        .jobs()
        .delete(&job, Precondition::None)
        .await?;

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.unsubscribe_already_sent);
    assert!(w.jobs().await?.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// OBS-EV AC3: a send that follows a successful undo is an unrecoverable action
// ---------------------------------------------------------------------------

/// The `(event_type, outcome)` pairs of every `metric` line captured in `sink`.
fn metrics(sink: &obs::CaptureSink) -> Vec<(String, String)> {
    sink.lines()
        .iter()
        .filter_map(|line| {
            let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
            if parsed.get("event")?.as_str()? != "metric" {
                return None;
            }
            Some((
                parsed.get("action")?.as_str()?.to_owned(),
                parsed.get("outcome")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// OBS-EV AC3 (api): an undo of a reject whose unsubscribe had already left the
/// queue (the runner claimed or sent it first) emits exactly one
/// `unsub_after_undo`; an undo that wins the cancel, sending nothing, emits
/// none (S10 8, T-1114).
#[tokio::test]
async fn obs_ev_ac3_unsub_after_undo_emitted_when_a_send_follows() -> TestResult {
    // The runner claims and sends first; the undo then only finds `Sent`.
    {
        let w = World::new().await?;
        let session = w.session(1);
        let id = w.seed("news", one_click_facts(), 10);
        let result = w.reject(&session, 1, &id).await?;
        let job = w.job_id(1);
        let egress = RecordingEgress::new();
        assert!(claim_and_send(&*w.fakes.store, &egress, &job).await);
        let (capture, _guard) =
            obs::capture("api", obs::arc(obs::FixedClock(w.app.ports.clock.now())));

        let undone = w.undo(&session, &result.undo_token).await?;
        assert!(undone.restored);
        assert!(undone.unsubscribe_already_sent);

        let events = metrics(&capture);
        let after: Vec<_> = events
            .iter()
            .filter(|(event, _)| event == "unsub_after_undo")
            .collect();
        assert_eq!(after.len(), 1, "exactly one unsub_after_undo: {events:?}");
        assert_eq!(after[0].1, "sent", "the event carries the sent outcome");
    }

    // The undo wins the cancel: the job is cancelled and nothing is sent, so the
    // benign race is not an unrecoverable action.
    {
        let w = World::new().await?;
        let session = w.session(1);
        let id = w.seed("news", one_click_facts(), 10);
        let result = w.reject(&session, 1, &id).await?;
        let (capture, _guard) =
            obs::capture("api", obs::arc(obs::FixedClock(w.app.ports.clock.now())));

        let undone = w.undo(&session, &result.undo_token).await?;
        assert!(undone.restored);
        assert!(!undone.unsubscribe_already_sent);

        let events = metrics(&capture);
        assert!(
            events.iter().all(|(event, _)| event != "unsub_after_undo"),
            "a cancelled job sends nothing: {events:?}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC4: one undo per call, walking back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac4_undo_walks_back_two_rejects() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let first = w.seed("bulk", bulk_no_header_facts(), 10);
    let second = w.seed("news", one_click_facts(), 9);
    let before_first = w.labels_of(&first).ok_or("first")?;
    let before_second = w.labels_of(&second).ok_or("second")?;
    let r1 = w.reject(&session, 1, &first).await?;
    let r2 = w.reject(&session, 2, &second).await?;

    assert!(w.undo(&session, &r2.undo_token).await?.restored);
    assert!(w.undo(&session, &r1.undo_token).await?.restored);

    assert_eq!(w.labels_of(&first).ok_or("first")?, before_first);
    assert_eq!(w.labels_of(&second).ok_or("second")?, before_second);
    let state = w.state().await?;
    assert!(state.rules.is_empty());
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert!(state.recent_swipes.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC4a: a spam report is reversed, the report itself is not recalled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac4a_undo_spam_restores_location() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("suspect", suspect_facts(), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::ReportedSpam);
    let after = w.labels_of(&id).ok_or("still present")?;
    assert!(!after.contains("INBOX"), "trashed");
    assert!(after.contains("SPAM"), "reported spam");

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(!undone.unsubscribe_already_sent);
    // The spam label is gone and the previous labels are exact.
    let restored = w.labels_of(&id).ok_or("still present")?;
    assert!(!restored.contains("SPAM"));
    assert_eq!(restored, before);
    let state = w.state().await?;
    assert!(
        state
            .history
            .iter()
            .all(|h| h.action != HistoryAction::ReportedSpam),
        "the reported_spam entry this swipe wrote is removed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC5: a retry after a restore failure gives the same answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac5_undo_retry_after_restore_failure_same_answer() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;

    // The provider refuses the first restore: `502` and the token stays valid.
    w.fakes
        .mailbox
        .fail_next(MailOp::RestoreLabels, MailError::Forbidden);
    let Err(err) = w.undo(&session, &result.undo_token).await else {
        panic!("the first undo must fail");
    };
    assert_eq!(
        err,
        ApiError::ProviderError {
            mailbox_id: Some(w.primary.0)
        }
    );
    // The cancelled job is still there, so the retry maps back to `Cancelled`.
    assert_eq!(w.jobs().await?.len(), 1);

    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(
        !undone.unsubscribe_already_sent,
        "the same answer, nothing sent"
    );
    assert!(w.jobs().await?.is_empty(), "the job is deleted");
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC1: a duplicate delivery after a cancel sends nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac1_duplicate_delivery_after_cancel_sends_nothing() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    let job = w.job_id(1);

    assert!(w.undo(&session, &result.undo_token).await?.restored);

    // Two late deliveries of the same task: both are no-ops.
    let egress = RecordingEgress::new();
    w.fakes
        .clock
        .set(result.unsubscribe_due_at.ok_or("queued")?);
    assert!(!claim_and_send(&*w.fakes.store, &egress, &job).await);
    assert!(!claim_and_send(&*w.fakes.store, &egress, &job).await);
    assert_eq!(egress.requests(), 0, "a cancelled job is never sent");
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC3: a cancelled unsubscribe is recorded in History once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac3_cancel_written_to_history_once() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;

    assert!(w.undo(&session, &result.undo_token).await?.restored);
    // The second attempt is refused: one History entry, not two.
    let Err(err) = w.undo(&session, &result.undo_token).await else {
        panic!("the replay must be refused");
    };
    assert_eq!(err, ApiError::UndoExpired);

    let state = w.state().await?;
    let cancels: Vec<_> = state
        .history
        .iter()
        .filter(|h| h.action == HistoryAction::Unsubscribe)
        .collect();
    assert_eq!(cancels.len(), 1, "one cancelled unsubscribe");
    assert_eq!(cancels[0].outcome, HistoryOutcome::Cancelled);
    assert!(state.pending_unsubscribes.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V9.2.1: a token from an earlier session is gone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v9_2_1_undo_token_from_earlier_session_gone() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let other = w.session(2);
    let id = w.seed("bulk", bulk_no_header_facts(), 10);
    // Seal a reject undo payload for a DIFFERENT session.
    let payload = UndoPayload {
        record: SwipeRecord {
            mailbox: w.primary,
            message: id.clone(),
            sender: SenderKey::from_address("bulk@example.com"),
            action: SwipeAction::Reject,
            outcome: SwipeOutcome::TrashedListNoUnsubscribe,
            previous_labels: label_set(&["INBOX", "UNREAD"]),
            job_id: None,
            rule_id: None,
            counted_reject_at: None,
            at: w.fakes.clock.now(),
        },
        swipe_id: Uuid::from_u128(9),
        history_entry: None,
        na_item: None,
        skip_key: None,
    };
    let token = w
        .sealer()
        .seal(
            TokenType::Undo,
            &w.user,
            &w.wrapped().await?,
            &other.session_record_id,
            w.fakes.clock.now() + Duration::hours(1),
            &payload,
        )
        .await?;
    let Err(err) = w.undo(&session, &token).await else {
        panic!("a token for another session must not open");
    };
    assert_eq!(err, ApiError::UndoExpired);
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-6: any mix of reject classes, then undo of each
// ---------------------------------------------------------------------------

/// The four reject classes a reject can be.
#[derive(Clone, Copy, Debug)]
enum RejectClass {
    Notice,
    BulkNoHeader,
    OneClick,
    Suspect,
}

impl RejectClass {
    fn facts(self) -> HeaderFacts {
        match self {
            RejectClass::Notice => notice_facts(),
            RejectClass::BulkNoHeader => bulk_no_header_facts(),
            RejectClass::OneClick => one_click_facts(),
            RejectClass::Suspect => suspect_facts(),
        }
    }
}

const CLASSES: [RejectClass; 4] = [
    RejectClass::Notice,
    RejectClass::BulkNoHeader,
    RejectClass::OneClick,
    RejectClass::Suspect,
];

#[tokio::test]
async fn inv_6_every_reject_undoable() -> TestResult {
    for len in 1..=3_u32 {
        for n in 0..4_u32.pow(len) {
            let mut seq = Vec::new();
            let mut x = n;
            for _ in 0..len {
                seq.push(CLASSES[(x % 4) as usize]);
                x /= 4;
            }
            run_reject_sequence(&seq).await?;
        }
    }
    Ok(())
}

/// A mix of rejects on distinct messages, then one undo each, newest first:
/// labels, rules and counts come back to where they started.
async fn run_reject_sequence(seq: &[RejectClass]) -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let mut before = Vec::new();
    let mut tokens = Vec::new();
    for (i, class) in seq.iter().enumerate() {
        let sender = format!("s{i}");
        let id = w.seed(&sender, class.facts(), 10 - i as i64);
        before.push((id.clone(), w.labels_of(&id).ok_or("seeded")?));
        let result = w
            .reject(&session, 1000 + i as u128, &id)
            .await
            .map_err(|e| format!("reject {i}: {e:?}"))?;
        tokens.push(result.undo_token);
    }
    for token in tokens.iter().rev() {
        assert!(w.undo(&session, token).await?.restored);
    }
    for (id, labels) in &before {
        assert_eq!(w.labels_of(id).ok_or("still present")?, *labels);
    }
    let state = w.state().await?;
    assert!(state.rules.is_empty(), "no rule is left");
    assert!(state.recent_swipes.is_empty());
    assert!(state.pending_unsubscribes.is_empty());
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert_eq!(state.totals.senders_silenced, 0);
    assert_eq!(state.totals.unsubscribes_queued, 0);
    assert_eq!(state.totals.round_unsubscribes, 0);
    assert!(w.jobs().await?.is_empty(), "every queued job is gone");
    for i in 0..seq.len() {
        let sender = format!("s{i}@example.com");
        if let Some(stats) = state.sender_stats.get(&sender) {
            assert_eq!(stats.keeps, 0, "{sender} keeps");
            assert!(stats.rejects_counted.is_empty(), "{sender} rejects");
            assert!(stats.files.is_empty(), "{sender} files");
            assert!(!stats.boss_defeated, "{sender} boss");
        }
    }
    Ok(())
}
