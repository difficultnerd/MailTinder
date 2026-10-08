//! T-605 reject swipe tests (SW-03, SR-01, PB-01, UN-01, UN-02, UN-04, CL-01,
//! GM-05, GM-08, INV-2, JOB-1, SW-05 AC5, ASVS V2.3.2, V2.3.4).
//!
//! Service integration: everything runs on `FakeMailbox`, the in-memory store,
//! keys, scheduler and app folder, and the virtual clock. The handler only adds
//! the `Idempotency-Key` header and the swipe rate limit, so the service is
//! called directly.

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
use api::http::request_id::RequestId;
use api::limits::{policies, LimitSubject};
use api::routes::feed::ClassificationPayload;
use api::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use api::sealed::{SealedTokens, TokenType};
use api::services::reject::{BlockPromptRef, NS_NA_ITEM};
use api::services::swipe::swipe;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use async_trait::async_trait;
use domain::user_state::UserState;
use domain::{
    derive_swipe_ids, Classification, EmailAddress, HeaderFacts, JobId, JobMethod, JobStatus,
    LabelSet, MailboxId, MessageClass, MessageId, NeedsAttentionReason, Provider,
    ProviderSubjectId, RuleKind, SwipeOutcome, UnsubscribeOptions, UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, CancelOutcome, Ciphertext, Clock, JobRecord, JobScheduler, KeyService, ListKeyHash,
    MailError, MailboxCtx, MailboxRecord, NeedsAttentionId, Precondition, Rng, SchedError,
    ServerStore, SessionHash, SessionRecordId, TaskName, UserRecord, Versioned, WrappedKey,
};
use proptest::prelude::*;
use testkit::app_folder::FolderOp;
use testkit::scheduler::SchedulerEvent;
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
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

fn err_of<T>(result: Result<T, ApiError>) -> ApiError {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

// ---------------------------------------------------------------------------
// Header facts for the corpus cases
// ---------------------------------------------------------------------------

/// A mailing list with DKIM-covered one-click unsubscribe, authenticated.
fn one_click_facts(list_id: Option<&str>) -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(Url::parse(ONE_CLICK).unwrap()),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: list_id.map(str::to_owned),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// An https-only (or http-only) page link: needs the user to open it.
fn page_link_facts(link: &str) -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: None,
            https: Some(Url::parse(link).unwrap()),
            mailto: None,
        }),
        list_unsubscribe_present: true,
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// Corpus "bulk look-alike with body unsubscribe link": bulk markers, no
/// `List-Unsubscribe` header at all.
fn bulk_no_header_facts() -> HeaderFacts {
    HeaderFacts {
        precedence_bulk: true,
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// Corpus "one-click headers not in DKIM h=": the header exists but no option
/// survives the DKIM cover check, so `list_unsubscribe` is `None`.
fn uncovered_one_click_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        list_id: Some(LIST_ID.to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

fn personal_facts(authenticated: bool) -> HeaderFacts {
    HeaderFacts {
        from_authenticated: authenticated,
        ..HeaderFacts::default()
    }
}

fn suspect_facts() -> HeaderFacts {
    HeaderFacts {
        from_authenticated: false,
        reply_to_mismatch: true,
        ..HeaderFacts::default()
    }
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

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

/// A scheduler that always fails, for the schedule-failure rollback.
struct FailingScheduler;

#[async_trait]
impl JobScheduler for FailingScheduler {
    async fn schedule(
        &self,
        _job: &JobId,
        _due_at: OffsetDateTime,
    ) -> Result<TaskName, SchedError> {
        Err(SchedError::Unavailable)
    }

    async fn cancel(&self, _task: &TaskName) -> Result<CancelOutcome, SchedError> {
        Ok(CancelOutcome::NotFound)
    }
}

impl World {
    async fn new() -> Result<World, Box<dyn std::error::Error>> {
        Self::build(false).await
    }

    async fn with_failing_scheduler() -> Result<World, Box<dyn std::error::Error>> {
        Self::build(true).await
    }

    async fn build(failing_scheduler: bool) -> Result<World, Box<dyn std::error::Error>> {
        let (mut parts, fakes) = fake_ports();
        if failing_scheduler {
            parts.scheduler = Arc::new(FailingScheduler);
        }
        let app = app_state(Arc::new(parts), Arc::new(config()?));
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

    /// Seed an inbox message from `address` with `facts`.
    fn seed_from(
        &self,
        address: &str,
        facts: HeaderFacts,
        offset_s: i64,
        labels: &[&str],
    ) -> MessageId {
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {address}"),
                from_address: address.to_owned(),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts,
                preview_text: format!("Preview at {offset_s}"),
                internal_date: at(offset_s),
                labels: labels.iter().map(|s| (*s).to_owned()).collect(),
            },
        )
    }

    fn seed(&self, sender: &str, facts: HeaderFacts, offset_s: i64) -> MessageId {
        self.seed_from(
            &format!("{sender}@example.com"),
            facts,
            offset_s,
            &["INBOX", "UNREAD"],
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

    async fn ctx(&self) -> Result<MailboxCtx, Box<dyn std::error::Error>> {
        Ok(self
            .app
            .tokens
            .mailbox_ctx(&self.app, &self.user, &self.primary)
            .await?)
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

    async fn reject_request(
        &self,
        session: &AuthedSession,
        id: &MessageId,
        token_class: MessageClass,
    ) -> Result<SwipeRequest, Box<dyn std::error::Error>> {
        Ok(SwipeRequest {
            mailbox_id: self.primary.0,
            message_id: id.as_str().to_owned(),
            action: ActionDto::Reject,
            category_id: None,
            new_category_name: None,
            classification_token: self
                .classification_token(session, id.as_str(), token_class)
                .await?,
        })
    }

    /// Reject `id` under idempotency key `key`.
    async fn reject(
        &self,
        session: &AuthedSession,
        key: u128,
        id: &MessageId,
    ) -> Result<SwipeResultDto, Box<dyn std::error::Error>> {
        let req = self
            .reject_request(session, id, MessageClass::Notice)
            .await?;
        Ok(swipe(&self.app, session, Uuid::from_u128(key), req).await?)
    }

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(&self.user)
            .await?
            .state)
    }

    async fn jobs(&self) -> Result<Vec<Versioned<JobRecord>>, Box<dyn std::error::Error>> {
        Ok(self.fakes.store.jobs().by_mailbox(&self.primary).await?)
    }

    fn labels_of(&self, id: &MessageId) -> Option<LabelSet> {
        self.fakes.mailbox.labels_of(&self.primary, id)
    }

    fn job_id(&self, key: u128) -> JobId {
        derive_swipe_ids(&self.user, Uuid::from_u128(key)).job_id
    }

    fn scheduled_count(&self) -> usize {
        self.fakes
            .scheduler
            .events()
            .iter()
            .filter(|e| matches!(e, SchedulerEvent::Scheduled(..)))
            .count()
    }
}

// ---------------------------------------------------------------------------
// SW-03 reject outcomes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_03_ac1_reject_trashes_message() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("alice", personal_facts(false), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::Trashed);
    // Moved to trash, not deleted: the message still exists with TRASH.
    let labels = w.labels_of(&id).ok_or("message must still exist")?;
    assert!(labels.contains("TRASH"));
    assert!(!labels.contains("INBOX"));
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 1);
    assert_eq!(w.fakes.mailbox.calls(MailOp::ReportSpam), 0);
    assert!(result.undo_token.starts_with("mt1.undo."));
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 1);
    assert_eq!(state.totals.cleared, 1);
    Ok(())
}

#[tokio::test]
async fn sw_03_ac2_reject_queues_unsubscribe_with_delay() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let now = w.fakes.clock.now();
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    let due = now + Duration::minutes(5);
    assert_eq!(result.unsubscribe_due_at, Some(due));

    // The job record, due now plus five minutes, queued.
    let job_id = w.job_id(1);
    let job = w
        .fakes
        .store
        .jobs()
        .get(&job_id)
        .await?
        .ok_or("job stored")?
        .record;
    assert_eq!(job.due_at, due);
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.method, JobMethod::OneClick);
    assert_eq!(job.attempts, 0);

    // The task is named after the job ID and due at the same time.
    assert!(w
        .fakes
        .scheduler
        .events()
        .contains(&SchedulerEvent::Scheduled(TaskName::for_job(&job_id), due)));

    // The reject_list rule, and the pending unsubscribe in the user's file.
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert_eq!(state.rules[0].rule.kind, RuleKind::RejectList);
    assert!(state.pending_unsubscribes.contains_key(&job_id.0));
    assert_eq!(state.totals.unsubscribes_queued, 1);
    assert_eq!(state.totals.senders_silenced, 1);
    Ok(())
}

#[tokio::test]
async fn sw_03_ac3_suspect_reported_and_trashed_no_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("phish", suspect_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::ReportedSpam);
    assert_eq!(w.fakes.mailbox.calls(MailOp::ReportSpam), 1);
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 1);
    let labels = w.labels_of(&id).ok_or("still present")?;
    assert!(labels.contains("TRASH"));
    assert!(!labels.contains("INBOX"));
    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    let state = w.state().await?;
    assert!(state.rules.is_empty(), "no rule for a suspect message");
    assert_eq!(state.history.len(), 1);
    assert_eq!(
        state.history[0].action,
        domain::user_state::HistoryAction::ReportedSpam
    );
    assert_eq!(
        state.history[0].entry_id,
        api::services::swipe::swipe_id(&w.user, Uuid::from_u128(1))
    );
    Ok(())
}

#[tokio::test]
async fn sw_03_ac4_personal_trashed_and_counted() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("alice", personal_facts(true), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::Trashed);
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("TRASH")));
    assert!(w.jobs().await?.is_empty());
    let state = w.state().await?;
    assert!(
        state.rules.is_empty(),
        "a person is never given a list rule"
    );
    let stats = state
        .sender_stats
        .get("alice@example.com")
        .ok_or("counted")?;
    assert_eq!(stats.rejects_counted.len(), 1);
    assert!(
        result.prompts.is_empty(),
        "one reject is below the threshold"
    );
    Ok(())
}

#[tokio::test]
async fn sw_03_ac5_no_header_rule_no_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("promo", bulk_no_header_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert_eq!(result.unsubscribe_due_at, None);
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("TRASH")));
    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert_eq!(state.rules[0].rule.kind, RuleKind::RejectList);
    assert!(state.pending_unsubscribes.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SR-01 rules
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sr_01_ac1_rule_keyed_on_list_id() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let with_list = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let without_list = w.seed("alerts", one_click_facts(None), 9);
    w.reject(&session, 1, &with_list).await?;
    w.reject(&session, 2, &without_list).await?;
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 2);
    let by_list = state
        .rules
        .iter()
        .find(|r| r.rule.matcher.sender.as_str() == "news@example.com")
        .ok_or("rule for the list")?;
    assert_eq!(by_list.rule.matcher.list_id.as_deref(), Some(LIST_ID));
    let by_sender = state
        .rules
        .iter()
        .find(|r| r.rule.matcher.sender.as_str() == "alerts@example.com")
        .ok_or("rule for the sender")?;
    assert_eq!(by_sender.rule.matcher.list_id, None);
    Ok(())
}

#[tokio::test]
async fn sr_01_ac1a_relay_rule_uses_original_sender() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let relay = domain::RELAY_DOMAINS[0];
    let address = format!("promo_at_shop_example_com_abcd@{relay}");
    let id = w.seed_from(&address, one_click_facts(Some(LIST_ID)), 10, &["INBOX"]);
    w.reject(&session, 1, &id).await?;
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert_eq!(
        state.rules[0].rule.matcher.sender.as_str(),
        "promo@shop.example.com"
    );
    Ok(())
}

#[tokio::test]
async fn sr_01_ac6_spoofed_sender_no_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let mut facts = one_click_facts(Some(LIST_ID));
    facts.from_authenticated = false;
    let id = w.seed("spoof", facts, 10);
    w.reject(&session, 1, &id).await?;
    // Trashed, but no rule and nothing counted.
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("TRASH")));
    let state = w.state().await?;
    assert!(state.rules.is_empty());
    assert_eq!(state.totals.senders_silenced, 0);
    let counted = state
        .sender_stats
        .get("spoof@example.com")
        .map_or(0, |s| s.rejects_counted.len());
    assert_eq!(counted, 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// PB-01 block prompt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pb_01_ac1_block_prompt_after_threshold() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let threshold = domain::Tunables::default().personal_block_threshold;
    let mut last = None;
    for n in 0..threshold {
        let id = w.seed("alice", personal_facts(true), 10 + i64::from(n));
        let result = w.reject(&session, 100 + u128::from(n), &id).await?;
        if n + 1 < threshold {
            assert!(result.prompts.is_empty(), "no prompt before the threshold");
        }
        last = Some(result);
    }
    let result = last.ok_or("at least one swipe")?;
    assert_eq!(result.prompts.len(), 1);
    let prompt = &result.prompts[0];
    assert_eq!(prompt.sender_name, "Name of alice@example.com");
    let opened: BlockPromptRef = w
        .sealer()
        .open(
            TokenType::PromptRef,
            &w.user,
            &w.wrapped().await?,
            &session.session_record_id,
            &prompt.prompt_ref,
        )
        .await?;
    assert_eq!(opened.sender_key, "alice@example.com");
    assert_eq!(opened.mailbox_id, w.primary.0);
    assert_eq!(
        opened.swipe_id,
        api::services::swipe::swipe_id(&w.user, Uuid::from_u128(100 + u128::from(threshold - 1)))
    );
    Ok(())
}

#[tokio::test]
async fn pb_01_ac4_unauthenticated_rejects_never_prompt() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    for n in 0..3_u32 {
        let id = w.seed("spoofer", personal_facts(false), 10 + i64::from(n));
        let result = w.reject(&session, 200 + u128::from(n), &id).await?;
        assert!(result.prompts.is_empty());
    }
    let state = w.state().await?;
    let counted = state
        .sender_stats
        .get("spoofer@example.com")
        .map_or(0, |s| s.rejects_counted.len());
    assert_eq!(counted, 0, "unauthenticated rejects never count");
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 / JOB-1 / INV-2: the job document
// ---------------------------------------------------------------------------

/// The stored job after one one-click reject, as the stored document JSON.
async fn stored_job(
    w: &World,
    key: u128,
) -> Result<(JobRecord, String), Box<dyn std::error::Error>> {
    let job = w
        .fakes
        .store
        .jobs()
        .get(&w.job_id(key))
        .await?
        .ok_or("job stored")?
        .record;
    let json = serde_json::to_string(&job)?;
    Ok((job, json))
}

#[tokio::test]
async fn un_01_ac4_job_holds_no_access_token() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    w.reject(&session, 1, &id).await?;
    let (job, json) = stored_job(&w, 1).await?;
    assert!(!json.contains("access-for-"), "no access token");
    assert!(!json.contains("refresh-"), "no refresh token");
    assert!(!json.contains("example.com"), "no address or URL in clear");
    // The target is sealed under the job-target AAD and opens to the header URL.
    let aad = Aad {
        user: w.user,
        scope: job.job_id.0.to_string(),
        field: aad_fields::JOB_TARGET,
    };
    let sealed = job.target.ok_or("target present while queued")?;
    let plain = w
        .fakes
        .keys
        .open(&w.user, &w.wrapped().await?, &aad, &sealed.0)
        .await?;
    assert_eq!(String::from_utf8(plain)?, format!("one_click\n{ONE_CLICK}"));
    Ok(())
}

#[tokio::test]
async fn job_1_no_access_token_on_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    w.reject(&session, 1, &id).await?;
    // Every job document in the store, whatever its shape.
    let jobs = w.jobs().await?;
    assert_eq!(jobs.len(), 1);
    for job in jobs {
        let json = serde_json::to_string(&job.record)?;
        let access = w.ctx().await?;
        assert!(!json.contains(access.access_token.expose()));
        assert!(!json.contains("access"));
        assert!(!json.contains("token"));
    }
    Ok(())
}

#[tokio::test]
async fn inv_2_job_has_expiry() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    w.reject(&session, 1, &id).await?;
    let (job, _) = stored_job(&w, 1).await?;
    assert_eq!(job.expires_at, job.due_at + Duration::minutes(60));
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-02 / CL-01: no job without DKIM cover
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_02_ac2_no_dkim_cover_no_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", uncovered_one_click_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("TRASH")));
    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    Ok(())
}

#[tokio::test]
async fn cl_01_ac2_token_claiming_list_cannot_create_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("promo", bulk_no_header_facts(), 10);

    // A forged-content token: a valid token with one body character changed
    // does not open.
    let valid = w
        .classification_token(&session, id.as_str(), MessageClass::List)
        .await?;
    let mut forged = valid;
    let last = forged.pop().ok_or("non-empty token")?;
    forged.push(if last == 'A' { 'B' } else { 'A' });
    let opened = w
        .sealer()
        .open::<ClassificationPayload>(
            TokenType::Classification,
            &w.user,
            &w.wrapped().await?,
            &session.session_record_id,
            &forged,
        )
        .await;
    assert!(opened.is_err(), "a forged token is refused at open");

    // A valid token that claims `list` for a `bulk_no_header` message: the
    // action class comes from the fresh re-read, so still no job.
    let req = w.reject_request(&session, &id, MessageClass::List).await?;
    let result = swipe(&w.app, &session, Uuid::from_u128(1), req).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-04 AC6: https-only
// ---------------------------------------------------------------------------

fn na_item_id(user: &UserId, sender: &str) -> NeedsAttentionId {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(user.0.as_bytes());
    bytes.extend_from_slice(sender.as_bytes());
    NeedsAttentionId(Uuid::new_v5(&NS_NA_ITEM, &bytes))
}

#[tokio::test]
async fn un_04_ac6_https_only_raises_item_with_header_link() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let link = "https://example.com/unsub?id=1";
    let https = w.seed("shop", page_link_facts(link), 10);
    let http = w.seed("plain", page_link_facts("http://example.com/unsub?id=2"), 9);
    let result = w.reject(&session, 1, &https).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    w.reject(&session, 2, &http).await?;

    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    assert_eq!(w.state().await?.rules.len(), 2);

    // The https item holds the header link, sealed.
    let item_id = na_item_id(&w.user, "shop@example.com");
    let item = w
        .fakes
        .store
        .needs_attention()
        .get(&item_id)
        .await?
        .ok_or("item raised")?
        .record;
    assert_eq!(item.reason_code, NeedsAttentionReason::HttpsOnlyUnsubscribe);
    assert_eq!(item.expires_at, item.created_at + Duration::days(30));
    let aad = Aad {
        user: w.user,
        scope: item_id.0.to_string(),
        field: aad_fields::NA_LINK,
    };
    let sealed = item.link.ok_or("link kept for https")?;
    let plain = w
        .fakes
        .keys
        .open(&w.user, &w.wrapped().await?, &aad, &sealed.0)
        .await?;
    assert_eq!(String::from_utf8(plain)?, link);

    // The http link is dropped: the item has no link.
    let dropped = w
        .fakes
        .store
        .needs_attention()
        .get(&na_item_id(&w.user, "plain@example.com"))
        .await?
        .ok_or("item raised")?
        .record;
    assert!(dropped.link.is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// GM-05, GM-08
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gm_05_ac1_rule_stores_yearly_rate() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let rejected = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    // Four more from the same list in the window; one from another list.
    for n in 1..=4 {
        w.seed("news", one_click_facts(Some(LIST_ID)), 10 + n);
    }
    w.seed("news", one_click_facts(Some("other.example.com")), 20);
    w.reject(&session, 1, &rejected).await?;
    assert_eq!(w.fakes.mailbox.calls(MailOp::CountMessages), 1, "one count");
    let state = w.state().await?;
    let expected = domain::yearly_rate(Some(5), Duration::days(90));
    assert!(expected.is_some());
    assert_eq!(state.rules[0].yearly_rate, expected);
    assert_eq!(state.rules[0].times_applied, 0);
    Ok(())
}

#[tokio::test]
async fn gm_05_ac3_count_failure_rule_still_created() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    w.fakes
        .mailbox
        .fail_next(MailOp::CountMessages, MailError::Transient);
    let result = w.reject(&session, 1, &id).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert_eq!(state.rules[0].yearly_rate, None);
    Ok(())
}

#[tokio::test]
async fn gm_08_ac3_reject_defeats_boss() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let tunables = domain::Tunables::default();
    // The sender has been seen enough to be a boss.
    let store = UserStateStore::new(Arc::new(w.app.clone()));
    let seen = tunables.boss_min_seen + 5;
    store
        .update(&w.user, move |s: &mut UserState| {
            s.sender_stats.insert(
                "boss@example.com".to_owned(),
                domain::SenderStats {
                    seen,
                    ..domain::SenderStats::default()
                },
            );
        })
        .await?;
    assert!(domain::feed::is_boss(
        "boss@example.com",
        &w.state().await?.sender_stats,
        &tunables
    ));
    let id = w.seed("boss", bulk_no_header_facts(), 10);
    let result = w.reject(&session, 1, &id).await?;
    assert!(result.boss_defeated);
    let state = w.state().await?;
    assert!(state.sender_stats["boss@example.com"].boss_defeated);
    assert!(!domain::feed::is_boss(
        "boss@example.com",
        &state.sender_stats,
        &tunables
    ));

    // A reject of a non-boss does not report a defeat.
    let other = w.seed("minor", bulk_no_header_facts(), 9);
    assert!(!w.reject(&session, 2, &other).await?.boss_defeated);
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC5: claimed or cancelled exactly once
// ---------------------------------------------------------------------------

fn queued_job(n: u128) -> JobRecord {
    let due = OffsetDateTime::from_unix_timestamp(BASE).unwrap();
    JobRecord {
        job_id: JobId(Uuid::from_u128(n)),
        user_id: UserId::new(Uuid::from_u128(n ^ 1)),
        mailbox_id: MailboxId::new(Uuid::from_u128(n ^ 2)),
        list_key_hash: ListKeyHash([7; 32]),
        method: JobMethod::OneClick,
        target: Some(Ciphertext(vec![1, 2, 3])),
        sender_display: None,
        due_at: due,
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: due + Duration::minutes(60),
    }
}

proptest! {
    #[test]
    fn sw_05_ac5_new_job_claimed_once(n in 1u128..u128::MAX, claim_first in any::<bool>()) {
        let (_ports, fakes) = fake_ports();
        let outcomes = futures::executor::block_on(async {
            let created = queued_job(n);
            let version = fakes
                .store
                .jobs()
                .put(&created, Precondition::MustNotExist)
                .await
                .map_err(|e| format!("{e:?}"))?;
            let running = JobRecord { status: JobStatus::Running, ..created.clone() };
            let cancelled = JobRecord { status: JobStatus::Cancelled, target: None, ..created };
            let jobs = fakes.store.jobs();
            let pre = Precondition::Matches(version);
            let (claim, cancel) = if claim_first {
                let c = jobs.put(&running, pre.clone()).await;
                let x = jobs.put(&cancelled, pre).await;
                (c, x)
            } else {
                let x = jobs.put(&cancelled, pre.clone()).await;
                let c = jobs.put(&running, pre).await;
                (c, x)
            };
            Ok::<_, String>((claim.is_ok(), cancel.is_ok()))
        });
        let (claimed, cancelled) = outcomes.map_err(TestCaseError::fail)?;
        prop_assert!(claimed ^ cancelled, "exactly one conditional write wins");
    }
}

// ---------------------------------------------------------------------------
// ASVS V2.3: idempotency and the rate limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v2_3_4_retried_reject_one_job_one_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let first = w.reject(&session, 7, &id).await?;
    let second = w.reject(&session, 7, &id).await?;
    assert_eq!(first.outcome, second.outcome);
    assert_eq!(first.unsubscribe_due_at, second.unsubscribe_due_at);
    assert_eq!(w.jobs().await?.len(), 1, "one job");
    assert_eq!(w.scheduled_count(), 1, "one task");
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 1, "one trash");
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1, "one rule");
    assert_eq!(state.totals.unsubscribes_queued, 1);
    assert_eq!(state.totals.triaged, 1);
    Ok(())
}

#[tokio::test]
async fn asvs_v2_3_2_job_rate_limit_before_trash() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    // 300 jobs queued earlier today.
    for n in 0..300_u128 {
        w.app
            .limits
            .check(
                &policies::UNSUB_JOBS,
                LimitSubject::User(&w.user),
                RequestId(Uuid::from_u128(n)),
            )
            .await
            .map_err(|e| format!("{e:?}"))?;
    }
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let req = w
        .reject_request(&session, &id, MessageClass::Notice)
        .await?;
    let err = err_of(swipe(&w.app, &session, Uuid::from_u128(301), req).await);
    assert!(matches!(err, ApiError::RateLimited { .. }));
    // Refused before any provider change.
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 0);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    assert!(w.jobs().await?.is_empty());
    assert_eq!(w.scheduled_count(), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reject_schedule_failure_restores_message() -> TestResult {
    let w = World::with_failing_scheduler().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    assert_eq!(before, label_set(&["INBOX", "UNREAD"]));
    let req = w
        .reject_request(&session, &id, MessageClass::Notice)
        .await?;
    let err = err_of(swipe(&w.app, &session, Uuid::from_u128(1), req).await);
    assert_eq!(
        err,
        ApiError::ProviderUnavailable {
            mailbox_id: Some(w.primary.0),
            retry_after_s: None
        }
    );
    // The trash is undone and the job is gone.
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 1);
    assert_eq!(w.fakes.mailbox.calls(MailOp::RestoreLabels), 1);
    assert!(w.jobs().await?.is_empty());
    let state = w.state().await?;
    assert!(state.rules.is_empty());
    assert!(state.recent_swipes.is_empty());
    assert_eq!(state.totals.triaged, 0);
    Ok(())
}

/// Security review R1: a failure after the unsubscribe job is queued rolls the
/// job back and restores the labels, so a reject can never leave a trashed
/// message with an orphan job and no rule, stats or History entry. The
/// user-state write is the last step, so failing it exercises exactly the
/// window the earlier code left unrolled-back.
#[tokio::test]
async fn sw_03_ac1_reject_failure_after_queue_restores_message() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", one_click_facts(Some(LIST_ID)), 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    assert_eq!(before, label_set(&["INBOX", "UNREAD"]));
    // The job is queued first; the user-state write then fails with a transient
    // provider error.
    w.fakes.app_folder.fail_op(FolderOp::Write, 1);
    let req = w
        .reject_request(&session, &id, MessageClass::Notice)
        .await?;
    let err = err_of(swipe(&w.app, &session, Uuid::from_u128(1), req).await);
    assert_eq!(
        err,
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None
        }
    );
    // The trash is undone and the job is gone: no orphan job, no rule, no stats.
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    assert!(w.jobs().await?.is_empty());
    let state = w.state().await?;
    assert!(state.rules.is_empty());
    assert!(state.recent_swipes.is_empty());
    assert!(state.pending_unsubscribes.is_empty());
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.unsubscribes_queued, 0);
    Ok(())
}
