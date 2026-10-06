//! T-803 account deletion integration tests (S2 AU-06 AC1-AC3, S5 DEL-1, DEL-2,
//! ASVS V7.4.2, S5 SES-1).
//!
//! The route runs against the real router with the in-memory store and keys,
//! the recording scheduler, the scripted identity provider, the virtual clock
//! and the in-memory app folder. The order test wraps the folder, scheduler,
//! identity provider and the user and session repos so that every call lands
//! on one shared sequence.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc
)]

use std::sync::{Arc, Mutex};

use api::auth::step_up::require_step_up;
use api::services::account_deletion::{delete_account, DeletionStep};
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::{
    EmailAddress, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason,
    Provider, ProviderSubjectId, UserId,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AppFolderError, AppFolderStore, AuthRequest, BakeoffSnapshotRepo, CancelOutcome,
    Ciphertext, ClassifierEvalRepo, Clock, ConfigRepo, ETag, IdClaims, IdError, IdentityProvider,
    InviteRepo, InviteRequestRepo, JobRecord, JobRepo, JobScheduler, KeyService, ListKeyHash,
    MailError, MailboxCtx, MailboxRecord, MailboxRepo, NeedsAttentionId, NeedsAttentionRecord,
    NeedsAttentionRepo, Page, PageRequest, Precondition, RateLimitRepo, Repo, Rng, SchedError,
    ServerStore, SessionHash, SessionRecord, SessionRecordId, SessionRepo, StoreError, TaskName,
    TokenSet, UserRecord, UserRepo, Version, Versioned,
};
use testkit::app_folder::FolderOp;
use testkit::{fake_ports, Fakes, InMemoryServerStore, SchedulerEvent, SeedMessage};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
/// The 30-day terminal retention (`S3` terminal job).
const RETAIN: Duration = Duration::days(30);

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn fixture() -> Result<(AppState, Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = fake_ports();
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes))
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn seed_user(fakes: &Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
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
    Ok(user)
}

async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
    sub: &str,
    email: &str,
    is_primary: bool,
    status: MailboxStatus,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new(sub)?;
    let address = EmailAddress::parse(email)?;
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
                email_address: Ciphertext(sealed),
                status,
                linked_at: fakes.clock.now(),
                is_primary,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

/// A stored, sealed refresh token plus the scripted refresh it mints from.
async fn grant(
    state: &AppState,
    fakes: &Fakes,
    user: &UserId,
    mailbox: &MailboxId,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    state
        .tokens
        .store_refresh_token(state, user, mailbox, Sensitive::new(token.to_owned()))
        .await?;
    fakes
        .identity
        .script_refresh(token, Ok(format!("access-for-{token}")));
    Ok(())
}

/// The primary mailbox, then a second (connected) one linked a second later.
async fn seed_pair(
    state: &AppState,
    fakes: &Fakes,
    user: UserId,
) -> Result<(MailboxId, MailboxId), Box<dyn std::error::Error>> {
    let primary = seed_mailbox(
        fakes,
        user,
        "sub-primary",
        "primary@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    grant(state, fakes, &user, &primary, "refresh-primary").await?;
    fakes.clock.advance(Duration::seconds(1));
    let other = seed_mailbox(
        fakes,
        user,
        "sub-other",
        "other@example.com",
        false,
        MailboxStatus::Connected,
    )
    .await?;
    grant(state, fakes, &user, &other, "refresh-other").await?;
    Ok((primary, other))
}

async fn seed_job(
    fakes: &Fakes,
    user: UserId,
    mailbox: MailboxId,
    status: JobStatus,
) -> Result<JobId, Box<dyn std::error::Error>> {
    let job_id = JobId::new(fakes.rng.uuid_v4());
    let now = fakes.clock.now();
    fakes
        .store
        .jobs()
        .put(
            &JobRecord {
                job_id,
                user_id: user,
                mailbox_id: mailbox,
                list_key_hash: ListKeyHash([7u8; 32]),
                method: JobMethod::OneClick,
                target: Some(Ciphertext(vec![1, 2, 3])),
                due_at: now + Duration::minutes(5),
                status,
                attempts: 0,
                outcome: None,
                expires_at: now + Duration::hours(1),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(job_id)
}

async fn seed_card(
    fakes: &Fakes,
    user: UserId,
    mailbox: MailboxId,
) -> Result<NeedsAttentionId, Box<dyn std::error::Error>> {
    let item_id = NeedsAttentionId(fakes.rng.uuid_v4());
    let now = fakes.clock.now();
    fakes
        .store
        .needs_attention()
        .put(
            &NeedsAttentionRecord {
                item_id,
                user_id: user,
                mailbox_id: mailbox,
                sender_display: Ciphertext(vec![9, 9]),
                link: None,
                reason_code: NeedsAttentionReason::HttpsOnlyUnsubscribe,
                created_at: now,
                expires_at: now + RETAIN,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(item_id)
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

struct Authed {
    cookie: String,
    csrf: String,
    user: UserId,
}

/// An authenticated session; `stale` sets `recent_auth_at` that far in the past.
async fn authed(
    state: &AppState,
    fakes: &Fakes,
    stale: Option<Duration>,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes).await?;
    // Fresh by default (a sign-in 10 s ago), or `stale` seconds ago.
    let recent = Some(fakes.clock.now() - stale.unwrap_or(Duration::seconds(10)));
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, recent)
        .await?;
    let cookie = cookie
        .0
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok(Authed {
        cookie,
        csrf: record.record.csrf_token.clone(),
        user,
    })
}

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The `code` value from a problem-details body.
async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok(body["code"].as_str().unwrap_or_default().to_owned())
}

fn mctx(mailbox: &MailboxId, token: &str) -> MailboxCtx {
    MailboxCtx {
        mailbox: *mailbox,
        access_token: Sensitive::new(token.to_owned()),
    }
}

async fn folder_bytes(fakes: &Fakes, mailbox: &MailboxId) -> Option<Vec<u8>> {
    fakes
        .app_folder
        .read(&mctx(mailbox, "access-read"))
        .await
        .ok()
        .flatten()
        .map(|(bytes, _)| bytes)
}

/// A CSRF-checked `DELETE /account`.
async fn delete_request(
    router: &Router,
    a: &Authed,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::DELETE)
        .uri("/api/v1/account")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, a.cookie.as_str())
        .header("x-csrf-token", a.csrf.as_str())
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

/// A `GET /mailboxes` with the given cookie, for the status it answers.
async fn mailboxes_status(
    router: &Router,
    cookie: &str,
) -> Result<StatusCode, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/mailboxes")
        .header(header::COOKIE, cookie)
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?.status())
}

/// A user with two connected mailboxes, a queued job with its task on the
/// primary, a Needs Attention item and an app folder file on each mailbox.
struct World {
    a: Authed,
    primary: MailboxId,
    other: MailboxId,
    job: JobId,
}

async fn world(state: &AppState, fakes: &Fakes) -> Result<World, Box<dyn std::error::Error>> {
    let a = authed(state, fakes, None).await?;
    let (primary, other) = seed_pair(state, fakes, a.user).await?;
    let job = seed_job(fakes, a.user, primary, JobStatus::Queued).await?;
    fakes
        .scheduler
        .schedule(&job, fakes.clock.now() + Duration::minutes(5))
        .await?;
    seed_card(fakes, a.user, primary).await?;
    fakes
        .app_folder
        .write(&mctx(&primary, "seed"), b"state-primary", None)
        .await?;
    fakes
        .app_folder
        .write(&mctx(&other, "seed"), b"state-stale", None)
        .await?;
    Ok(World {
        a,
        primary,
        other,
        job,
    })
}

// ---------------------------------------------------------------------------
// One shared sequence: each wrapper logs the call, then delegates to the fake.
// ---------------------------------------------------------------------------

type Trace = Arc<Mutex<Vec<String>>>;

fn note(trace: &Trace, entry: String) {
    trace.lock().unwrap().push(entry);
}

struct TraceFolder {
    inner: Arc<dyn AppFolderStore>,
    trace: Trace,
}

#[async_trait]
impl AppFolderStore for TraceFolder {
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        self.inner.read(mb).await
    }

    async fn write(
        &self,
        mb: &MailboxCtx,
        bytes: &[u8],
        if_match: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        self.inner.write(mb, bytes, if_match).await
    }

    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError> {
        note(&self.trace, format!("folder_delete:{}", mb.mailbox.0));
        self.inner.delete(mb).await
    }
}

struct TraceScheduler {
    inner: Arc<dyn JobScheduler>,
    trace: Trace,
}

#[async_trait]
impl JobScheduler for TraceScheduler {
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError> {
        self.inner.schedule(job, due_at).await
    }

    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError> {
        note(&self.trace, "task_cancel".to_owned());
        self.inner.cancel(task).await
    }
}

struct TraceIdentity {
    inner: Arc<dyn IdentityProvider>,
    trace: Trace,
}

#[async_trait]
impl IdentityProvider for TraceIdentity {
    fn authorize_url(&self, req: &AuthRequest) -> Url {
        self.inner.authorize_url(req)
    }

    async fn exchange(
        &self,
        code: &str,
        verifier: &Sensitive<String>,
        redirect_uri: &Url,
    ) -> Result<TokenSet, IdError> {
        self.inner.exchange(code, verifier, redirect_uri).await
    }

    async fn validate_id_token(
        &self,
        raw: &Sensitive<String>,
        nonce: &str,
    ) -> Result<IdClaims, IdError> {
        self.inner.validate_id_token(raw, nonce).await
    }

    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError> {
        self.inner.refresh(refresh).await
    }

    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError> {
        note(&self.trace, format!("revoke:{}", token.expose()));
        self.inner.revoke(token).await
    }
}

/// The user repo, logging the delete that destroys the key.
struct TraceUsers {
    inner: Arc<InMemoryServerStore>,
    trace: Trace,
}

#[async_trait]
impl Repo<UserId, UserRecord> for TraceUsers {
    async fn get(&self, key: &UserId) -> Result<Option<Versioned<UserRecord>>, StoreError> {
        self.inner.users().get(key).await
    }

    async fn put(&self, record: &UserRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.inner.users().put(record, pre).await
    }

    async fn delete(&self, key: &UserId, pre: Precondition) -> Result<(), StoreError> {
        note(&self.trace, "user_delete".to_owned());
        self.inner.users().delete(key, pre).await
    }
}

#[async_trait]
impl UserRepo for TraceUsers {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<UserRecord>>, StoreError> {
        self.inner.users().list(page).await
    }
}

/// The session repo, logging the bulk delete that ends the user's sessions.
struct TraceSessions {
    inner: Arc<InMemoryServerStore>,
    trace: Trace,
}

#[async_trait]
impl Repo<SessionHash, SessionRecord> for TraceSessions {
    async fn get(&self, key: &SessionHash) -> Result<Option<Versioned<SessionRecord>>, StoreError> {
        self.inner.sessions().get(key).await
    }

    async fn put(&self, record: &SessionRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.inner.sessions().put(record, pre).await
    }

    async fn delete(&self, key: &SessionHash, pre: Precondition) -> Result<(), StoreError> {
        self.inner.sessions().delete(key, pre).await
    }
}

#[async_trait]
impl SessionRepo for TraceSessions {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<SessionRecord>>, StoreError> {
        self.inner.sessions().by_user(user).await
    }

    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<SessionHash>, StoreError> {
        self.inner.sessions().expires_by(now, page).await
    }

    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        note(&self.trace, "sessions_delete".to_owned());
        self.inner.sessions().delete_all_for_user(user).await
    }
}

struct TraceStore {
    inner: Arc<InMemoryServerStore>,
    users: TraceUsers,
    sessions: TraceSessions,
}

#[async_trait]
impl ServerStore for TraceStore {
    fn users(&self) -> &dyn UserRepo {
        &self.users
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        self.inner.mailboxes()
    }
    fn invites(&self) -> &dyn InviteRepo {
        self.inner.invites()
    }
    fn invite_requests(&self) -> &dyn InviteRequestRepo {
        self.inner.invite_requests()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self.inner.jobs()
    }
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo {
        self.inner.needs_attention()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        &self.sessions
    }
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo {
        self.inner.classifier_eval()
    }
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo {
        self.inner.bakeoff_snapshots()
    }
    fn config(&self) -> &dyn ConfigRepo {
        self.inner.config()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.inner.rate_limits()
    }
}

/// A fixture whose folder, scheduler, identity provider, user repo and session
/// repo all log onto one sequence.
fn traced_fixture() -> Result<(AppState, Fakes, Trace), Box<dyn std::error::Error>> {
    let (mut ports, fakes) = fake_ports();
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    ports.app_folder = Arc::new(TraceFolder {
        inner: Arc::clone(&fakes.app_folder) as Arc<dyn AppFolderStore>,
        trace: Arc::clone(&trace),
    });
    ports.scheduler = Arc::new(TraceScheduler {
        inner: Arc::clone(&fakes.scheduler) as Arc<dyn JobScheduler>,
        trace: Arc::clone(&trace),
    });
    ports.identity = Arc::new(TraceIdentity {
        inner: Arc::clone(&fakes.identity) as Arc<dyn IdentityProvider>,
        trace: Arc::clone(&trace),
    });
    ports.store = Arc::new(TraceStore {
        inner: Arc::clone(&fakes.store),
        users: TraceUsers {
            inner: Arc::clone(&fakes.store),
            trace: Arc::clone(&trace),
        },
        sessions: TraceSessions {
            inner: Arc::clone(&fakes.store),
            trace: Arc::clone(&trace),
        },
    });
    Ok((
        app_state(Arc::new(ports), Arc::new(config()?)),
        fakes,
        trace,
    ))
}

// ---------------------------------------------------------------------------
// AU-06 AC1: app folder, then task cancels, then revokes, then the key, then
// the records and sessions, on one shared sequence.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_06_ac1_deletion_order() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes, trace) = traced_fixture()?;
    let w = world(&state, &fakes).await?;
    let session = AuthedSession {
        user: w.a.user,
        session_record_id: SessionRecordId(fakes.rng.uuid_v4()),
        is_admin: false,
        recent_auth_at: Some(fakes.clock.now() - Duration::seconds(10)),
        session_hash: SessionHash([0u8; 32]),
    };
    require_step_up(&session, fakes.clock.as_ref())?;
    trace.lock().unwrap().clear();

    let (accepted, steps) = delete_account(&state, &session).await?;

    assert_eq!(
        steps,
        vec![
            DeletionStep::AppFolderDeleted,
            DeletionStep::JobsCancelled,
            DeletionStep::TokensRevoked,
            DeletionStep::KeyDestroyed,
            DeletionStep::RecordsDeleted,
            DeletionStep::SessionsEnded,
        ]
    );
    assert_eq!(
        *trace.lock().unwrap(),
        vec![
            format!("folder_delete:{}", w.primary.0),
            format!("folder_delete:{}", w.other.0),
            "task_cancel".to_owned(),
            "revoke:refresh-primary".to_owned(),
            "revoke:refresh-other".to_owned(),
            "user_delete".to_owned(),
            "sessions_delete".to_owned(),
        ],
        "app folder, task cancels, revokes, key, then sessions, in that order"
    );
    assert_eq!(
        cancels(&fakes),
        vec![TaskName::for_job(&w.job)],
        "the queued job's task was cancelled by name"
    );
    assert!(accepted.app_folders_not_deleted.is_empty());
    assert_eq!(
        accepted.deletion_due_by,
        fakes.clock.now() + Duration::hours(24)
    );
    assert!(fakes.store.users().get(&w.a.user).await?.is_none());
    assert_eq!(fakes.store.mailboxes().by_user(&w.a.user).await?.len(), 0);
    assert!(fakes.store.jobs().get(&w.job).await?.is_none());
    Ok(())
}

fn cancels(fakes: &Fakes) -> Vec<TaskName> {
    fakes
        .scheduler
        .events()
        .into_iter()
        .filter_map(|e| match e {
            SchedulerEvent::Cancelled(name, _) => Some(name),
            SchedulerEvent::Scheduled(..) => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// AU-06 AC1: an unreachable Drive never stops the deletion; the mailbox is
// listed so the user can remove the file by hand.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_06_ac1_provider_unreachable_still_deletes() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;
    // The primary's delete is the first call, so it is the one that fails.
    fakes.app_folder.fail_op(FolderOp::Delete, 1);

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    assert_eq!(
        body["app_folders_not_deleted"],
        serde_json::json!([{
            "mailbox_id": w.primary.0.to_string(),
            "email_address": "primary@example.com",
        }]),
        "only the unreachable mailbox is listed, with its address"
    );
    assert!(body["deletion_due_by"].is_string());

    assert_eq!(
        folder_bytes(&fakes, &w.primary).await,
        Some(b"state-primary".to_vec()),
        "the file that could not be deleted is still there"
    );
    assert_eq!(folder_bytes(&fakes, &w.other).await, None);
    assert_eq!(
        fakes.identity.revoked(),
        vec!["refresh-primary".to_owned(), "refresh-other".to_owned()],
        "every grant is revoked anyway"
    );
    assert!(fakes.store.users().get(&w.a.user).await?.is_none());
    assert_eq!(fakes.store.mailboxes().by_user(&w.a.user).await?.len(), 0);
    assert!(fakes.store.jobs().get(&w.job).await?.is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-06 AC1: a queued job never sends after deletion.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_06_ac1_queued_job_never_sends_after_deletion() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    fakes.clock.advance(Duration::minutes(10));
    assert!(
        fakes.scheduler.due(fakes.clock.now()).is_empty(),
        "no task is left to deliver"
    );
    assert!(fakes.store.jobs().get(&w.job).await?.is_none());
    assert!(fakes.mailbox.sent().is_empty(), "nothing was sent");
    assert!(fakes.egress.records().is_empty(), "no request went out");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-06 AC2: labels already applied stay on the user's messages.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_06_ac2_labels_remain() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;
    let id = fakes.mailbox.seed(
        &w.primary,
        SeedMessage {
            from_display: "Sender".to_owned(),
            from_address: "sender@example.com".to_owned(),
            subject: "Subject".to_owned(),
            raw_headers: vec![],
            facts: domain::HeaderFacts::default(),
            preview_text: "Preview".to_owned(),
            internal_date: fakes.clock.now(),
            labels: vec!["INBOX".to_owned(), "Label_filed".to_owned()],
        },
    );
    let before = fakes.mailbox.labels_of(&w.primary, &id).ok_or("labels")?;

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    assert_eq!(
        fakes.mailbox.labels_of(&w.primary, &id),
        Some(before),
        "the filed message keeps its labels"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-06 AC3: without a fresh step-up nothing is deleted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_06_ac3_deletion_needs_step_up() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    // 5 minutes and 1 second old: outside the window (T-504).
    let a = authed(&state, &fakes, Some(Duration::seconds(301))).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;
    let job = seed_job(&fakes, a.user, primary, JobStatus::Queued).await?;
    fakes
        .scheduler
        .schedule(&job, fakes.clock.now() + Duration::minutes(5))
        .await?;
    fakes
        .app_folder
        .write(&mctx(&primary, "seed"), b"state-bytes", None)
        .await?;

    let resp = delete_request(&router, &a).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");

    assert!(fakes.store.users().get(&a.user).await?.is_some());
    assert!(fakes.store.mailboxes().get(&primary).await?.is_some());
    assert!(fakes.store.mailboxes().get(&other).await?.is_some());
    assert_eq!(
        fakes
            .store
            .jobs()
            .get(&job)
            .await?
            .ok_or("job")?
            .record
            .status,
        JobStatus::Queued
    );
    assert_eq!(cancels(&fakes), Vec::<TaskName>::new());
    assert_eq!(fakes.identity.revoked(), Vec::<String>::new());
    assert_eq!(
        folder_bytes(&fakes, &primary).await,
        Some(b"state-bytes".to_vec())
    );
    assert_eq!(mailboxes_status(&router, &a.cookie).await?, StatusCode::OK);
    Ok(())
}

// ---------------------------------------------------------------------------
// DEL-1: after deletion no document references the user ID.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn del_1_no_document_references_user() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;
    let bystander = seed_user(&fakes).await?;
    let before = serde_json::to_string(&fakes.store.export_json())?;
    assert!(
        before.contains(&w.a.user.0.to_string()),
        "the scan sees the user before deletion"
    );

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let after = serde_json::to_string(&fakes.store.export_json())?;
    assert!(
        !after.contains(&w.a.user.0.to_string()),
        "no collection mentions the deleted user: {after}"
    );
    assert!(
        after.contains(&bystander.0.to_string()),
        "another user's records are untouched"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// DEL-2: a copy of an encrypted field taken before deletion cannot be opened.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn del_2_old_ciphertext_unreadable_after_deletion() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;
    let copy = fakes
        .store
        .mailboxes()
        .get(&w.primary)
        .await?
        .ok_or("mailbox")?
        .record
        .refresh_token
        .ok_or("sealed refresh token")?;
    let aad = Aad {
        user: w.a.user,
        scope: w.primary.0.to_string(),
        field: aad_fields::MAILBOX_REFRESH_TOKEN,
    };
    let stranger = seed_user(&fakes).await?;
    let stranger_key = fakes
        .store
        .users()
        .get(&stranger)
        .await?
        .ok_or("stranger")?
        .record
        .wrapped_data_key;

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    assert!(
        fakes.store.users().get(&w.a.user).await?.is_none(),
        "the wrapped key is gone, so there is nothing to open the copy with"
    );
    assert!(
        fakes
            .keys
            .open(&w.a.user, &stranger_key, &aad, &copy.0)
            .await
            .is_err(),
        "another user's key does not open it"
    );
    assert!(
        fakes
            .keys
            .open(&stranger, &stranger_key, &aad, &copy.0)
            .await
            .is_err(),
        "nor does it under another user's identity"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V7.4.2: deleting the account ends the user's session.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_4_2_deletion_ends_session() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let w = world(&state, &fakes).await?;
    assert_eq!(
        mailboxes_status(&router, &w.a.cookie).await?,
        StatusCode::OK
    );

    let resp = delete_request(&router, &w.a).await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let cleared = resp
        .headers()
        .get(header::SET_COOKIE)
        .ok_or("set-cookie")?
        .to_str()?
        .to_owned();
    assert!(
        cleared.contains("Max-Age=0") || cleared.contains("1970"),
        "the cookie is cleared: {cleared}"
    );
    assert_eq!(
        resp.headers()
            .get("clear-site-data")
            .ok_or("clear-site-data")?,
        "\"cache\", \"storage\""
    );

    assert_eq!(fakes.store.sessions().by_user(&w.a.user).await?.len(), 0);
    assert_eq!(
        mailboxes_status(&router, &w.a.cookie).await?,
        StatusCode::UNAUTHORIZED,
        "the old cookie is refused"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SES-1: a session whose user is gone is refused, whatever the session says.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn ses_1_deleted_users_session_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    assert_eq!(mailboxes_status(&router, &a.cookie).await?, StatusCode::OK);

    // Only the user record goes; the session record is still stored.
    fakes
        .store
        .users()
        .delete(&a.user, Precondition::None)
        .await?;
    assert_eq!(fakes.store.sessions().by_user(&a.user).await?.len(), 1);

    assert_eq!(
        mailboxes_status(&router, &a.cookie).await?,
        StatusCode::UNAUTHORIZED,
        "the session is refused"
    );
    assert_eq!(
        fakes.store.sessions().by_user(&a.user).await?.len(),
        0,
        "and its record is purged"
    );
    let resp = delete_request(&router, &a).await?;
    assert!(
        resp.status().is_client_error(),
        "the old cookie cannot delete anything"
    );
    Ok(())
}
