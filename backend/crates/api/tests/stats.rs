//! T-802: stats reads and achievements through the real API services.
#![allow(clippy::too_many_lines)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use api::routes::feed::{ClassificationPayload, FeedRequest};
use api::routes::stats::stats;
use api::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use api::sealed::{SealedTokens, TokenType};

use api::services::categories::create;
use api::services::feed::next_page;
use api::services::reject::BlockPromptRef;
use api::services::swipe::swipe;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::{app_state, build_router, config::ApiConfig, state::AppState};
use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use domain::user_state::{AchievementRecord, MailboxPosition, StoredRule, Totals, UserState};
use domain::AchievementId;
use domain::{
    Classification, HeaderFacts, MailboxId, MailboxStatus, MessageClass, MessageId, Provider,
    ProviderSubjectId, RuleId, RuleKind, RuleMatch, SenderKey, SortRule, SwipeOutcome,
    UnsubscribeOptions, UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AppFolderError, AppFolderStore, Ciphertext, Clock, ETag, KeyService, MailError,
    MailboxCtx, MailboxRecord, Precondition, Rng, ServerStore, UserRecord,
};
use serde_json::{json, Value};
use testkit::{fake_ports, Fakes, SeedMessage};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A deterministic, real ETag-conflict injection, not a transient write error.
struct ConflictFolder {
    inner: Arc<dyn AppFolderStore>,
    conflicts: AtomicU32,
    writes: AtomicU32,
}

#[async_trait]
impl AppFolderStore for ConflictFolder {
    async fn read(&self, ctx: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        self.inner.read(ctx).await
    }

    async fn write(
        &self,
        ctx: &MailboxCtx,
        bytes: &[u8],
        tag: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut remaining = self.conflicts.load(Ordering::SeqCst);
        while remaining > 0 {
            match self.conflicts.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Err(AppFolderError::Conflict),
                Err(current) => remaining = current,
            }
        }
        self.inner.write(ctx, bytes, tag).await
    }

    async fn delete(&self, ctx: &MailboxCtx) -> Result<(), MailError> {
        self.inner.delete(ctx).await
    }
}

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
    session: AuthedSession,
    cookie: String,
    csrf: String,
    folder: Arc<ConflictFolder>,
}

impl World {
    async fn new() -> TestResult<Self> {
        let (mut parts, fakes) = fake_ports();
        let folder = Arc::new(ConflictFolder {
            inner: parts.app_folder.clone(),
            conflicts: AtomicU32::new(0),
            writes: AtomicU32::new(0),
        });
        parts.app_folder = folder.clone();
        let config = ApiConfig::new(
            "https://mailtinder.test".into(),
            "fake-client".into(),
            Sensitive::new(b"fake-log-key".to_vec()),
            Sensitive::new(b"fake-email-key".to_vec()),
        )?;
        let app = app_state(Arc::new(parts), Arc::new(config));
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
                    wrapped_data_key: wrapped.clone(),
                    experiments_consent_version: None,
                    experiments_opted_in_at: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        let subject = ProviderSubjectId::new("synthetic-stats-subject")?;
        let mailbox = ports::mailbox_id_for(Provider::Gmail, &subject);
        let email = fakes
            .keys
            .seal(
                &user,
                &wrapped,
                &Aad {
                    user,
                    scope: mailbox.0.to_string(),
                    field: aad_fields::MAILBOX_EMAIL,
                },
                b"stats@example.com",
            )
            .await?;
        fakes
            .store
            .mailboxes()
            .put(
                &MailboxRecord {
                    mailbox_id: mailbox,
                    user_id: user,
                    provider: Provider::Gmail,
                    provider_subject_id: subject,
                    email_address: Ciphertext(email),
                    status: MailboxStatus::Connected,
                    linked_at: fakes.clock.now(),
                    is_primary: true,
                    refresh_token: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        app.tokens
            .store_refresh_token(
                &app,
                &user,
                &mailbox,
                Sensitive::new("stats-refresh".into()),
            )
            .await?;
        fakes
            .identity
            .script_refresh("stats-refresh", Ok("stats-access".into()));
        let (cookie, record) = SessionService::new(&app)
            .establish(None, &user, None)
            .await?;
        let cookie = cookie
            .0
            .to_str()?
            .split(';')
            .next()
            .ok_or("missing cookie")?
            .to_owned();
        let session = AuthedSession {
            user,
            session_record_id: record.record.session_record_id,
            is_admin: false,
            recent_auth_at: None,
            session_hash: record.record.session_hash,
        };
        let csrf = record.record.csrf_token.clone();
        Ok(Self {
            app,
            fakes,
            user,
            mailbox,
            session,
            cookie,
            csrf,
            folder,
        })
    }

    fn store(&self) -> UserStateStore {
        UserStateStore::new(Arc::new(self.app.clone()))
    }

    fn ctx(&self) -> MailboxCtx {
        MailboxCtx {
            mailbox: self.mailbox,
            access_token: Sensitive::new("stats-access".into()),
        }
    }

    async fn state(&self) -> TestResult<UserState> {
        Ok(self.store().load(&self.user).await?.state)
    }

    async fn seed(&self, state: UserState) -> TestResult {
        self.store()
            .update(&self.user, move |s| *s = state.clone())
            .await?;
        Ok(())
    }

    async fn meter(&self) -> TestResult<Value> {
        Ok(serde_json::to_value(
            stats(&self.app, &self.session).await?,
        )?)
    }

    async fn get(&self, cookie: Option<&str>) -> TestResult<(StatusCode, Value)> {
        let mut request = Request::builder().uri("/api/v1/stats");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = build_router(self.app.clone())
            .oneshot(request.body(Body::empty())?)
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    fn rule(&self, kind: RuleKind, enabled: bool, yearly_rate: Option<u32>) -> StoredRule {
        StoredRule {
            rule: SortRule {
                rule_id: RuleId(self.fakes.rng.uuid_v4()),
                kind,
                matcher: RuleMatch {
                    sender: SenderKey::from_address("synthetic@example.com"),
                    list_id: None,
                    feedback_id: None,
                },
                category: None,
                enabled,
                created_at: self.fakes.clock.now(),
                source_swipe: None,
            },
            times_applied: 0,
            yearly_rate,
        }
    }

    fn message(&self, n: u32, list: bool, date: OffsetDateTime) -> TestResult<MessageId> {
        let facts = if list {
            HeaderFacts {
                list_unsubscribe: Some(UnsubscribeOptions {
                    one_click_https: Some(Url::parse("https://lists.example.com/unsubscribe")?),
                    https: None,
                    mailto: None,
                }),
                list_unsubscribe_present: true,
                list_id: Some(format!("synthetic-{n}.example.com")),
                from_authenticated: true,
                ..HeaderFacts::default()
            }
        } else {
            HeaderFacts {
                from_authenticated: true,
                ..HeaderFacts::default()
            }
        };
        Ok(self.fakes.mailbox.seed(
            &self.mailbox,
            SeedMessage {
                from_display: format!("Synthetic sender {n}"),
                from_address: format!("sender-{n}@example.com"),
                subject: "Synthetic subject".into(),
                raw_headers: Vec::new(),
                facts,
                preview_text: String::new(),
                internal_date: date,
                labels: vec!["INBOX".into()],
            },
        ))
    }

    async fn request(
        &self,
        id: &MessageId,
        action: ActionDto,
        session: &AuthedSession,
    ) -> TestResult<SwipeRequest> {
        let wrapped = self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("missing user")?
            .record
            .wrapped_data_key;
        let token = SealedTokens::new(self.app.ports.keys.clone(), self.app.ports.clock.clone())
            .seal(
                TokenType::Classification,
                &self.user,
                &wrapped,
                &session.session_record_id,
                self.fakes.clock.now() + Duration::hours(1),
                &ClassificationPayload {
                    mailbox_id: self.mailbox.0,
                    message_id: id.as_str().into(),
                    header_rules: Classification {
                        class: MessageClass::Personal,
                        bulk_score: 0,
                        bulk_reason: "synthetic".into(),
                        confidence: None,
                        probabilities: None,
                    },
                    classifier_id: HEADER_RULES_ID.into(),
                    issued_at: self.fakes.clock.now(),
                    bakeoff: None,
                },
            )
            .await?;
        Ok(SwipeRequest {
            mailbox_id: self.mailbox.0,
            message_id: id.as_str().into(),
            action,
            category_id: None,
            new_category_name: (action == ActionDto::File).then(|| "Synthetic filing".into()),
            classification_token: token,
        })
    }

    async fn reject(&self, n: u32, session: &AuthedSession) -> TestResult<SwipeResultDto> {
        let id = self.message(n, true, self.fakes.clock.now())?;
        Ok(swipe(
            &self.app,
            session,
            self.fakes.rng.uuid_v4(),
            self.request(&id, ActionDto::Reject, session).await?,
        )
        .await?)
    }

    /// The wrapped data key of the World's user.
    async fn wrapped(&self) -> TestResult<ports::WrappedKey> {
        Ok(self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("missing user")?
            .record
            .wrapped_data_key)
    }

    /// Seal the prompt reference a reject would have raised (PB-01 AC1).
    async fn seal_prompt(
        &self,
        session: &AuthedSession,
        sender_key: &str,
        swipe_id: uuid::Uuid,
    ) -> TestResult<String> {
        let payload = BlockPromptRef {
            sender_key: sender_key.to_owned(),
            sender_display: format!("Name of {sender_key}"),
            mailbox_id: self.mailbox.0,
            swipe_id,
        };
        let expires_at = self.app.ports.clock.now() + Duration::hours(1);
        Ok(
            SealedTokens::new(self.app.ports.keys.clone(), self.app.ports.clock.clone())
                .seal(
                    TokenType::PromptRef,
                    &self.user,
                    &self.wrapped().await?,
                    &session.session_record_id,
                    expires_at,
                    &payload,
                )
                .await?,
        )
    }

    /// POST a `block_person` rule through the real route (API-RULE-2), so the
    /// block achievement path runs exactly as production runs it.
    async fn post_block(&self, prompt_ref: &str) -> TestResult<(StatusCode, Value)> {
        let body = serde_json::to_vec(&json!({
            "kind": "block_person",
            "prompt_ref": prompt_ref,
        }))?;
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/rules")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "https://mailtinder.test")
            .header(header::COOKIE, self.cookie.as_str())
            .header("x-csrf-token", self.csrf.as_str())
            .body(Body::from(body))?;
        let response = build_router(self.app.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }
}

fn has_unlock(state: &UserState, id: AchievementId) -> bool {
    state
        .achievements
        .iter()
        .any(|a| a.achievement_id == id.as_str())
}

fn assert_swipe_unlock(result: &SwipeResultDto, id: AchievementId) {
    assert!(
        result
            .achievements_unlocked
            .iter()
            .any(|a| a.achievement_id == id.as_str()),
        "missing {}",
        id.as_str()
    );
}

#[tokio::test]
async fn st_02_ac1_counts_reported() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        totals: Totals {
            triaged: 27,
            cleared: 19,
            senders_unsubscribed: 7,
            unsubscribes_confirmed: 5,
            unsubscribes_queued: 9,
            senders_silenced: 11,
            years_cleared: 3,
            categories_created: 4,
            people_blocked: 2,
            ..Totals::default()
        },
        ..UserState::default()
    })
    .await?;
    let value = w.meter().await?;
    for (field, count) in [
        ("emails_triaged", 27),
        ("senders_unsubscribed", 7),
        ("unsubscribes_confirmed", 5),
    ] {
        assert_eq!(value[field], json!(count), "{field}");
    }
    Ok(())
}

#[tokio::test]
async fn st_02_ac2_total_from_enabled_rules() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        rules: vec![
            w.rule(RuleKind::RejectList, true, Some(120)),
            w.rule(RuleKind::BlockPerson, true, Some(40)),
            w.rule(RuleKind::File, true, Some(25)),
            w.rule(RuleKind::RejectList, false, Some(999)),
            w.rule(RuleKind::BlockPerson, true, None),
        ],
        ..UserState::default()
    })
    .await?;
    assert_eq!(w.meter().await?["mail_stopped_per_year"], 185);
    Ok(())
}

#[tokio::test]
async fn st_02_ac3_unlocked_achievements_listed() -> TestResult {
    let w = World::new().await?;
    let now = w.fakes.clock.now();
    w.seed(UserState {
        achievements: vec![AchievementRecord {
            achievement_id: AchievementId::FirstUnsubscribe.as_str().into(),
            unlocked_at: now,
        }],
        ..UserState::default()
    })
    .await?;
    let achievements = w.meter().await?["achievements"]
        .as_array()
        .ok_or("achievement list")?
        .clone();
    assert_eq!(achievements.len(), 1);
    assert_eq!(
        achievements[0]["achievement_id"],
        AchievementId::FirstUnsubscribe.as_str()
    );
    assert_eq!(
        achievements[0]["unlocked_at"],
        now.format(&time::format_description::well_known::Rfc3339)?
    );
    Ok(())
}

#[tokio::test]
async fn gm_05_ac2_stats_mail_stopped_total() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        rules: vec![
            w.rule(RuleKind::RejectList, true, Some(u32::MAX)),
            w.rule(RuleKind::BlockPerson, true, Some(u32::MAX)),
        ],
        ..UserState::default()
    })
    .await?;
    assert_eq!(
        w.meter().await?["mail_stopped_per_year"],
        u64::from(u32::MAX) * 2
    );
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_first_unsubscribe_on_first_queued_reject() -> TestResult {
    let w = World::new().await?;
    assert!(!has_unlock(
        &w.state().await?,
        AchievementId::FirstUnsubscribe
    ));
    let result = w.reject(1, &w.session).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    assert_swipe_unlock(&result, AchievementId::FirstUnsubscribe);
    let state = w.state().await?;
    assert_eq!(state.totals.unsubscribes_queued, 1);
    assert!(has_unlock(&state, AchievementId::FirstUnsubscribe));
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_first_filing_category_on_create() -> TestResult {
    let w = World::new().await?;
    create(&w.app, &w.session, "Synthetic category").await?;
    let state = w.state().await?;
    assert_eq!(state.totals.categories_created, 1);
    assert!(has_unlock(&state, AchievementId::FirstFilingCategory));
    create(&w.app, &w.session, "Another synthetic category").await?;
    assert_eq!(
        w.state()
            .await?
            .achievements
            .iter()
            .filter(|a| a.achievement_id == AchievementId::FirstFilingCategory.as_str())
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_first_blocked_person_on_block_rule() -> TestResult {
    let w = World::new().await?;
    let prompt = w
        .seal_prompt(&w.session, "synthetic@example.com", w.fakes.rng.uuid_v4())
        .await?;
    // The unlock is created by the real POST /api/v1/rules route, not a bare
    // service function: this is the product path T-608 wired up.
    let (status, rule) = w.post_block(&prompt).await?;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(rule["kind"], "block_person");
    // A retry of the same prompt through the route reuses the rule and unlocks
    // nothing twice.
    let (again, retry) = w.post_block(&prompt).await?;
    assert_eq!(again, StatusCode::CREATED);
    assert_eq!(retry["rule_id"], rule["rule_id"]);
    let state = w.state().await?;
    assert_eq!(state.rules.len(), 1);
    assert!(has_unlock(&state, AchievementId::FirstBlockedPerson));
    assert_eq!(
        state
            .achievements
            .iter()
            .filter(|a| a.achievement_id == AchievementId::FirstBlockedPerson.as_str())
            .count(),
        1,
        "the achievement unlocks once"
    );
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_year_cleared_on_level_complete() -> TestResult {
    let w = World::new().await?;
    let ceiling = w.fakes.clock.now() - Duration::days(400);
    w.seed(UserState {
        positions: [(
            w.mailbox.0,
            MailboxPosition {
                newest_seen: Some(ceiling),
                new_done: true,
                backlog_ceiling: Some(ceiling),
                session_record_id: Some(w.session.session_record_id.0),
                ..MailboxPosition::default()
            },
        )]
        .into(),
        ..UserState::default()
    })
    .await?;
    api::services::progress::progress(&w.app, &w.session).await?;
    next_page(
        &w.app,
        &w.session,
        FeedRequest {
            cursor: None,
            limit: 20,
            refresh: false,
        },
    )
    .await?;
    let state = w.state().await?;
    assert!(state.totals.years_cleared >= 1);
    assert!(has_unlock(&state, AchievementId::YearCleared));
    let years = state.totals.years_cleared;
    api::services::progress::progress(&w.app, &w.session).await?;
    next_page(
        &w.app,
        &w.session,
        FeedRequest {
            cursor: None,
            limit: 20,
            refresh: false,
        },
    )
    .await?;
    assert_eq!(
        w.state().await?.totals.years_cleared,
        years,
        "empty level must not be counted twice"
    );
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_ten_unsubscribes_in_one_session() -> TestResult {
    let w = World::new().await?;
    for n in 0..9 {
        w.reject(n, &w.session).await?;
    }
    assert!(!has_unlock(
        &w.state().await?,
        AchievementId::TenUnsubscribesInRound
    ));
    let result = w.reject(9, &w.session).await?;
    assert_swipe_unlock(&result, AchievementId::TenUnsubscribesInRound);
    assert_eq!(w.state().await?.totals.round_unsubscribes, 10);
    Ok(())
}

#[tokio::test]
async fn gm_06_ac2_unlock_once_under_retry_and_conflict() -> TestResult {
    let w = World::new().await?;
    let id = w.message(1, true, w.fakes.clock.now())?;
    let key = w.fakes.rng.uuid_v4();
    w.folder.conflicts.store(1, Ordering::SeqCst);
    let writes = w.folder.writes.load(Ordering::SeqCst);
    let first = swipe(
        &w.app,
        &w.session,
        key,
        w.request(&id, ActionDto::Reject, &w.session).await?,
    )
    .await?;
    let retry = swipe(
        &w.app,
        &w.session,
        key,
        w.request(&id, ActionDto::Reject, &w.session).await?,
    )
    .await?;
    assert!(
        w.folder.writes.load(Ordering::SeqCst) >= writes + 2,
        "conflict must exercise write retry"
    );
    assert_eq!(w.folder.conflicts.load(Ordering::SeqCst), 0);
    assert_swipe_unlock(&first, AchievementId::FirstUnsubscribe);
    assert_eq!(
        serde_json::to_value(&first.achievements_unlocked)?,
        serde_json::to_value(&retry.achievements_unlocked)?
    );
    let state = w.state().await?;
    assert_eq!(state.totals.unsubscribes_queued, 1);
    assert_eq!(
        state
            .achievements
            .iter()
            .filter(|a| a.achievement_id == AchievementId::FirstUnsubscribe.as_str())
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn gm_06_ac3_swipe_returns_unlock() -> TestResult {
    let w = World::new().await?;
    let before = w.fakes.clock.now();
    let result = w.reject(1, &w.session).await?;
    assert_swipe_unlock(&result, AchievementId::FirstUnsubscribe);
    assert_eq!(result.achievements_unlocked.len(), 1);
    assert_eq!(result.achievements_unlocked[0].unlocked_at, before);
    let second = w.reject(2, &w.session).await?;
    assert!(second.achievements_unlocked.is_empty());
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_hundred_senders_threshold() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        totals: Totals {
            senders_silenced: 98,
            ..Totals::default()
        },
        ..UserState::default()
    })
    .await?;
    let first = w.reject(1, &w.session).await?;
    assert!(!first
        .achievements_unlocked
        .iter()
        .any(|a| a.achievement_id == AchievementId::SendersSilenced100.as_str()));
    let second = w.reject(2, &w.session).await?;
    assert_swipe_unlock(&second, AchievementId::SendersSilenced100);
    assert!(has_unlock(
        &w.state().await?,
        AchievementId::SendersSilenced100
    ));
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_thousand_cleared_threshold() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        totals: Totals {
            cleared: 998,
            ..Totals::default()
        },
        ..UserState::default()
    })
    .await?;
    w.reject(1, &w.session).await?;
    assert!(!has_unlock(&w.state().await?, AchievementId::Cleared1000));
    let result = w.reject(2, &w.session).await?;
    assert_swipe_unlock(&result, AchievementId::Cleared1000);
    assert!(has_unlock(&w.state().await?, AchievementId::Cleared1000));
    Ok(())
}

#[tokio::test]
async fn gm_06_ac1_round_resets_on_session_change() -> TestResult {
    let w = World::new().await?;
    for n in 0..9 {
        w.reject(n, &w.session).await?;
    }
    let session = AuthedSession {
        user: w.user,
        session_record_id: ports::SessionRecordId(w.fakes.rng.uuid_v4()),
        is_admin: false,
        recent_auth_at: None,
        session_hash: w.session.session_hash,
    };
    let result = w.reject(9, &session).await?;
    assert!(!result
        .achievements_unlocked
        .iter()
        .any(|a| a.achievement_id == AchievementId::TenUnsubscribesInRound.as_str()));
    let state = w.state().await?;
    assert_eq!(state.totals.round_unsubscribes, 1);
    assert_eq!(
        state.totals.round_session,
        Some(session.session_record_id.0)
    );
    Ok(())
}

#[tokio::test]
async fn st_02_ac1_stats_http_is_read_only() -> TestResult {
    let w = World::new().await?;
    w.seed(UserState {
        totals: Totals {
            triaged: 1,
            unsubscribes_queued: 1,
            ..Totals::default()
        },
        ..UserState::default()
    })
    .await?;
    let before = w.folder.read(&w.ctx()).await?;
    let writes = w.folder.writes.load(Ordering::SeqCst);
    let (status, value) = w.get(Some(&w.cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["emails_triaged"], 1);
    assert_eq!(
        w.folder.read(&w.ctx()).await?,
        before,
        "bytes and ETag unchanged"
    );
    assert_eq!(w.folder.writes.load(Ordering::SeqCst), writes);
    assert!(
        w.state().await?.achievements.is_empty(),
        "GET must not evaluate unlocks"
    );
    Ok(())
}

#[tokio::test]
async fn st_02_ac1_stats_missing_file_stays_missing() -> TestResult {
    let w = World::new().await?;
    assert!(w.folder.read(&w.ctx()).await?.is_none());
    let (status, value) = w.get(Some(&w.cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["emails_triaged"], 0);
    assert_eq!(value["mail_stopped_per_year"], 0);
    assert_eq!(value["achievements"], json!([]));
    assert!(w.folder.read(&w.ctx()).await?.is_none());
    assert_eq!(w.folder.writes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn st_02_ac1_stats_http_requires_authentication() -> TestResult {
    let w = World::new().await?;
    assert_eq!(w.get(None).await?.0, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn gm_06_ac3_filing_swipe_returns_category_unlock_on_retry() -> TestResult {
    let w = World::new().await?;
    let message = w.message(500, false, w.fakes.clock.now())?;
    let key = w.fakes.rng.uuid_v4();
    let result = swipe(
        &w.app,
        &w.session,
        key,
        w.request(&message, ActionDto::File, &w.session).await?,
    )
    .await?;
    assert!(result
        .achievements_unlocked
        .iter()
        .any(|a| a.achievement_id == AchievementId::FirstFilingCategory.as_str()));
    let retry = swipe(
        &w.app,
        &w.session,
        key,
        w.request(&message, ActionDto::File, &w.session).await?,
    )
    .await?;
    assert_eq!(
        serde_json::to_value(result.achievements_unlocked)?,
        serde_json::to_value(retry.achievements_unlocked)?
    );
    let state = w.state().await?;
    let value = serde_json::to_value(&state.achievements[0])?;
    assert_eq!(
        value.as_object().ok_or("achievement not an object")?.len(),
        2
    );
    assert!(value.get("achievement_id").is_some());
    assert!(value.get("unlocked_at").is_some());
    Ok(())
}

#[test]
fn gm_06_ac1_legacy_totals_default_levels_cleared() -> TestResult {
    let mut value = serde_json::to_value(Totals::default())?;
    value
        .as_object_mut()
        .ok_or("totals not an object")?
        .remove("levels_cleared");
    let totals: Totals = serde_json::from_value(value)?;
    assert_eq!(totals.levels_cleared, Vec::<i32>::new());
    Ok(())
}
