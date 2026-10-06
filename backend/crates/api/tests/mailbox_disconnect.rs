//! T-601b mailbox disconnect integration tests (S2 AU-05 AC1-AC4, S5 DEL-3,
//! S3 INV-2, ASVS V8.2.2, S10 6.3 undo racing the run, SW-05 AC5).
//!
//! The service and the route both run against the real router with the
//! in-memory store and keys, the recording scheduler, the scripted identity
//! provider, the virtual clock and the in-memory app folder.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::missing_panics_doc
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use api::services::jobs::{cancel_queued_job, CancelResult};
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
    Aad, AppFolderStore, BakeoffSnapshotRepo, Ciphertext, ClassifierEvalRepo, Clock, ConfigRepo,
    InviteRepo, InviteRequestRepo, JobOutcome, JobOutcomeCode, JobRecord, JobRepo, JobScheduler,
    KeyService, ListKeyHash, MailboxCtx, MailboxRecord, MailboxRepo, NeedsAttentionId,
    NeedsAttentionRecord, NeedsAttentionRepo, Page, PageRequest, Precondition, RateLimitRepo, Repo,
    Rng, ServerStore, SessionRepo, StoreError, TaskName, UserRecord, UserRepo, Version, Versioned,
};
use testkit::app_folder::FolderOp;
use testkit::{fake_ports, Fakes, InMemoryServerStore, SchedulerEvent};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

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

/// A CSRF-checked `DELETE /mailboxes/{id}`.
async fn disconnect(
    router: &Router,
    a: &Authed,
    mailbox: &MailboxId,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/v1/mailboxes/{}", mailbox.0))
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, a.cookie.as_str())
        .header("x-csrf-token", a.csrf.as_str())
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

async fn list_mailboxes(
    router: &Router,
    cookie: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/mailboxes")
        .header(header::COOKIE, cookie)
        .body(Body::empty())?;
    let resp = router.clone().oneshot(req).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok(json["mailboxes"]
        .as_array()
        .ok_or("mailboxes array")?
        .iter()
        .map(|m| m["email_address"].as_str().unwrap_or_default().to_owned())
        .collect())
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
// AU-05 AC1: disconnect revokes the grant at Google and deletes the document.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac1_disconnect_revokes_and_deletes() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fakes.identity.revoked(),
        vec!["refresh-primary".to_owned()],
        "the grant is revoked at the provider"
    );
    assert!(
        fakes.store.mailboxes().get(&primary).await?.is_none(),
        "the document is gone"
    );
    assert!(
        fakes.store.mailboxes().get(&other).await?.is_some(),
        "the other mailbox stays"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC1: queued jobs are cancelled and their Cloud Tasks deleted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac1_disconnect_cancels_queued_jobs_and_tasks(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, _other) = seed_pair(&state, &fakes, a.user).await?;
    let job = seed_job(&fakes, a.user, primary, JobStatus::Queued).await?;
    let due = fakes.clock.now() + Duration::minutes(5);
    fakes.scheduler.schedule(&job, due).await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let stored = fakes.store.jobs().get(&job).await?.ok_or("job")?;
    assert_eq!(stored.record.status, JobStatus::Cancelled);
    assert_eq!(
        cancels(&fakes),
        vec![TaskName::for_job(&job)],
        "the task is cancelled by name"
    );
    fakes.clock.advance(Duration::minutes(10));
    assert!(
        fakes.scheduler.due(fakes.clock.now()).is_empty(),
        "nothing is left to deliver"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC1: the mailbox's cards leave the Feed (API-MBX-1 until T-602c).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac1_disconnected_mailbox_cards_leave_feed() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;
    let card = seed_card(&fakes, a.user, primary).await?;
    let keep = seed_card(&fakes, a.user, other).await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert!(
        fakes.store.needs_attention().get(&card).await?.is_none(),
        "the disconnected mailbox's cards are deleted"
    );
    assert!(
        fakes.store.needs_attention().get(&keep).await?.is_some(),
        "the other mailbox's cards stay"
    );
    let listed = list_mailboxes(&router, &a.cookie).await?;
    assert_eq!(listed, vec!["other@example.com".to_owned()]);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC1: a job the runner already claimed is left alone.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac1_running_job_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, _other) = seed_pair(&state, &fakes, a.user).await?;
    let job = seed_job(&fakes, a.user, primary, JobStatus::Running).await?;
    let due = fakes.clock.now() + Duration::minutes(5);
    fakes.scheduler.schedule(&job, due).await?;
    fakes.scheduler.start(&TaskName::for_job(&job));

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let stored = fakes.store.jobs().get(&job).await?.ok_or("job")?;
    assert_eq!(stored.record.status, JobStatus::Running, "untouched");
    assert!(
        cancels(&fakes).is_empty(),
        "a running job's task is never deleted under it"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC2: the only mailbox cannot be disconnected.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac2_last_mailbox_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let only = seed_mailbox(
        &fakes,
        a.user,
        "sub-only",
        "only@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    grant(&state, &fakes, &a.user, &only, "refresh-only").await?;

    let resp = disconnect(&router, &a, &only).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "last_mailbox");
    assert!(fakes.store.mailboxes().get(&only).await?.is_some());
    assert!(
        fakes.identity.revoked().is_empty(),
        "nothing is revoked or deleted"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC3: without a fresh step-up nothing changes.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac3_disconnect_needs_step_up() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    // 5 minutes and 1 second old: outside the window (T-504).
    let a = authed(&state, &fakes, Some(Duration::seconds(301))).await?;
    let (primary, _other) = seed_pair(&state, &fakes, a.user).await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert!(fakes.store.mailboxes().get(&primary).await?.is_some());
    assert_eq!(fakes.identity.revoked(), Vec::<String>::new());
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC4: the primary's app folder file is copied, then the flag moves.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac4_primary_move_copies_file_then_flips_primary(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;
    fakes
        .app_folder
        .write(&mctx(&primary, "seed"), b"state-bytes", None)
        .await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        folder_bytes(&fakes, &other).await,
        Some(b"state-bytes".to_vec()),
        "the new Drive holds the same bytes"
    );
    assert_eq!(
        folder_bytes(&fakes, &primary).await,
        None,
        "the old copy is deleted"
    );
    let promoted = fakes
        .store
        .mailboxes()
        .get(&other)
        .await?
        .ok_or("other mailbox")?;
    assert!(promoted.record.is_primary, "the next mailbox is primary");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC4: a failed copy disconnects nothing.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac4_move_failure_disconnects_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;
    fakes
        .app_folder
        .write(&mctx(&primary, "seed"), b"state-bytes", None)
        .await?;
    fakes.app_folder.fail_op(FolderOp::Write, 1);

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "app_folder_move_failed");
    assert!(
        fakes.identity.revoked().is_empty(),
        "the grant is not revoked after a failed move"
    );
    let old = fakes
        .store
        .mailboxes()
        .get(&primary)
        .await?
        .ok_or("primary")?;
    assert!(old.record.is_primary, "still primary");
    assert_eq!(
        old.record.status,
        MailboxStatus::Connected,
        "still connected"
    );
    let next = fakes.store.mailboxes().get(&other).await?.ok_or("other")?;
    assert!(!next.record.is_primary, "the next mailbox is not promoted");
    assert_eq!(
        folder_bytes(&fakes, &primary).await,
        Some(b"state-bytes".to_vec()),
        "the old copy is untouched"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC4: a failed delete of the old copy rolls the move back.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_05_ac4_old_copy_delete_failure_rolls_back() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, other) = seed_pair(&state, &fakes, a.user).await?;
    fakes
        .app_folder
        .write(&mctx(&primary, "seed"), b"state-bytes", None)
        .await?;
    fakes.app_folder.fail_op(FolderOp::Delete, 1);

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "app_folder_move_failed");
    assert_eq!(fakes.identity.revoked(), Vec::<String>::new());
    let old = fakes
        .store
        .mailboxes()
        .get(&primary)
        .await?
        .ok_or("primary")?;
    assert!(old.record.is_primary, "the old mailbox is primary again");
    let next = fakes.store.mailboxes().get(&other).await?.ok_or("other")?;
    assert!(!next.record.is_primary, "the flag is not left behind");
    assert_eq!(
        folder_bytes(&fakes, &primary).await,
        Some(b"state-bytes".to_vec()),
        "the file is still where the user left it"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// DEL-3: revoke at the provider, then remove the document.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn del_3_disconnect_revokes_and_removes_document() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (primary, _other) = seed_pair(&state, &fakes, a.user).await?;

    let resp = disconnect(&router, &a, &primary).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(fakes.identity.revoked(), vec!["refresh-primary".to_owned()]);
    let dumped = serde_json::to_string(&fakes.store.export_json())?;
    assert!(
        !dumped.contains(&primary.0.to_string()),
        "no document for the disconnected mailbox survives"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.2: another user's mailbox is a flat 404.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v8_2_2_disconnect_other_users_mailbox_not_found(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (_mine, _mine2) = seed_pair(&state, &fakes, a.user).await?;
    let stranger = seed_user(&fakes).await?;
    let foreign = seed_mailbox(
        &fakes,
        stranger,
        "sub-foreign",
        "foreign@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    grant(&state, &fakes, &stranger, &foreign, "refresh-foreign").await?;

    let resp = disconnect(&router, &a, &foreign).await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(code_of(resp).await?, "not_found");
    assert!(
        fakes.store.mailboxes().get(&foreign).await?.is_some(),
        "the stranger's mailbox is untouched"
    );
    assert_eq!(fakes.identity.revoked(), Vec::<String>::new());
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-2: the cancelled record keeps the outcome, drops the target and carries
// the terminal 30-day retention.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_2_cancelled_job_keeps_outcome_and_expiry() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        user,
        "sub-job",
        "job@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let job = seed_job(&fakes, user, mailbox, JobStatus::Queued).await?;
    let now = fakes.clock.now();

    assert_eq!(
        cancel_queued_job(&state, &job).await?,
        CancelResult::Cancelled
    );
    let stored = fakes.store.jobs().get(&job).await?.ok_or("job")?;
    assert_eq!(stored.record.status, JobStatus::Cancelled);
    assert_eq!(
        stored.record.outcome,
        Some(JobOutcome {
            code: JobOutcomeCode::Cancelled,
            at: now
        })
    );
    assert!(
        stored.record.target.is_none(),
        "a terminal job has no target"
    );
    assert_eq!(stored.record.expires_at, now + RETAIN);
    fakes.clock.advance(RETAIN + Duration::seconds(1));
    assert!(
        fakes
            .store
            .jobs()
            .expires_by(
                fakes.clock.now(),
                PageRequest {
                    limit: 100,
                    after: None,
                },
            )
            .await?
            .items
            .iter()
            .any(|j| j.record.job_id == job),
        "the record is collectable after the retention window"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-05 AC5 / S10 6.3: the cancel loses to the runner's claim, cleanly.
// ---------------------------------------------------------------------------

/// A store whose `jobs()` repo lets the runner claim the job between the
/// cancel's read and its conditional write.
struct RacingStore {
    inner: Arc<InMemoryServerStore>,
    jobs: RacingJobs,
}

struct RacingJobs {
    inner: Arc<InMemoryServerStore>,
    armed: Arc<AtomicBool>,
}

impl RacingJobs {
    /// One-shot: claim the job (bump its version) before the cancel's write.
    async fn claim_first(&self, job: &JobId) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        let Ok(Some(versioned)) = self.inner.jobs().get(job).await else {
            return;
        };
        let mut claimed = versioned.record;
        claimed.status = JobStatus::Running;
        let _ = self.inner.jobs().put(&claimed, Precondition::None).await;
    }
}

#[async_trait]
impl Repo<JobId, JobRecord> for RacingJobs {
    async fn get(&self, key: &JobId) -> Result<Option<Versioned<JobRecord>>, StoreError> {
        self.inner.jobs().get(key).await
    }

    async fn put(&self, record: &JobRecord, pre: Precondition) -> Result<Version, StoreError> {
        let job = record.job_id;
        self.claim_first(&job).await;
        self.inner.jobs().put(record, pre).await
    }

    async fn delete(&self, key: &JobId, pre: Precondition) -> Result<(), StoreError> {
        self.inner.jobs().delete(key, pre).await
    }
}

#[async_trait]
impl JobRepo for RacingJobs {
    async fn by_mailbox(
        &self,
        mailbox: &MailboxId,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        self.inner.jobs().by_mailbox(mailbox).await
    }

    async fn by_user_with_outcome(
        &self,
        user: &UserId,
        limit: u32,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        self.inner.jobs().by_user_with_outcome(user, limit).await
    }

    async fn queued_for_list(
        &self,
        user: &UserId,
        list: &ListKeyHash,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        self.inner.jobs().queued_for_list(user, list).await
    }

    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<Versioned<JobRecord>>, StoreError> {
        self.inner.jobs().expires_by(now, page).await
    }

    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        self.inner.jobs().delete_all_for_user(user).await
    }
}

#[async_trait]
impl ServerStore for RacingStore {
    fn users(&self) -> &dyn UserRepo {
        self.inner.users()
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
        &self.jobs
    }
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo {
        self.inner.needs_attention()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.inner.sessions()
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

/// A fixture whose job store can be raced; the flag is armed by the test.
fn racing_fixture() -> Result<(AppState, Fakes, Arc<AtomicBool>), Box<dyn std::error::Error>> {
    let (mut ports, fakes) = fake_ports();
    let armed = Arc::new(AtomicBool::new(false));
    ports.store = Arc::new(RacingStore {
        inner: Arc::clone(&fakes.store),
        jobs: RacingJobs {
            inner: Arc::clone(&fakes.store),
            armed: Arc::clone(&armed),
        },
    });
    Ok((
        app_state(Arc::new(ports), Arc::new(config()?)),
        fakes,
        armed,
    ))
}

#[tokio::test]
async fn sw_05_ac5_cancel_loses_to_claim_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes, armed) = racing_fixture()?;
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        user,
        "sub-race",
        "race@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let job = seed_job(&fakes, user, mailbox, JobStatus::Queued).await?;
    let due = fakes.clock.now() + Duration::minutes(5);
    fakes.scheduler.schedule(&job, due).await?;

    // The runner claims the job after the cancel's read.
    armed.store(true, Ordering::SeqCst);
    assert_eq!(
        cancel_queued_job(&state, &job).await?,
        CancelResult::NotQueued(JobStatus::Running)
    );

    let stored = fakes.store.jobs().get(&job).await?.ok_or("job")?;
    assert_eq!(stored.record.status, JobStatus::Running);
    assert!(stored.record.outcome.is_none(), "the claim stays intact");
    assert!(
        cancels(&fakes).is_empty(),
        "no task is deleted under a running job"
    );
    assert_eq!(fakes.scheduler.due(fakes.clock.now()), Vec::new());
    Ok(())
}

/// A cancel of a job that never existed is `Missing`, and a second cancel of
/// the same job is a no-op (`NotQueued(Cancelled)`).
#[tokio::test]
async fn cancel_queued_job_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        user,
        "sub-twice",
        "twice@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let missing = JobId::new(Uuid::new_v4());
    assert_eq!(
        cancel_queued_job(&state, &missing).await?,
        CancelResult::Missing
    );
    let job = seed_job(&fakes, user, mailbox, JobStatus::Queued).await?;
    assert_eq!(
        cancel_queued_job(&state, &job).await?,
        CancelResult::Cancelled
    );
    assert_eq!(
        cancel_queued_job(&state, &job).await?,
        CancelResult::NotQueued(JobStatus::Cancelled)
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-05 AC2 / INV-2: two disconnects that race on the user's two mailboxes
// cannot leave the user with zero mailboxes (security review F2).
// ---------------------------------------------------------------------------

/// A store whose mailbox `by_user` serves one stale read, so the step-3 guard
/// sees the two-mailbox picture both racing requests saw, and later reads are
/// live.
struct StaleOnceStore {
    inner: Arc<InMemoryServerStore>,
    mailboxes: StaleOnceMailboxes,
}

struct StaleOnceMailboxes {
    inner: Arc<InMemoryServerStore>,
    stale: Vec<Versioned<MailboxRecord>>,
    served: AtomicBool,
}

#[async_trait]
impl Repo<MailboxId, MailboxRecord> for StaleOnceMailboxes {
    async fn get(&self, key: &MailboxId) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        self.inner.mailboxes().get(key).await
    }

    async fn put(&self, record: &MailboxRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.inner.mailboxes().put(record, pre).await
    }

    async fn delete(&self, key: &MailboxId, pre: Precondition) -> Result<(), StoreError> {
        self.inner.mailboxes().delete(key, pre).await
    }
}

#[async_trait]
impl MailboxRepo for StaleOnceMailboxes {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<MailboxRecord>>, StoreError> {
        if !self.served.swap(true, Ordering::SeqCst) {
            return Ok(self.stale.clone());
        }
        self.inner.mailboxes().by_user(user).await
    }

    async fn by_subject(
        &self,
        provider: Provider,
        subject: &ProviderSubjectId,
    ) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        self.inner.mailboxes().by_subject(provider, subject).await
    }

    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<MailboxRecord>>, StoreError> {
        self.inner.mailboxes().list(page).await
    }

    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        self.inner.mailboxes().delete_all_for_user(user).await
    }
}

#[async_trait]
impl ServerStore for StaleOnceStore {
    fn users(&self) -> &dyn UserRepo {
        self.inner.users()
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        &self.mailboxes
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
        self.inner.sessions()
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

/// The other racing request has already removed the primary, but this one's
/// guard read still saw two mailboxes. The disconnect must refuse and put the
/// mailbox it deleted back rather than leave the user unreachable.
#[tokio::test]
async fn au_05_ac2_concurrent_disconnect_cannot_leave_zero_mailboxes(
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut ports, fakes) = fake_ports();
    let user = seed_user(&fakes).await?;
    let primary = seed_mailbox(
        &fakes,
        user,
        "sub-race-primary",
        "primary@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let other = seed_mailbox(
        &fakes,
        user,
        "sub-race-other",
        "other@example.com",
        false,
        MailboxStatus::Connected,
    )
    .await?;
    // What both racing requests read at step 3.
    let stale = fakes.store.mailboxes().by_user(&user).await?;
    // The other request's delete lands first.
    fakes
        .store
        .mailboxes()
        .delete(&primary, Precondition::None)
        .await?;
    ports.store = Arc::new(StaleOnceStore {
        inner: Arc::clone(&fakes.store),
        mailboxes: StaleOnceMailboxes {
            inner: Arc::clone(&fakes.store),
            stale,
            served: AtomicBool::new(false),
        },
    });
    let state = app_state(Arc::new(ports), Arc::new(config()?));
    let router = build_router(state.clone());
    let (cookie, record) = SessionService::new(&state)
        .establish(None, &user, Some(fakes.clock.now() - Duration::seconds(10)))
        .await?;
    let a = Authed {
        cookie: cookie
            .0
            .to_str()?
            .split(';')
            .next()
            .unwrap_or_default()
            .to_owned(),
        csrf: record.record.csrf_token.clone(),
        user,
    };

    let resp = disconnect(&router, &a, &other).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "last_mailbox");
    let left = fakes.store.mailboxes().by_user(&user).await?;
    assert_eq!(left.len(), 1, "the user keeps exactly one mailbox");
    assert_eq!(left[0].record.mailbox_id, other);
    assert_eq!(
        left[0].record.status,
        MailboxStatus::NeedsSignIn,
        "the restored mailbox has no grant, so it needs a sign-in"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V16.3.2: authorisation refusals beyond the admin extractor are logged
// (security-review F2).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v16_3_2_step_up_refusal_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    // A sign-in 20 minutes ago: outside the 300 s step-up window.
    let a = authed(&state, &fakes, Some(Duration::minutes(20))).await?;
    let (mine, _other) = seed_pair(&state, &fakes, a.user).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = disconnect(&router, &a, &mine).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert!(
        capture.text().contains("authz_failure"),
        "a refused step-up is logged: {}",
        capture.text()
    );
    assert!(
        fakes.store.mailboxes().get(&mine).await?.is_some(),
        "a refused step-up changes nothing"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v16_3_2_cross_user_disconnect_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let (_mine, _other) = seed_pair(&state, &fakes, a.user).await?;
    let stranger = seed_user(&fakes).await?;
    let foreign = seed_mailbox(
        &fakes,
        stranger,
        "sub-foreign",
        "foreign@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    grant(&state, &fakes, &stranger, &foreign, "refresh-foreign").await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = disconnect(&router, &a, &foreign).await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(
        capture.text().contains("authz_failure"),
        "the refused cross-user access is logged: {}",
        capture.text()
    );
    assert!(
        fakes.store.mailboxes().get(&foreign).await?.is_some(),
        "the stranger's mailbox is untouched"
    );
    Ok(())
}
