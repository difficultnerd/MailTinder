//! T-801 service integration tests: ST-01 AC1/AC2 and UN-01 AC3.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use api::routes::feed::{CursorPayload, FeedRequest, Phase};
use api::sealed::{SealedTokens, TokenType, MAX_TOKEN_TTL};
use api::services::feed::next_page;
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::{app_state, build_router, config::ApiConfig, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::user_state::{
    HistoryAction, HistoryEntry, HistoryOutcome, PendingUnsubscribe, UserState,
};
use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, Provider, ProviderSubjectId, RuleId,
    UserId,
};
use obs::Sensitive;
use ports::store::{aad_fields, JobOutcome, JobOutcomeCode};
use ports::{
    Aad, AppFolderStore, Ciphertext, Clock, JobRecord, KeyService, ListKeyHash, MailboxCtx,
    MailboxRecord, Precondition, Rng, ServerStore, SessionHash, UserRecord,
};
use serde_json::{json, Value};
use testkit::{fake_ports, Fakes};
use time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
    session: AuthedSession,
    cookie: String,
}

impl World {
    async fn new() -> TestResult<Self> {
        let (ports, fakes) = fake_ports();
        let config = ApiConfig::new(
            "https://mailtinder.test".into(),
            "fake-client".into(),
            Sensitive::new(b"fake-log-key".to_vec()),
            Sensitive::new(b"fake-email-key".to_vec()),
        )?;
        let app = app_state(Arc::new(ports), Arc::new(config));
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
        let subject = ProviderSubjectId::new("history-test-subject")?;
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
                b"history@example.com",
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
                Sensitive::new("history-refresh".into()),
            )
            .await?;
        fakes
            .identity
            .script_refresh("history-refresh", Ok("history-access".into()));
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
            session_hash: SessionHash([9; 32]),
        };
        Ok(Self {
            app,
            fakes,
            user,
            mailbox,
            session,
            cookie,
        })
    }

    fn ctx(&self) -> MailboxCtx {
        MailboxCtx {
            mailbox: self.mailbox,
            access_token: Sensitive::new("history-access".into()),
        }
    }

    fn entry(&self, id: u128, action: HistoryAction) -> HistoryEntry {
        HistoryEntry {
            entry_id: Uuid::from_u128(id),
            at: self.fakes.clock.now(),
            mailbox_id: self.mailbox,
            sender_display: "Synthetic sender".into(),
            action,
            outcome: HistoryOutcome::Done,
            rule_id: None,
        }
    }

    // Write the encrypted fixture directly: include stale entries that GET must not persistently trim.
    async fn seed(&self, state: &UserState) -> TestResult {
        let wrapped = self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("missing user")?
            .record
            .wrapped_data_key;
        let aad = Aad {
            user: self.user,
            scope: "app_folder".into(),
            field: aad_fields::APP_FOLDER_USER_STATE,
        };
        let bytes = self
            .fakes
            .keys
            .seal(&self.user, &wrapped, &aad, &serde_json::to_vec(state)?)
            .await?;
        self.fakes
            .app_folder
            .write(&self.ctx(), &bytes, None)
            .await?;
        Ok(())
    }

    async fn get(&self, query: &str) -> TestResult<(StatusCode, Value)> {
        let response = build_router(self.app.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/history{query}"))
                    .header("cookie", &self.cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 100_000).await?;
        Ok((status, serde_json::from_slice(&body)?))
    }

    async fn page(&self, query: &str) -> TestResult<Value> {
        let (status, body) = self.get(query).await?;
        assert_eq!(status, StatusCode::OK);
        Ok(body)
    }

    async fn feed_cursor(&self) -> TestResult<String> {
        let wrapped = self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("missing user")?
            .record
            .wrapped_data_key;
        Ok(
            SealedTokens::new(self.app.ports.keys.clone(), self.app.ports.clock.clone())
                .seal(
                    TokenType::Cursor,
                    &self.user,
                    &wrapped,
                    &self.session.session_record_id,
                    self.fakes.clock.now() + MAX_TOKEN_TTL,
                    &CursorPayload {
                        positions: BTreeMap::default(),
                        phase: Phase::New,
                    },
                )
                .await?,
        )
    }
}

#[tokio::test]
async fn st_01_ac1_history_newest_first() -> TestResult {
    let w = World::new().await?;
    let mut oldest = w.entry(9, HistoryAction::Filed);
    oldest.at -= Duration::days(365);
    let mut expired = w.entry(2, HistoryAction::Blocked);
    expired.at -= Duration::days(365) + Duration::seconds(1);
    let mut newest = w.entry(3, HistoryAction::Unsubscribe);
    newest.sender_display = format!("  Synthetic\n\u{202e}{}", "x".repeat(300));
    w.seed(&UserState {
        history: vec![oldest.clone(), expired, newest.clone()],
        ..Default::default()
    })
    .await?;
    let page = w.page("").await?;
    let entries = page["entries"].as_array().ok_or("entries")?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["entry_id"], json!(newest.entry_id));
    assert_eq!(entries[1]["entry_id"], json!(oldest.entry_id));
    assert_eq!(entries[0]["mailbox_id"], json!(w.mailbox.0));
    let at = entries[0]["at"].as_str().ok_or("RFC3339 time")?;
    assert_eq!(
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)?,
        newest.at
    );
    assert_eq!(
        entries[0]["sender_display"],
        format!("Synthetic {}", "x".repeat(246))
    );
    assert!(page["next_cursor"].is_null());
    Ok(())
}

#[tokio::test]
async fn st_01_ac1_filters() -> TestResult {
    let w = World::new().await?;
    let actions = [
        HistoryAction::TrashedByRule,
        HistoryAction::Unsubscribe,
        HistoryAction::Filed,
        HistoryAction::FiledByRule,
        HistoryAction::Blocked,
        HistoryAction::ReportedSpam,
    ];
    let history = actions
        .into_iter()
        .zip(1_u128..)
        .map(|(a, id)| w.entry(id, a))
        .collect();
    w.seed(&UserState {
        history,
        ..Default::default()
    })
    .await?;
    for (filter, expected) in [
        ("all", vec![6, 5, 4, 3, 2, 1]),
        ("unsubscribes", vec![2]),
        ("rule_actions", vec![5, 4, 1]),
        ("filing", vec![4, 3]),
    ] {
        let page = w.page(&format!("?filter={filter}")).await?;
        let ids: Vec<Value> = page["entries"]
            .as_array()
            .ok_or("entries")?
            .iter()
            .map(|e| e["entry_id"].clone())
            .collect();
        assert_eq!(
            ids,
            expected
                .into_iter()
                .map(|id| json!(Uuid::from_u128(id)))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

#[tokio::test]
async fn st_01_ac1_paging_stable() -> TestResult {
    let w = World::new().await?;
    let history = (1..=120)
        .map(|id| w.entry(id, HistoryAction::Filed))
        .collect();
    w.seed(&UserState {
        history,
        ..Default::default()
    })
    .await?;
    assert_eq!(
        w.page("").await?["entries"]
            .as_array()
            .ok_or("entries")?
            .len(),
        20
    );
    let mut ids = Vec::new();
    let mut sizes = Vec::new();
    let mut query = "?limit=50".to_owned();
    loop {
        let page = w.page(&query).await?;
        let entries = page["entries"].as_array().ok_or("entries")?;
        sizes.push(entries.len());
        for entry in entries {
            ids.push(Uuid::parse_str(entry["entry_id"].as_str().ok_or("id")?)?);
        }
        if page["next_cursor"].is_null() {
            break;
        }
        assert!(sizes.len() < 4, "pagination must terminate");
        let cursor = page["next_cursor"].as_str().ok_or("cursor")?;
        let (status, _) = w
            .get(&format!("?filter=unsubscribes&cursor={cursor}"))
            .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        query = format!("?limit=50&cursor={cursor}");
    }
    assert_eq!(sizes, vec![50, 50, 20]);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 120);
    assert_eq!(
        ids,
        (1..=120).rev().map(Uuid::from_u128).collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
async fn st_01_ac2_rule_entries_carry_rule_id() -> TestResult {
    let w = World::new().await?;
    let rule = RuleId::new(Uuid::from_u128(99));
    let history = [
        HistoryAction::TrashedByRule,
        HistoryAction::FiledByRule,
        HistoryAction::Blocked,
    ]
    .into_iter()
    .zip(1_u128..)
    .map(|(action, id)| {
        let mut e = w.entry(id, action);
        e.rule_id = Some(rule);
        e
    })
    .collect();
    w.seed(&UserState {
        history,
        ..Default::default()
    })
    .await?;
    for entry in w.page("?filter=rule_actions").await?["entries"]
        .as_array()
        .ok_or("entries")?
    {
        assert_eq!(entry["rule_id"], json!(rule.0));
    }
    Ok(())
}

#[tokio::test]
async fn un_01_ac3_unsubscribe_outcome_listed() -> TestResult {
    let w = World::new().await?;
    let mut state = UserState::default();
    let cases = [
        (JobStatus::Sent, JobOutcomeCode::OneClickAccepted, "sent"),
        (JobStatus::Failed, JobOutcomeCode::TokenInvalid, "failed"),
        (JobStatus::Cancelled, JobOutcomeCode::Cancelled, "cancelled"),
        (JobStatus::Expired, JobOutcomeCode::Expired, "expired"),
        (
            JobStatus::NeedsAttention,
            JobOutcomeCode::HttpRejected,
            "needs_attention",
        ),
    ];
    for ((status, code, _), n) in cases.into_iter().zip(1_u128..) {
        let job_id = JobId(Uuid::from_u128(n));
        state.pending_unsubscribes.insert(
            job_id.0,
            PendingUnsubscribe {
                mailbox_id: w.mailbox,
                sender_display: "Synthetic list".into(),
                sender_key: format!("list{n}@example.com"),
                list_id: None,
                rule_id: None,
                created_at: w.fakes.clock.now() - Duration::minutes(1),
            },
        );
        w.fakes
            .store
            .jobs()
            .put(
                &JobRecord {
                    job_id,
                    user_id: w.user,
                    mailbox_id: w.mailbox,
                    list_key_hash: ListKeyHash([7; 32]),
                    method: JobMethod::OneClick,
                    target: None,
                    sender_display: None,
                    due_at: w.fakes.clock.now() - Duration::seconds(1),
                    status,
                    attempts: 1,
                    outcome: Some(JobOutcome {
                        code,
                        at: w.fakes.clock.now(),
                    }),
                    expires_at: w.fakes.clock.now() + Duration::days(1),
                },
                Precondition::MustNotExist,
            )
            .await?;
    }
    w.seed(&state).await?;
    assert_eq!(
        w.page("?filter=unsubscribes").await?["entries"]
            .as_array()
            .ok_or("entries")?
            .len(),
        0
    );
    next_page(
        &w.app,
        &w.session,
        FeedRequest {
            cursor: None,
            limit: 50,
            refresh: true,
        },
    )
    .await?;
    let page = w.page("?filter=unsubscribes").await?;
    let entries = page["entries"].as_array().ok_or("entries")?;
    assert_eq!(entries.len(), cases.len());
    let outcomes = entries
        .iter()
        .map(|e| e["outcome"].as_str().ok_or("outcome"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        outcomes,
        cases.into_iter().map(|(_, _, outcome)| outcome).collect()
    );
    for entry in entries {
        assert_eq!(entry["action"], "unsubscribe");
        assert_eq!(entry["sender_display"], "Synthetic list");
        assert_eq!(entry["mailbox_id"], json!(w.mailbox.0));
    }
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_2_feed_cursor_as_history_cursor_refused() -> TestResult {
    let w = World::new().await?;
    let cursor = w.feed_cursor().await?;
    let (status, body) = w.get(&format!("?cursor={cursor}")).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_request");
    Ok(())
}

#[tokio::test]
async fn history_get_writes_nothing() -> TestResult {
    let w = World::new().await?;
    assert_eq!(
        w.page("").await?["entries"]
            .as_array()
            .ok_or("entries")?
            .len(),
        0
    );
    assert!(w.fakes.app_folder.read(&w.ctx()).await?.is_none());
    let mut stale = w.entry(1, HistoryAction::Filed);
    stale.at -= Duration::days(366);
    w.seed(&UserState {
        history: vec![stale, w.entry(2, HistoryAction::Filed)],
        ..Default::default()
    })
    .await?;
    let before = w.fakes.app_folder.read(&w.ctx()).await?;
    w.fakes
        .app_folder
        .fail_op(testkit::app_folder::FolderOp::Write, 1);
    assert_eq!(
        w.page("").await?["entries"]
            .as_array()
            .ok_or("entries")?
            .len(),
        1
    );
    assert_eq!(w.fakes.app_folder.read(&w.ctx()).await?, before);
    Ok(())
}

#[tokio::test]
async fn st_01_ac1_invalid_queries_refused() -> TestResult {
    let w = World::new().await?;
    for query in [
        "?limit=0",
        "?limit=51",
        "?limit=-1",
        "?limit=no",
        "?filter=unknown",
        "?unknown=yes",
        "?cursor=invalid",
    ] {
        let (status, body) = w.get(query).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
    }
    Ok(())
}
