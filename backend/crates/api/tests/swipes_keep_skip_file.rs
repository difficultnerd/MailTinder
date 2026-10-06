//! T-604 swipe tests (SW-01, SW-02, SW-04, SW-05, FL-02 AC1, FL-03 AC2,
//! CL-03 AC4, INV-5, INV-6, ASVS V2.3.4, V8.2.2, V9.2.2, FD-04 AC1).
//!
//! Service integration: everything runs on `FakeMailbox`, the in-memory store,
//! keys and app folder, and the virtual clock. The handlers only add the
//! `Idempotency-Key` header and the rate limit, so the services are called
//! directly.

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

use std::collections::{BTreeMap, BTreeSet};
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
use domain::user_state::{Category, UserState};
use domain::{
    Classification, EmailAddress, LabelSet, MailboxId, MessageClass, MessageId, Provider,
    ProviderSubjectId, SenderKey, SenderStats, SwipeAction, SwipeOutcome, SwipeRecord, UserId,
    HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, KeyService, MailError, MailProvider, MailboxCtx, MailboxRecord,
    Precondition, Rng, ServerStore, SessionHash, SessionRecordId, UserRecord, WrappedKey,
};
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const BASE: i64 = 1_790_000_000;

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

/// A fixed `Classification`, so a sealed token is easy to build.
fn notice() -> Classification {
    Classification {
        class: MessageClass::Notice,
        bulk_score: 10,
        bulk_reason: "notice".to_owned(),
        confidence: None,
        probabilities: None,
    }
}

/// The error of a call that must fail, without needing the `Ok` type to be
/// `Debug`.
fn err_of<T>(result: Result<T, ApiError>) -> ApiError {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
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
    is_primary: bool,
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
                is_primary,
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
        let mut world = World {
            app,
            fakes,
            user,
            primary: MailboxId::new(Uuid::nil()),
        };
        world.primary = world.add_mailbox("sub-a", "a@example.com", true).await?;
        Ok(world)
    }

    async fn add_mailbox(
        &self,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        let id = seed_mailbox(&self.fakes, self.user, sub, email, is_primary).await?;
        let token = format!("refresh-{sub}");
        self.app
            .tokens
            .store_refresh_token(&self.app, &self.user, &id, Sensitive::new(token.clone()))
            .await?;
        self.fakes
            .identity
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(id)
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

    fn seed(&self, mailbox: &MailboxId, sender: &str, offset_s: i64) -> MessageId {
        self.fakes.mailbox.seed(
            mailbox,
            SeedMessage {
                from_display: format!("Name of {sender}"),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts: domain::HeaderFacts::default(),
                preview_text: format!("Preview at {offset_s}"),
                internal_date: at(offset_s),
                labels: vec!["INBOX".to_owned()],
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

    async fn ctx(&self) -> Result<MailboxCtx, Box<dyn std::error::Error>> {
        Ok(self
            .app
            .tokens
            .mailbox_ctx(&self.app, &self.user, &self.primary)
            .await?)
    }

    /// A sealed classification token for `message_id`, valid for `ttl_s`.
    async fn classification_token(
        &self,
        session: &AuthedSession,
        message_id: &str,
        ttl_s: i64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = ClassificationPayload {
            mailbox_id: self.primary.0,
            message_id: message_id.to_owned(),
            header_rules: notice(),
            classifier_id: HEADER_RULES_ID.to_owned(),
        };
        Ok(self
            .sealer()
            .seal(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                self.app.ports.clock.now() + Duration::seconds(ttl_s),
                &payload,
            )
            .await?)
    }

    async fn request(
        &self,
        session: &AuthedSession,
        id: &MessageId,
        action: ActionDto,
        name: Option<&str>,
    ) -> Result<SwipeRequest, Box<dyn std::error::Error>> {
        Ok(SwipeRequest {
            mailbox_id: self.primary.0,
            message_id: id.as_str().to_owned(),
            action,
            category_id: None,
            new_category_name: name.map(str::to_owned),
            classification_token: self
                .classification_token(session, id.as_str(), 3600)
                .await?,
        })
    }

    async fn swipe(
        &self,
        session: &AuthedSession,
        key: u128,
        req: SwipeRequest,
    ) -> Result<SwipeResultDto, ApiError> {
        swipe(&self.app, session, Uuid::from_u128(key), req).await
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

    fn labels_of(&self, id: &MessageId) -> Option<LabelSet> {
        self.fakes.mailbox.labels_of(&self.primary, id)
    }
}

fn label_set(ids: &[&str]) -> LabelSet {
    LabelSet::from_ids(ids.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>())
}

// ---------------------------------------------------------------------------
// SW-01 keep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_01_ac1_keep_leaves_labels_unchanged() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::Keep, None).await?,
        )
        .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    assert!(w.labels_of(&id).is_some_and(|l| l.contains("INBOX")));
    // No provider write at all: keep is local bookkeeping only.
    assert_eq!(w.fakes.mailbox.calls(MailOp::SetLabels), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 0);
    assert!(result.undo_token.starts_with("mt1.undo."));
    Ok(())
}

#[tokio::test]
async fn sw_01_ac2_keep_counted_for_sender() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let _ = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::Keep, None).await?,
        )
        .await?;
    let state = w.state().await?;
    let stats = state
        .sender_stats
        .get("alice@example.com")
        .ok_or("counted")?;
    assert_eq!(stats.keeps, 1);
    assert_eq!(state.totals.triaged, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-02 skip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_02_ac1_skip_changes_nothing() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::Skip, None).await?,
        )
        .await?;
    assert_eq!(result.outcome, SwipeOutcome::Skipped);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    assert_eq!(w.fakes.mailbox.calls(MailOp::SetLabels), 0);
    let state = w.state().await?;
    let key = format!("{}/{}", w.primary.0, id.as_str());
    assert_eq!(state.skips.counts.get(&key), Some(&1));
    assert_eq!(state.skips.queue.len(), 1);
    assert_eq!(state.totals.triaged, 0);
    Ok(())
}

#[tokio::test]
async fn sw_02_ac2_third_skip_not_queued() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    for key in 1..=3 {
        let req = w.request(&session, &id, ActionDto::Skip, None).await?;
        let result = w.swipe(&session, key, req).await?;
        assert_eq!(result.outcome, SwipeOutcome::Skipped);
    }
    let state = w.state().await?;
    let k = format!("{}/{}", w.primary.0, id.as_str());
    assert_eq!(state.skips.counts.get(&k), Some(&3));
    // The third skip reaches the cap, so the card is not queued again.
    assert_eq!(state.skips.queue.len(), 2);
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-04 AC2 / FL-02 AC1 / FL-03 AC2 file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_04_ac2_file_applies_label_and_leaves_inbox() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    assert_eq!(result.outcome, SwipeOutcome::Filed);
    let labels = w.labels_of(&id).ok_or("still present")?;
    assert!(!labels.contains("INBOX"), "left the inbox");
    assert!(labels.iter().any(|l| l.starts_with("Label_")));
    let filed = result.filed_category.ok_or("filed category")?;
    assert_eq!(filed.name, "Tax");
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 1);
    assert_eq!(state.totals.cleared, 1);
    assert_eq!(state.history.len(), 1);
    Ok(())
}

#[tokio::test]
async fn fl_02_ac1_new_category_creates_label() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let first = w.seed(&w.primary, "alice", 10);
    let second = w.seed(&w.primary, "bob", 9);
    let r1 = w
        .swipe(
            &session,
            1,
            w.request(&session, &first, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    // The second swipe names the same category in other case: it is reused.
    let r2 = w
        .swipe(
            &session,
            2,
            w.request(&session, &second, ActionDto::File, Some("tax"))
                .await?,
        )
        .await?;
    let c1 = r1.filed_category.ok_or("first category")?;
    let c2 = r2.filed_category.ok_or("second category")?;
    assert_eq!(c1.category_id, c2.category_id, "case-insensitive reuse");
    let state = w.state().await?;
    assert_eq!(state.categories.len(), 1);
    assert_eq!(state.totals.categories_created, 1);
    let category: &Category = &state.categories[0];
    assert_eq!(category.labels.len(), 1, "one lazily created label");
    assert_eq!(
        w.labels_of(&first)
            .and_then(|l| l.iter().find(|v| v.starts_with("Label_")).cloned()),
        w.labels_of(&second)
            .and_then(|l| l.iter().find(|v| v.starts_with("Label_")).cloned())
    );
    Ok(())
}

#[tokio::test]
async fn fl_03_ac2_no_filing_without_file_request() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let keep = w.seed(&w.primary, "alice", 10);
    let skip = w.seed(&w.primary, "bob", 9);
    let _ = w
        .swipe(
            &session,
            1,
            w.request(&session, &keep, ActionDto::Keep, None).await?,
        )
        .await?;
    let _ = w
        .swipe(
            &session,
            2,
            w.request(&session, &skip, ActionDto::Skip, None).await?,
        )
        .await?;
    assert_eq!(w.fakes.mailbox.calls(MailOp::SetLabels), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::EnsureLabel), 0);
    let state = w.state().await?;
    assert!(state.categories.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 undo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sw_05_ac1_undo_file_restores_exact_labels() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert!(!undone.unsubscribe_already_sent);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert!(state.history.is_empty());
    Ok(())
}

#[tokio::test]
async fn sw_05_ac1_undo_restore_failure_keeps_token_valid() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    // The provider refuses the first restore: `502` and nothing changed.
    w.fakes
        .mailbox
        .fail_next(MailOp::RestoreLabels, MailError::Forbidden);
    let err = w.undo(&session, &result.undo_token).await.unwrap_err();
    assert_eq!(
        err,
        ApiError::ProviderError {
            mailbox_id: Some(w.primary.0)
        }
    );
    assert!(w.labels_of(&id).is_some_and(|l| !l.contains("INBOX")));
    // The token is still valid: the retry succeeds.
    let undone = w.undo(&session, &result.undo_token).await?;
    assert!(undone.restored);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    Ok(())
}

#[tokio::test]
async fn sw_05_ac4_undo_walks_back_three_swipes() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let kept = w.seed(&w.primary, "alice", 10);
    let skipped = w.seed(&w.primary, "bob", 9);
    let filed = w.seed(&w.primary, "carol", 8);
    let before_kept = w.labels_of(&kept).ok_or("kept")?;
    let before_skipped = w.labels_of(&skipped).ok_or("skipped")?;
    let before_filed = w.labels_of(&filed).ok_or("filed")?;
    let r1 = w
        .swipe(
            &session,
            1,
            w.request(&session, &kept, ActionDto::Keep, None).await?,
        )
        .await?;
    let r2 = w
        .swipe(
            &session,
            2,
            w.request(&session, &skipped, ActionDto::Skip, None).await?,
        )
        .await?;
    let r3 = w
        .swipe(
            &session,
            3,
            w.request(&session, &filed, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    // One undo per call, newest first.
    assert!(w.undo(&session, &r3.undo_token).await?.restored);
    assert!(w.undo(&session, &r2.undo_token).await?.restored);
    assert!(w.undo(&session, &r1.undo_token).await?.restored);
    assert_eq!(w.labels_of(&filed).ok_or("filed")?, before_filed);
    assert_eq!(w.labels_of(&skipped).ok_or("skipped")?, before_skipped);
    assert_eq!(w.labels_of(&kept).ok_or("kept")?, before_kept);
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert_eq!(state.skips.counts.len(), 0);
    assert!(state.skips.queue.is_empty());
    assert!(state.recent_swipes.is_empty());
    Ok(())
}

#[tokio::test]
async fn sw_05_ac1_undo_same_token_twice_is_expired() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let before = w.labels_of(&id).ok_or("seeded")?;
    let result = w
        .swipe(
            &session,
            1,
            w.request(&session, &id, ActionDto::File, Some("Tax"))
                .await?,
        )
        .await?;
    assert!(w.undo(&session, &result.undo_token).await?.restored);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    let calls = w.fakes.mailbox.calls(MailOp::RestoreLabels);
    assert_eq!(calls, 1);
    // The token is single-use: the replay is refused and touches nothing.
    let err = w.undo(&session, &result.undo_token).await.unwrap_err();
    assert_eq!(err, ApiError::UndoExpired);
    assert_eq!(w.fakes.mailbox.calls(MailOp::RestoreLabels), calls);
    assert_eq!(w.labels_of(&id).ok_or("still present")?, before);
    let state = w.state().await?;
    assert_eq!(state.totals.triaged, 0);
    assert_eq!(state.totals.cleared, 0);
    assert!(state.history.is_empty());
    assert!(state.recent_swipes.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// CL-03 AC4 / V9.2.2 classification token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cl_03_ac4_token_for_other_message_refused() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let target = w.seed(&w.primary, "alice", 10);
    let _other = w.seed(&w.primary, "bob", 9);
    let mut req = w.request(&session, &target, ActionDto::Keep, None).await?;
    // A token that opens but names a different message is refused.
    req.classification_token = w.classification_token(&session, "m0002", 3600).await?;
    let err = err_of(w.swipe(&session, 1, req).await);
    assert!(matches!(err, ApiError::InvalidRequest { .. }));
    Ok(())
}

#[tokio::test]
async fn cl_03_ac4_expired_token_swipe_proceeds() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let mut req = w.request(&session, &id, ActionDto::Keep, None).await?;
    req.classification_token = w.classification_token(&session, id.as_str(), 60).await?;
    // The token expires before the swipe is sent: the swipe still proceeds.
    w.fakes.clock.advance(Duration::seconds(120));
    let result = w.swipe(&session, 1, req).await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_2_undo_token_as_classification_refused() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    // Seal an undo token and present it as a classification token.
    let payload = UndoPayload {
        record: SwipeRecord {
            mailbox: w.primary,
            message: id.clone(),
            sender: SenderKey::from_address("alice@example.com"),
            action: SwipeAction::Keep,
            outcome: SwipeOutcome::Kept,
            previous_labels: label_set(&["INBOX"]),
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
    let undo_token = w
        .sealer()
        .seal(
            TokenType::Undo,
            &w.user,
            &w.wrapped().await?,
            &session.session_record_id,
            w.fakes.clock.now() + Duration::hours(1),
            &payload,
        )
        .await?;
    let mut req = w.request(&session, &id, ActionDto::Keep, None).await?;
    req.classification_token = undo_token;
    let err = err_of(w.swipe(&session, 1, req).await);
    assert!(matches!(err, ApiError::InvalidRequest { .. }));
    Ok(())
}

// ---------------------------------------------------------------------------
// Idempotency and ownership
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v2_3_4_retried_swipe_counts_once() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let first = w
        .swipe(
            &session,
            7,
            w.request(&session, &id, ActionDto::Keep, None).await?,
        )
        .await?;
    let second = w
        .swipe(
            &session,
            7,
            w.request(&session, &id, ActionDto::Keep, None).await?,
        )
        .await?;
    assert_eq!(first.outcome, second.outcome);
    let state = w.state().await?;
    assert_eq!(state.sender_stats["alice@example.com"].keeps, 1);
    assert_eq!(state.totals.triaged, 1);
    // The retry is answered from the recorded response: no second re-read.
    assert_eq!(w.fakes.mailbox.calls(MailOp::GetMeta), 1);
    assert!(second.undo_token.starts_with("mt1.undo."));
    Ok(())
}

#[tokio::test]
async fn asvs_v8_2_2_swipe_on_other_users_mailbox_not_found() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    // Another user's mailbox, in the same store.
    let other_user = UserId::new(w.fakes.rng.uuid_v4());
    let wrapped = w.fakes.keys.new_user_key(&other_user).await?;
    w.fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: other_user,
                created_at: w.fakes.clock.now(),
                is_admin: false,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    let theirs = seed_mailbox(&w.fakes, other_user, "sub-b", "b@example.com", true).await?;
    let id = w.seed(&w.primary, "alice", 10);
    let mut req = w.request(&session, &id, ActionDto::Keep, None).await?;
    req.mailbox_id = theirs.0;
    let err = err_of(w.swipe(&session, 1, req).await);
    assert_eq!(err, ApiError::NotFound);
    Ok(())
}

#[tokio::test]
async fn fd_04_ac1_swipe_on_moved_message_conflict() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(&w.primary, "alice", 10);
    let req = w.request(&session, &id, ActionDto::Keep, None).await?;
    // The message left the inbox between the card and the swipe.
    let ctx = w.ctx().await?;
    w.fakes
        .mailbox
        .set_labels(&ctx, &id, &LabelSet::new(), &label_set(&["INBOX"]))
        .await?;
    let err = err_of(w.swipe(&session, 1, req).await);
    assert_eq!(err, ApiError::MessageChanged);
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-5, INV-6
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv_5_swipe_paths_never_delete() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let ids = [
        w.seed(&w.primary, "alice", 10),
        w.seed(&w.primary, "bob", 9),
        w.seed(&w.primary, "carol", 8),
    ];
    let actions = [ActionDto::Keep, ActionDto::Skip, ActionDto::File];
    for (i, action) in actions.iter().enumerate() {
        let req = w
            .request(
                &session,
                &ids[i],
                *action,
                matches!(action, ActionDto::File).then_some("Tax"),
            )
            .await?;
        let _ = w.swipe(&session, 100 + i as u128, req).await?;
        // The message still exists after every action: nothing is deleted.
        assert!(w.labels_of(&ids[i]).is_some(), "message removed");
    }
    // No trash, no spam report: those are reject-only (T-605), and no delete
    // exists on the provider at all (INV-5).
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::ReportSpam), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::RemoveLabel), 0);
    Ok(())
}

/// Stats with the all-default entries dropped, so a fully undone swipe leaves
/// the same picture it started from.
fn normalised(state: &UserState) -> (BTreeMap<String, SenderStats>, u64, u64) {
    let stats: BTreeMap<String, SenderStats> = state
        .sender_stats
        .iter()
        .filter(|(_, v)| **v != SenderStats::default())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (stats, state.totals.triaged, state.totals.cleared)
}

#[tokio::test]
async fn inv_6_every_session_swipe_undoable() -> TestResult {
    let alphabet = [ActionDto::Keep, ActionDto::Skip, ActionDto::File];
    for len in 1..=3_u32 {
        for n in 0..3_u32.pow(len) {
            let mut seq = Vec::new();
            let mut x = n;
            for _ in 0..len {
                seq.push(alphabet[(x % 3) as usize]);
                x /= 3;
            }
            run_sequence(&seq).await?;
        }
    }
    Ok(())
}

/// One sequence of swipes on distinct messages, then the same number of undos
/// newest first; labels and stats come back to where they started.
async fn run_sequence(seq: &[ActionDto]) -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    // The picture before any swipe: every sequence must return to exactly this.
    let initial = normalised(&w.state().await?);
    assert!(initial.0.is_empty(), "nothing counted before any swipe");
    let mut before = Vec::new();
    let mut tokens = Vec::new();
    let mut senders = BTreeSet::new();
    for (i, action) in seq.iter().enumerate() {
        let sender = format!("s{i}");
        senders.insert(format!("{sender}@example.com"));
        let id = w.seed(&w.primary, &sender, 10 - i as i64);
        before.push((id.clone(), w.labels_of(&id).ok_or("seeded")?));
        let name = matches!(action, ActionDto::File).then_some("Invoices");
        let req = w.request(&session, &id, *action, name).await?;
        let result = w.swipe(&session, 1000 + i as u128, req).await?;
        tokens.push(result.undo_token);
    }
    for token in tokens.iter().rev() {
        assert!(w.undo(&session, token).await?.restored);
    }
    for (id, labels) in &before {
        assert_eq!(w.labels_of(id).ok_or("still present")?, *labels);
    }
    let after = w.state().await?;
    assert_eq!(normalised(&after), initial);
    assert!(after.recent_swipes.is_empty());
    assert!(after.history.is_empty());
    for sender in senders {
        assert!(
            after
                .sender_stats
                .get(&sender)
                .map_or(true, |s| *s == SenderStats::default()),
            "{sender} left a non-zero count"
        );
    }
    Ok(())
}
