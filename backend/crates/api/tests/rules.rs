//! T-608 rules endpoints and block prompt decline (SR-01 AC4, PB-01 AC2,
//! PB-01 AC3, FL-04 AC2, ST-01 AC2, GM-05 AC2, ASVS V9.2.2, V8.2.2).
//!
//! Service integration: the in-memory store, keys, app folder and
//! `FakeMailbox`, with the virtual clock. The handlers only add the default
//! rate limits and the CSRF check, so the services are called directly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::unused_self,
    clippy::assert_is_empty
)]

use std::sync::Arc;

use api::error::ApiError;
use api::routes::rules::RuleDto;
use api::sealed::{SealedTokens, TokenType};
use api::services::categories::create as create_category;
use api::services::reject::BlockPromptRef;
use api::services::rules::{create_block, create_file, decline, delete, list, patch};
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use domain::user_state::{HistoryAction, HistoryOutcome, StoredRule, UserState};
use domain::{
    BlockPrompt, HeaderFacts, MailboxId, Provider, ProviderSubjectId, RuleId, SenderKey, SortRule,
    Tunables, UserId,
};
use obs::Sensitive;
use ports::store::{aad_fields, Precondition};
use ports::{
    Aad, Clock, KeyService, MailboxRecord, Rng, ServerStore, SessionHash, SessionRecordId,
    UserRecord,
};
use testkit::{fake_ports, Fakes, SeedMessage};
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
    let address = domain::EmailAddress::parse(email)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes.store.users().get(&user).await?.ok_or("seeded user")?;
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
                email_address: ports::Ciphertext(sealed),
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
        let mut world = World {
            app,
            fakes,
            user: UserId::new(Uuid::nil()),
            primary: MailboxId::new(Uuid::nil()),
        };
        let user = world.add_user().await?;
        world.user = user;
        world.primary = world.add_mailbox("sub-a", "a@example.com", true).await?;
        Ok(world)
    }

    async fn add_user(&self) -> Result<UserId, Box<dyn std::error::Error>> {
        let user = UserId::new(self.fakes.rng.uuid_v4());
        let wrapped = self.fakes.keys.new_user_key(&user).await?;
        self.fakes
            .store
            .users()
            .put(
                &UserRecord {
                    user_id: user,
                    created_at: self.fakes.clock.now(),
                    is_admin: false,
                    wrapped_data_key: wrapped,
                    experiments_consent_version: None,
                    experiments_opted_in_at: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        Ok(user)
    }

    async fn other_user(&self) -> Result<(UserId, MailboxId), Box<dyn std::error::Error>> {
        let user = self.add_user().await?;
        let mailbox = self
            .add_mailbox_for(user, "sub-b", "b@example.com", true)
            .await?;
        Ok((user, mailbox))
    }

    async fn add_mailbox(
        &self,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        self.add_mailbox_for(self.user, sub, email, is_primary)
            .await
    }

    async fn add_mailbox_for(
        &self,
        user: UserId,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        let id = seed_mailbox(&self.fakes, user, sub, email, is_primary).await?;
        let token = format!("refresh-{sub}");
        self.app
            .tokens
            .store_refresh_token(&self.app, &user, &id, Sensitive::new(token.clone()))
            .await?;
        self.fakes
            .identity
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(id)
    }

    fn session(&self, n: u128) -> AuthedSession {
        self.session_for(self.user, n)
    }

    fn session_for(&self, user: UserId, n: u128) -> AuthedSession {
        AuthedSession {
            user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    fn sealer(&self) -> SealedTokens {
        SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        )
    }

    async fn wrapped(&self) -> Result<ports::WrappedKey, Box<dyn std::error::Error>> {
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

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(&self.user)
            .await?
            .state)
    }

    async fn state_of(&self, user: &UserId) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(user)
            .await?
            .state)
    }

    /// Seed one inbox message (the fake keeps the labels verbatim).
    fn seed(&self, sender: &str, offset_s: i64) -> domain::MessageId {
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {sender}"),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts: HeaderFacts::default(),
                preview_text: format!("Preview at {offset_s}"),
                internal_date: at(offset_s),
                labels: vec!["INBOX".to_owned()],
            },
        )
    }

    /// Seal the prompt reference a reject would have raised (PB-01 AC1).
    async fn seal_prompt(
        &self,
        session: &AuthedSession,
        sender_key: &str,
        swipe_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = BlockPromptRef {
            sender_key: sender_key.to_owned(),
            sender_display: format!("Name of {sender_key}"),
            mailbox_id: self.primary.0,
            swipe_id,
        };
        let expires_at = self.app.ports.clock.now() + Duration::hours(1);
        Ok(self
            .sealer()
            .seal(
                TokenType::PromptRef,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                expires_at,
                &payload,
            )
            .await?)
    }

    /// Create a filing rule via API-RULE-2, for the tests that need a rule.
    async fn file_rule(
        &self,
        session: &AuthedSession,
        sender: &str,
    ) -> Result<RuleDto, Box<dyn std::error::Error>> {
        let category = create_category(&self.app, session, "Bills").await?;
        let message = self.seed(sender, 10);
        Ok(create_file(
            &self.app,
            session,
            self.primary.0,
            message.as_str(),
            category.category_id,
        )
        .await?)
    }
}

// ---------------------------------------------------------------------------
// PB-01 AC2: a confirmed prompt creates one block rule, listed in History
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pb_01_ac2_block_rule_from_prompt_ref() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let swipe = Uuid::from_u128(0xabc);
    let prompt_ref = w.seal_prompt(&session, "alice@example.com", swipe).await?;

    let rule = create_block(&w.app, &session, &prompt_ref).await?;
    assert_eq!(rule.kind, "block_person");
    assert_eq!(rule.r#match.sender_address, "alice@example.com");
    assert_eq!(rule.r#match.list_id, None);
    assert_eq!(rule.category_id, None);
    assert!(rule.enabled);
    assert_eq!(rule.times_applied, 0);
    assert_eq!(rule.yearly_rate, None);

    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert_eq!(
        state.rules.first().map(|r| r.rule.rule_id),
        Some(RuleId(rule.rule_id))
    );
    assert_eq!(state.totals.people_blocked, 1);
    assert_eq!(state.totals.senders_silenced, 1);
    let entry = state
        .history
        .iter()
        .find(|h| h.rule_id == Some(RuleId(rule.rule_id)))
        .ok_or("history entry")?;
    assert_eq!(entry.action, HistoryAction::Blocked);
    assert_eq!(entry.outcome, HistoryOutcome::Done);
    assert_eq!(entry.entry_id, rule.rule_id);
    assert_eq!(entry.mailbox_id, w.primary);
    Ok(())
}

#[tokio::test]
async fn pb_01_ac2_retry_creates_one_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let swipe = Uuid::from_u128(0xabc);
    let prompt_ref = w.seal_prompt(&session, "alice@example.com", swipe).await?;

    let first = create_block(&w.app, &session, &prompt_ref).await?;
    let second = create_block(&w.app, &session, &prompt_ref).await?;
    assert_eq!(first.rule_id, second.rule_id);
    let state = w.state().await?;
    assert_eq!(
        state.rules.len(),
        1,
        "a retry does not create a second rule"
    );
    assert_eq!(state.totals.people_blocked, 1);
    assert_eq!(
        state
            .history
            .iter()
            .filter(|h| h.action == HistoryAction::Blocked)
            .count(),
        1,
        "the History entry is pushed once"
    );
    Ok(())
}

#[tokio::test]
async fn pb_01_ac2_retry_after_disable_creates_one_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let swipe = Uuid::from_u128(0xabd);
    let prompt_ref = w.seal_prompt(&session, "alice@example.com", swipe).await?;

    let first = create_block(&w.app, &session, &prompt_ref).await?;
    patch(&w.app, &session, first.rule_id, false).await?;
    let second = create_block(&w.app, &session, &prompt_ref).await?;
    assert_eq!(first.rule_id, second.rule_id);
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1, "a disabled rule is not duplicated");
    assert_eq!(state.totals.people_blocked, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// PB-01 AC3: declining suppresses the prompt for that sender for 90 days
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pb_01_ac3_decline_suppresses_for_90_days() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let prompt_ref = w
        .seal_prompt(&session, "alice@example.com", Uuid::from_u128(0xabc))
        .await?;
    decline(&w.app, &session, &prompt_ref).await?;

    let now0 = w.app.ports.clock.now();
    let state = w.state().await?;
    let stats = state
        .sender_stats
        .get("alice@example.com")
        .cloned()
        .ok_or("sender stats")?;
    assert_eq!(
        stats.block_prompt_declined_until,
        Some(now0 + Duration::days(90)),
        "the decline lasts 90 days"
    );

    // Day 89: still declined, so a personal reject raises no prompt.
    let t = Tunables::default();
    let mut day89 = stats.clone();
    day89.rejects_counted = vec![now0 + Duration::days(89) - Duration::hours(1); 3];
    assert_eq!(
        day89.record_reject(true, true, now0 + Duration::days(89), &t),
        BlockPrompt::Declined
    );

    // Day 91: the decline has lapsed and the threshold is met again.
    let mut day91 = stats;
    day91.rejects_counted = vec![now0 + Duration::days(91) - Duration::hours(1); 3];
    assert_eq!(
        day91.record_reject(true, true, now0 + Duration::days(91), &t),
        BlockPrompt::Ask
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// FL-04 AC2: accepting the keep prompt creates a filing rule from the re-read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fl_04_ac2_file_rule_from_message() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let category = create_category(&w.app, &session, "Bills").await?;
    let message = w.seed("carol", 10);

    // An unknown category is `404`.
    assert!(matches!(
        err_of(
            create_file(
                &w.app,
                &session,
                w.primary.0,
                message.as_str(),
                Uuid::from_u128(0xdead),
            )
            .await
        ),
        ApiError::NotFound
    ));

    let rule = create_file(
        &w.app,
        &session,
        w.primary.0,
        message.as_str(),
        category.category_id,
    )
    .await?;
    assert_eq!(rule.kind, "file");
    assert_eq!(rule.r#match.sender_address, "carol@example.com");
    assert_eq!(rule.category_id, Some(category.category_id));
    assert!(rule.enabled);

    // A second accept reuses the identical enabled rule.
    let again = create_file(
        &w.app,
        &session,
        w.primary.0,
        message.as_str(),
        category.category_id,
    )
    .await?;
    assert_eq!(rule.rule_id, again.rule_id);
    assert_eq!(w.state().await?.rules.len(), 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// SR-01 AC4 / ST-01 AC2: a rule switched off is stored off and returned off
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sr_01_ac4_disabled_rule_not_applied() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let rule = w.file_rule(&session, "alice").await?;

    // Before T-609 there is no Feed matcher to exercise, so the contract is
    // that `enabled: false` is stored and returned: `SortRule::matches` never
    // matches a disabled rule.
    let off = patch(&w.app, &session, rule.rule_id, false).await?;
    assert!(!off.enabled);
    let stored = w
        .state()
        .await?
        .rule(&RuleId(rule.rule_id))
        .cloned()
        .ok_or("stored rule")?;
    assert!(!stored.rule.enabled);
    Ok(())
}

#[tokio::test]
async fn st_01_ac2_switch_off_rule() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let rule = w.file_rule(&session, "alice").await?;
    assert!(rule.enabled);

    let off = patch(&w.app, &session, rule.rule_id, false).await?;
    assert!(!off.enabled);
    let on = patch(&w.app, &session, rule.rule_id, true).await?;
    assert!(on.enabled);
    Ok(())
}

// ---------------------------------------------------------------------------
// GM-05 AC2: each rule shows its yearly estimate, or null when unknown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gm_05_ac2_rule_lists_yearly_rate() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let known = RuleId(Uuid::from_u128(0x1111));
    let unknown = RuleId(Uuid::from_u128(0x2222));
    let store = UserStateStore::new(Arc::new(w.app.clone()));
    store
        .update(&w.user, move |s| {
            s.rules.push(StoredRule {
                rule: SortRule::block_person_for(
                    &SenderKey::from_address("alice@example.com"),
                    known,
                    at(0),
                    None,
                ),
                times_applied: 4,
                yearly_rate: Some(120),
            });
            s.rules.push(StoredRule {
                rule: SortRule::block_person_for(
                    &SenderKey::from_address("bob@example.com"),
                    unknown,
                    at(1),
                    None,
                ),
                times_applied: 0,
                yearly_rate: None,
            });
        })
        .await?;

    let rules = list(&w.app, &session, None).await?;
    assert_eq!(rules.len(), 2);
    // Newest first: `bob` (created at offset 1) precedes `alice`.
    assert_eq!(rules.first().map(|r| r.rule_id), Some(unknown.0));
    let alice = rules
        .iter()
        .find(|r| r.rule_id == known.0)
        .ok_or("alice rule")?;
    assert_eq!(alice.yearly_rate, Some(120));
    assert_eq!(alice.times_applied, 4);
    let bob = rules
        .iter()
        .find(|r| r.rule_id == unknown.0)
        .ok_or("bob rule")?;
    assert_eq!(bob.yearly_rate, None);

    let blocks = list(&w.app, &session, Some("block_person")).await?;
    assert_eq!(blocks.len(), 2);
    assert!(list(&w.app, &session, Some("file")).await?.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V9.2.2: a token of another type is refused as a prompt reference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v9_2_2_classification_token_as_prompt_ref_refused() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let expires_at = w.app.ports.clock.now() + Duration::hours(1);
    let other_type = w
        .sealer()
        .seal(
            TokenType::Classification,
            &w.user,
            &w.wrapped().await?,
            &session.session_record_id,
            expires_at,
            &"payload",
        )
        .await?;

    assert!(matches!(
        err_of(create_block(&w.app, &session, &other_type).await),
        ApiError::InvalidRequest { .. }
    ));
    assert!(matches!(
        err_of(decline(&w.app, &session, &other_type).await),
        ApiError::InvalidRequest { .. }
    ));
    // A plain string is not a sealed token either.
    assert!(matches!(
        err_of(create_block(&w.app, &session, "not-a-token").await),
        ApiError::InvalidRequest { .. }
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.2: another user's rule is not found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v8_2_2_other_users_rule_not_found() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let rule = w.file_rule(&session, "alice").await?;
    let (other, _) = w.other_user().await?;
    let other_session = w.session_for(other, 2);

    assert!(matches!(
        err_of(patch(&w.app, &other_session, rule.rule_id, false).await),
        ApiError::NotFound
    ));
    assert!(matches!(
        err_of(delete(&w.app, &other_session, rule.rule_id).await),
        ApiError::NotFound
    ));
    assert!(list(&w.app, &other_session, None).await?.is_empty());
    // The owner can still delete their own rule.
    delete(&w.app, &session, rule.rule_id).await?;
    assert!(list(&w.app, &session, None).await?.is_empty());
    assert_eq!(w.state_of(&other).await?.rules.len(), 0);
    Ok(())
}
