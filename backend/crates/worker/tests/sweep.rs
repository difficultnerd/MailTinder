//! Service integration and property tests for the sweep and its internal route
//! (T-706). Every test returns `Result`; an assertion failure is the test
//! failing.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::struct_field_names
)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::{
    InviteStatus, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, MessageClass,
    NeedsAttentionReason, Provider, ProviderSubjectId, UserId,
};
use obs::Sensitive;
use ports::{
    AgeBucket, Ciphertext, ClassifierEvalRecord, ClassifierEvalRepo, Clock, EmailLookupHash,
    EvalHeaderFacts, EvalId, EvalOutcome, HeaderRulesResult, InviteId, InviteRecord, InviteRepo,
    JobOutcomeCode, JobRecord, JobRepo, KeyService, ListKeyHash, MailboxRecord, MailboxRepo,
    NeedsAttentionRecord, NeedsAttentionRepo, Page, PageRequest, Precondition, RateLimitKey, Repo,
    Rng, ServerStore, SessionHash, SessionRecord, SessionRecordId, SessionRepo, SessionState,
    Sha256Hash, SwipeDirection, TextTokensBucket, UserPseudoId, UserRecord, UserRepo, Versioned,
};
use proptest::prelude::*;
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use svc_common::needs_attention::{raise_item, NewItem, NEEDS_ATTENTION_TTL};
use testkit::{Fakes, InMemoryServerStore};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use unsub::runner::{run_job, Delivery, RunResponse, UnsubState};
use unsub::sender::{ClaimedJob, SendResult, UnsubSender};
use uuid::Uuid;
use worker::sweep::{run_sweep, WorkerState, SWEEP_BATCH, SWEEP_INTERVAL_MIN};
use worker::{router, SWEEP_PATH};

const AUDIENCE: &str = "worker-audience";
const CALLER: &str = "scheduler@mailtinder.iam.gserviceaccount.com";
const TASKS_CALLER: &str = "tasks@mailtinder.iam.gserviceaccount.com";
const TARGET: &str = "https://lists.example.com/unsubscribe?token=abc";
const SENDER_DISPLAY: &str = "Example News";

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Env {
    ports: ports::Ports,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
}

async fn setup() -> Result<Env, Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(&fakes, &user, "sub-1").await?;
    Ok(Env {
        ports,
        fakes,
        user,
        mailbox,
    })
}

fn worker_state(env: &Env) -> WorkerState {
    WorkerState {
        ports: env.ports.clone(),
        auth: InternalAuthConfig {
            audience: AUDIENCE.to_owned(),
            allowed_caller_email: CALLER.to_owned(),
        },
    }
}

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
    user: &UserId,
    subject: &str,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let mailbox = MailboxId::new(fakes.rng.uuid_v4());
    let record = MailboxRecord {
        mailbox_id: mailbox,
        user_id: *user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new(subject)?,
        email_address: Ciphertext(vec![7u8; 24]),
        status: MailboxStatus::Connected,
        linked_at: fakes.clock.now(),
        is_primary: true,
        refresh_token: None,
    };
    fakes
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(mailbox)
}

#[allow(clippy::too_many_arguments)]
async fn seed_job(
    env: &Env,
    user: &UserId,
    mailbox: &MailboxId,
    list_seed: u8,
    method: JobMethod,
    status: JobStatus,
    due_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<JobId, Box<dyn std::error::Error>> {
    let job_id = JobId(Uuid::new_v4());
    let target = job_record::seal_target(
        &env.ports,
        user,
        &job_id,
        &Sensitive::new(TARGET.to_owned()),
    )
    .await?;
    let display = job_record::seal_sender_display(
        &env.ports,
        user,
        &job_id,
        &Sensitive::new(SENDER_DISPLAY.to_owned()),
    )
    .await?;
    let record = JobRecord {
        job_id,
        user_id: *user,
        mailbox_id: *mailbox,
        list_key_hash: ListKeyHash([list_seed; 32]),
        method,
        target: Some(target),
        sender_display: Some(display),
        due_at,
        status,
        attempts: 0,
        outcome: None,
        expires_at,
    };
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(job_id)
}

async fn seed_session(
    env: &Env,
    seed: u8,
    expires_at: OffsetDateTime,
) -> Result<SessionHash, Box<dyn std::error::Error>> {
    let hash = SessionHash([seed; 32]);
    let now = env.fakes.clock.now();
    let record = SessionRecord {
        session_hash: hash,
        session_record_id: SessionRecordId(env.fakes.rng.uuid_v4()),
        state: SessionState::Authenticated,
        user_id: Some(env.user),
        csrf_token: "csrf".to_owned(),
        created_at: now,
        last_seen_at: now,
        recent_auth_at: None,
        expires_at,
        pre_auth: None,
    };
    env.ports
        .store
        .sessions()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(hash)
}

async fn seed_eval(
    env: &Env,
    expires_at: OffsetDateTime,
) -> Result<EvalId, Box<dyn std::error::Error>> {
    let id = EvalId(env.fakes.rng.uuid_v4());
    let record = ClassifierEvalRecord {
        eval_id: id,
        user_pseudo_id: UserPseudoId("abc123".to_owned()),
        created_at: env.fakes.clock.now(),
        expires_at,
        header_rules: HeaderRulesResult {
            class: MessageClass::List,
            score: 5,
        },
        gemini: None,
        jev: None,
        outcome: EvalOutcome {
            direction: SwipeDirection::Right,
            undone: false,
            time_to_swipe_ms: 1,
        },
        header_facts: EvalHeaderFacts {
            has_list_unsubscribe: true,
            dkim_covers_list_unsubscribe: false,
            has_list_unsubscribe_post: false,
            has_list_id: false,
            has_feedback_id: false,
            precedence_bulk: false,
            auto_submitted: false,
            from_authenticated: false,
        },
        provider: Provider::Gmail,
        age_bucket: AgeBucket::Under7d,
        text_tokens_bucket: TextTokensBucket::Under100,
        lang_is_english: true,
        input_version: "v1".to_owned(),
        question_version: "v1".to_owned(),
        price_version: "v1".to_owned(),
    };
    env.ports
        .store
        .classifier_eval()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(id)
}

async fn seed_invite(
    env: &Env,
    seed: u8,
    purge_at: OffsetDateTime,
) -> Result<InviteId, Box<dyn std::error::Error>> {
    let id = InviteId(env.fakes.rng.uuid_v4());
    let now = env.fakes.clock.now();
    let record = InviteRecord {
        invite_id: id,
        email_address: Ciphertext(vec![seed; 24]),
        email_lookup: EmailLookupHash([seed; 32]),
        token_hash: Sha256Hash([seed; 32]),
        status: InviteStatus::Expired,
        created_at: now,
        last_sent_at: now,
        expires_at: purge_at,
        purge_at,
    };
    env.ports
        .store
        .invites()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(id)
}

async fn seed_rate_limit(
    env: &Env,
    key: &str,
    window_start: OffsetDateTime,
    window: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    env.ports
        .store
        .rate_limits()
        .hit(&RateLimitKey(key.to_owned()), window_start, window)
        .await?;
    Ok(())
}

async fn stored_job(env: &Env, id: &JobId) -> Result<JobRecord, Box<dyn std::error::Error>> {
    Ok(env
        .ports
        .store
        .jobs()
        .get(id)
        .await?
        .ok_or("missing job")?
        .record)
}

async fn open_items(env: &Env) -> Result<Vec<NeedsAttentionRecord>, Box<dyn std::error::Error>> {
    let page = PageRequest {
        limit: 100,
        after: None,
    };
    Ok(env
        .ports
        .store
        .needs_attention()
        .by_user(&env.user, page)
        .await?
        .items
        .into_iter()
        .map(|v| v.record)
        .collect())
}

async fn raise_expired_item(env: &Env) -> Result<(), Box<dyn std::error::Error>> {
    raise_item(
        &env.ports,
        NewItem {
            user: &env.user,
            mailbox: &env.mailbox,
            sender_display: SENDER_DISPLAY,
            link: None,
            reason: NeedsAttentionReason::JobExpired,
        },
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-05 AC2, UN-01 AC3, INV-2, JOB-1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_05_ac2_overdue_job_expires_with_needs_attention() -> TestResult {
    let env = setup().await?;
    let due = env.fakes.clock.now();
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due,
        due + Duration::hours(1),
    )
    .await?;

    // The clock is at `due_at + 1 h + 1 s`: past the job's hard TTL.
    env.fakes
        .clock
        .advance(Duration::hours(1) + Duration::seconds(1));
    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.jobs_expired, 1);

    let stored = stored_job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Expired);
    assert_eq!(
        stored.outcome.as_ref().map(|o| o.code),
        Some(JobOutcomeCode::Expired)
    );
    assert_eq!(stored.target, None, "the target is cleared at expiry");

    let items = open_items(&env).await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].reason_code, NeedsAttentionReason::JobExpired);
    assert_eq!(
        items[0].expires_at,
        env.fakes.clock.now() + NEEDS_ATTENTION_TTL
    );
    Ok(())
}

#[tokio::test]
async fn un_01_ac3_expired_outcome_logged_and_stored() -> TestResult {
    let env = setup().await?;
    let (capture, _guard) =
        obs::capture("worker", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let due = env.fakes.clock.now();
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due,
        due + Duration::hours(1),
    )
    .await?;
    env.fakes
        .clock
        .advance(Duration::hours(1) + Duration::seconds(1));

    let counts = run_sweep(&worker_state(&env)).await;
    assert_eq!(counts.jobs_expired, 1);

    let stored = stored_job(&env, &id).await?;
    assert_eq!(
        stored.outcome.as_ref().map(|o| o.code),
        Some(JobOutcomeCode::Expired),
        "the expired outcome is stored on the job"
    );

    let text = capture.text();
    assert_eq!(
        text.matches("unsub_job_outcome").count(),
        1,
        "one security event per expired job: {text}"
    );
    assert!(text.contains("expired"));
    Ok(())
}

#[tokio::test]
async fn inv_2_sweeper_deletes_expired_jobs() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    // A terminal job whose 30-day outcome retention has ended.
    let gone = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Sent,
        now,
        now - Duration::seconds(1),
    )
    .await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.jobs_deleted, 1);
    assert!(env.ports.store.jobs().get(&gone).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn inv_2_sweeper_deletes_expired_needs_attention() -> TestResult {
    let env = setup().await?;
    raise_expired_item(&env).await?;
    env.fakes
        .clock
        .advance(NEEDS_ATTENTION_TTL + Duration::seconds(1));

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.needs_attention_deleted, 1);
    assert!(open_items(&env).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn job_1_no_job_survives_expiry_plus_interval() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    // A terminal job at `expires_at + sweep interval` (JOB-1's bound).
    let gone = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Sent,
        now,
        now - Duration::minutes(SWEEP_INTERVAL_MIN),
    )
    .await?;
    // A terminal job one second before its `expires_at` is kept.
    let kept = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        2,
        JobMethod::OneClick,
        JobStatus::Sent,
        now,
        now + Duration::seconds(1),
    )
    .await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.jobs_deleted, 1);
    assert!(env.ports.store.jobs().get(&gone).await?.is_none());
    assert!(env.ports.store.jobs().get(&kept).await?.is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// NA-01 AC2, NA-T1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn na_01_ac2_item_deleted_after_ttl() -> TestResult {
    let env = setup().await?;
    raise_expired_item(&env).await?;

    // One second before the TTL: kept.
    env.fakes
        .clock
        .advance(NEEDS_ATTENTION_TTL - Duration::seconds(1));
    let counts = run_sweep(&worker_state(&env)).await;
    assert_eq!(counts.needs_attention_deleted, 0);
    assert_eq!(open_items(&env).await?.len(), 1);

    // Past the TTL: deleted.
    env.fakes.clock.advance(Duration::seconds(2));
    let counts = run_sweep(&worker_state(&env)).await;
    assert_eq!(counts.needs_attention_deleted, 1);
    assert!(open_items(&env).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn na_t1_no_item_survives_30_days() -> TestResult {
    let env = setup().await?;
    raise_expired_item(&env).await?;
    env.fakes
        .clock
        .advance(Duration::days(30) + Duration::seconds(1));

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.needs_attention_deleted, 1);
    assert!(open_items(&env).await?.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SES-1, INV-T1, RL-1, EXP-1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ses_1_sweeper_deletes_expired_sessions() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    let expired = seed_session(&env, 1, now - Duration::seconds(1)).await?;
    let live = seed_session(&env, 2, now + Duration::hours(1)).await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.sessions_deleted, 1);
    assert!(env.ports.store.sessions().get(&expired).await?.is_none());
    assert!(env.ports.store.sessions().get(&live).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn inv_t1_invites_deleted_when_purge_due() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    let due = seed_invite(&env, 1, now - Duration::seconds(1)).await?;
    let live = seed_invite(&env, 2, now + Duration::days(1)).await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.invites_deleted, 1);
    assert!(env.ports.store.invites().get(&due).await?.is_none());
    assert!(env.ports.store.invites().get(&live).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn rl_1_rate_limit_counters_deleted() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    // The window ended five minutes ago.
    seed_rate_limit(
        &env,
        "signin:abc",
        now - Duration::minutes(10),
        Duration::minutes(5),
    )
    .await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.rate_limits_deleted, 1);
    Ok(())
}

#[tokio::test]
async fn exp_1_sweeper_deletes_expired_eval_records() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    let expired = seed_eval(&env, now - Duration::seconds(1)).await?;
    let live = seed_eval(&env, now + Duration::days(1)).await?;

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.evals_deleted, 1);
    assert!(env
        .ports
        .store
        .classifier_eval()
        .get(&expired)
        .await?
        .is_none());
    assert!(env
        .ports
        .store
        .classifier_eval()
        .get(&live)
        .await?
        .is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.3.1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v8_3_1_sweeper_rechecks_owner() -> TestResult {
    let env = setup().await?;
    // A second user owns the mailbox this job names.
    let other = seed_user(&env.fakes).await?;
    let other_mailbox = seed_mailbox(&env.fakes, &other, "sub-2").await?;
    let due = env.fakes.clock.now();
    let id = seed_job(
        &env,
        &env.user,
        &other_mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due,
        due + Duration::hours(1),
    )
    .await?;
    env.fakes
        .clock
        .advance(Duration::hours(1) + Duration::seconds(1));

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.jobs_expired, 1);
    let stored = stored_job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Expired);
    assert!(
        open_items(&env).await?.is_empty(),
        "a job whose mailbox belongs to another user raises no item"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-05 AC2 (property): every job ends terminal within JOB_TTL
// ---------------------------------------------------------------------------

/// A sender with a fixed result, for driving T-701's runner.
struct ScriptedSender {
    result: SendResult,
}

#[async_trait]
impl UnsubSender for ScriptedSender {
    fn method(&self) -> JobMethod {
        JobMethod::OneClick
    }

    async fn send(&self, _ports: &ports::Ports, _job: &ClaimedJob) -> SendResult {
        self.result
    }
}

/// Drive one generated job through T-701's runner until it is terminal, then
/// advance the clock past `due_at + JOB_TTL` and run one sweep. Returns whether
/// every job ended terminal.
async fn terminal_property_case(behaviours: &[u8]) -> Result<bool, Box<dyn std::error::Error>> {
    let env = setup().await?;
    let due = env.fakes.clock.now();
    let mut ids = Vec::new();
    for (i, b) in behaviours.iter().enumerate() {
        let id = seed_job(
            &env,
            &env.user,
            &env.mailbox,
            (i % 250) as u8 + 1,
            JobMethod::OneClick,
            JobStatus::Queued,
            due,
            due + Duration::hours(1),
        )
        .await?;
        let result = match b {
            0 => SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
            1 => SendResult::Retryable {
                code: JobOutcomeCode::TimedOut,
            },
            _ => SendResult::NeedsAttention {
                reason: NeedsAttentionReason::UnsubscribeFailed,
                code: JobOutcomeCode::Refused,
            },
        };
        let state = UnsubState {
            ports: env.ports.clone(),
            senders: vec![Arc::new(ScriptedSender { result })],
            auth: InternalAuthConfig {
                audience: "unsub-audience".to_owned(),
                allowed_caller_email: "tasks@mailtinder.iam.gserviceaccount.com".to_owned(),
            },
        };
        for attempt in 0..4u32 {
            match run_job(&state, &id, Delivery { attempt }).await? {
                RunResponse::Done => break,
                RunResponse::Retry => {}
            }
        }
        ids.push(id);
    }

    env.fakes
        .clock
        .advance(Duration::hours(1) + Duration::seconds(1));
    let _ = run_sweep(&worker_state(&env)).await;

    for id in ids {
        let stored = stored_job(&env, &id).await?;
        if !stored.status.is_terminal() {
            return Ok(false);
        }
    }
    Ok(true)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn un_05_ac2_every_job_terminal_within_job_ttl(
        behaviours in proptest::collection::vec(0u8..3, 1..6),
    ) {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => panic!("tokio runtime"),
        };
        let all_terminal = rt.block_on(async move { terminal_property_case(&behaviours).await });
        prop_assert!(
            matches!(all_terminal.as_ref(), Ok(true)),
            "every job must be terminal after the sweep: {all_terminal:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The route (API-INT-2)
// ---------------------------------------------------------------------------

fn request(headers: &[(&str, &str)]) -> Result<Request<Body>, Box<dyn std::error::Error>> {
    let mut builder = Request::builder().method("POST").uri(SWEEP_PATH);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    Ok(builder.body(Body::empty())?)
}

async fn call(
    state: WorkerState,
    req: Request<Body>,
) -> Result<(StatusCode, String), Box<dyn std::error::Error>> {
    let response = router(state).oneshot(req).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, String::from_utf8(bytes.to_vec())?))
}

#[tokio::test]
async fn api_int_2_rejects_non_scheduler_caller() -> TestResult {
    let env = setup().await?;

    // No token at all.
    let (missing, body) = call(worker_state(&env), request(&[])?).await?;
    assert_eq!(missing, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty(), "401 has an empty body");

    // A valid token for the Cloud Tasks account: the worker's caller is the
    // Cloud Scheduler account, so this is refused.
    let token = env.fakes.caller.mint(AUDIENCE, TASKS_CALLER);
    let (wrong, _) = call(
        worker_state(&env),
        request(&[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(wrong, StatusCode::UNAUTHORIZED);

    // The Cloud Scheduler account is accepted.
    let good = env.fakes.caller.mint(AUDIENCE, CALLER);
    let (ok, _) = call(
        worker_state(&env),
        request(&[("authorization", &format!("Bearer {good}"))])?,
    )
    .await?;
    assert_eq!(ok, StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn api_int_2_more_flag_when_batch_full() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    for i in 0..(SWEEP_BATCH + 1) {
        seed_job(
            &env,
            &env.user,
            &env.mailbox,
            (i % 250) as u8 + 1,
            JobMethod::OneClick,
            JobStatus::Sent,
            now,
            now - Duration::seconds(1),
        )
        .await?;
    }

    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert!(counts.more, "a full batch sets `more`");
    assert_eq!(counts.jobs_deleted, u64::from(SWEEP_BATCH));
    Ok(())
}

// ---------------------------------------------------------------------------
// API-INT-2: one failing step still runs the others, and the route answers 500
// ---------------------------------------------------------------------------

/// A store whose `sessions().expires_by` fails; everything else delegates.
struct FailingSessionsStore {
    inner: Arc<InMemoryServerStore>,
    sessions: FailingSessions,
}

struct FailingSessions {
    inner: Arc<InMemoryServerStore>,
}

impl ServerStore for FailingSessionsStore {
    fn users(&self) -> &dyn UserRepo {
        self.inner.users()
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        self.inner.mailboxes()
    }
    fn invites(&self) -> &dyn InviteRepo {
        self.inner.invites()
    }
    fn invite_requests(&self) -> &dyn ports::InviteRequestRepo {
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
    fn bakeoff_snapshots(&self) -> &dyn ports::BakeoffSnapshotRepo {
        self.inner.bakeoff_snapshots()
    }
    fn config(&self) -> &dyn ports::ConfigRepo {
        self.inner.config()
    }
    fn rate_limits(&self) -> &dyn ports::RateLimitRepo {
        self.inner.rate_limits()
    }
}

#[async_trait]
impl Repo<SessionHash, SessionRecord> for FailingSessions {
    async fn get(
        &self,
        k: &SessionHash,
    ) -> Result<Option<Versioned<SessionRecord>>, ports::StoreError> {
        self.inner.sessions().get(k).await
    }
    async fn put(
        &self,
        r: &SessionRecord,
        pre: Precondition,
    ) -> Result<ports::Version, ports::StoreError> {
        self.inner.sessions().put(r, pre).await
    }
    async fn delete(&self, k: &SessionHash, pre: Precondition) -> Result<(), ports::StoreError> {
        self.inner.sessions().delete(k, pre).await
    }
}

#[async_trait]
impl SessionRepo for FailingSessions {
    async fn by_user(
        &self,
        user: &UserId,
    ) -> Result<Vec<Versioned<SessionRecord>>, ports::StoreError> {
        self.inner.sessions().by_user(user).await
    }
    async fn expires_by(
        &self,
        _now: OffsetDateTime,
        _page: PageRequest,
    ) -> Result<Page<SessionHash>, ports::StoreError> {
        Err(ports::StoreError::Unavailable)
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, ports::StoreError> {
        self.inner.sessions().delete_all_for_user(user).await
    }
}

#[tokio::test]
async fn api_int_2_failed_step_returns_500_and_others_run() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();

    // One expired record for a step that works, so its count is non-zero.
    let _ = seed_eval(&env, now - Duration::seconds(1)).await?;
    let _ = seed_invite(&env, 1, now - Duration::seconds(1)).await?;

    let mut ports = env.ports.clone();
    ports.store = Arc::new(FailingSessionsStore {
        inner: Arc::clone(&env.fakes.store),
        sessions: FailingSessions {
            inner: Arc::clone(&env.fakes.store),
        },
    }) as Arc<dyn ServerStore>;
    let state = WorkerState {
        ports,
        auth: InternalAuthConfig {
            audience: AUDIENCE.to_owned(),
            allowed_caller_email: CALLER.to_owned(),
        },
    };

    let token = env.fakes.caller.mint(AUDIENCE, CALLER);
    let (status, body) = call(
        state,
        request(&[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let counts: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(counts["failed_steps"][0], "sessions");
    assert_eq!(
        counts["evals_deleted"].as_u64(),
        Some(1),
        "the next steps still ran: {body}"
    );
    assert_eq!(counts["invites_deleted"].as_u64(), Some(1));
    Ok(())
}

/// OBS-EV AC3 (worker): a terminal job with no recorded outcome emits
/// `history_missing` before the sweep deletes it (S10 8, T-1114). A terminal
/// job that carries its outcome emits nothing.
#[tokio::test]
async fn obs_ev_ac3_unsub_after_undo_and_history_missing_emitted() -> TestResult {
    let env = setup().await?;
    let now = env.fakes.clock.now();
    // Terminal, past retention, and (the anomaly) with no recorded outcome: it
    // can never be collected into History, so the automated action has none.
    let lost = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Sent,
        now,
        now - Duration::seconds(1),
    )
    .await?;
    // A normal terminal job (outcome recorded) is not a `history_missing`.
    let ok = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        2,
        JobMethod::OneClick,
        JobStatus::Sent,
        now,
        now - Duration::seconds(1),
    )
    .await?;
    let versioned = env
        .ports
        .store
        .jobs()
        .get(&ok)
        .await?
        .ok_or("missing job")?;
    let mut record = versioned.record;
    record.outcome = Some(ports::store::JobOutcome {
        code: JobOutcomeCode::OneClickAccepted,
        at: now,
    });
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::Matches(versioned.version))
        .await?;

    let (capture, _guard) =
        obs::capture("worker", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let counts = run_sweep(&worker_state(&env)).await;
    assert!(counts.failed_steps.is_empty(), "{:?}", counts.failed_steps);
    assert_eq!(counts.jobs_deleted, 2, "both terminal jobs are purged");
    assert!(env.ports.store.jobs().get(&lost).await?.is_none());
    assert!(env.ports.store.jobs().get(&ok).await?.is_none());
    assert_eq!(
        capture.text().matches("history_missing").count(),
        1,
        "only the terminal job with no outcome is a history_missing: {}",
        capture.text()
    );
    Ok(())
}
