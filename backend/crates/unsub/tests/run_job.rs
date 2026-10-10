//! Service integration tests for `unsub::run_job` and the internal route
//! (T-701). Every test returns `Result`; an assertion failure is the test
//! failing.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason, Provider,
    ProviderSubjectId, UserId,
};
use obs::Sensitive;
use ports::store::{JobOutcomeCode, JobRecord, NeedsAttentionRecord, Precondition};
use ports::{
    Ciphertext, Clock, IdError, JobScheduler, KeyService, MailboxRecord, Ports, Rng, ServerStore,
    UserRecord,
};
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use svc_common::mint::{mint_access_token, seal_refresh_token, MintError};
use testkit::Fakes;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use unsub::runner::{run_job, Delivery, RunResponse, UnsubState};
use unsub::sender::{ClaimedJob, SendResult, UnsubSender};
use uuid::Uuid;

const AUDIENCE: &str = "unsub-audience";
const CALLER: &str = "tasks@mailtinder.iam.gserviceaccount.com";
const CANARY_ACCESS: &str = "CANARY-access-token-0001";
const CANARY_REFRESH: &str = "CANARY-refresh-token-0001";
const TARGET: &str = "https://lists.example.com/unsubscribe?token=abc";
const SENDER_DISPLAY: &str = "Example News";

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A sender with a scripted result that records how often it was asked.
struct ScriptedSender {
    method: JobMethod,
    result: SendResult,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl UnsubSender for ScriptedSender {
    fn method(&self) -> JobMethod {
        self.method
    }

    async fn send(&self, _ports: &Ports, _job: &ClaimedJob) -> SendResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result
    }
}

/// A one-click sender that mints an access token before reporting success, so
/// the test can prove the token is never stored (UN-01 AC4).
struct MintingSender {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl UnsubSender for MintingSender {
    fn method(&self) -> JobMethod {
        JobMethod::OneClick
    }

    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match mint_access_token(ports, &job.job.user_id, &job.job.mailbox_id).await {
            Ok(_) => SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
            Err(MintError::Revoked) => SendResult::TokenRevoked,
            Err(_) => SendResult::Retryable {
                code: JobOutcomeCode::TimedOut,
            },
        }
    }
}

struct Env {
    ports: Ports,
    fakes: Fakes,
    calls: Arc<AtomicUsize>,
    user: UserId,
    mailbox: MailboxId,
}

async fn setup(_result: SendResult) -> Result<Env, Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(&ports, &fakes, &user, MailboxStatus::Connected, true).await?;
    let calls = Arc::new(AtomicUsize::new(0));
    Ok(Env {
        ports,
        fakes,
        calls,
        user,
        mailbox,
    })
}

fn state_with(env: &Env, senders: Vec<Arc<dyn UnsubSender>>) -> UnsubState {
    UnsubState {
        ports: env.ports.clone(),
        senders,
        auth: InternalAuthConfig {
            audience: AUDIENCE.to_owned(),
            allowed_caller_email: CALLER.to_owned(),
        },
    }
}

fn sender(env: &Env, result: SendResult) -> Arc<dyn UnsubSender> {
    Arc::new(ScriptedSender {
        method: JobMethod::OneClick,
        result,
        calls: Arc::clone(&env.calls),
    })
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
    ports: &Ports,
    fakes: &Fakes,
    user: &UserId,
    status: MailboxStatus,
    with_refresh: bool,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let mailbox = MailboxId::new(fakes.rng.uuid_v4());
    let mut record = MailboxRecord {
        mailbox_id: mailbox,
        user_id: *user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new("sub-1")?,
        email_address: Ciphertext(vec![7u8; 24]),
        status,
        linked_at: fakes.clock.now(),
        is_primary: true,
        refresh_token: None,
    };
    if with_refresh {
        let user_record = fakes
            .store
            .users()
            .get(user)
            .await?
            .ok_or("missing user")?
            .record;
        record.refresh_token = Some(
            seal_refresh_token(
                ports,
                &user_record,
                &mailbox,
                &Sensitive::new(CANARY_REFRESH.to_owned()),
            )
            .await?,
        );
    }
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
        list_key_hash: ports::ListKeyHash([list_seed; 32]),
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

async fn job(env: &Env, id: &JobId) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let v = env.ports.store.jobs().get(id).await?.ok_or("missing job")?;
    Ok(v.record)
}

fn due(env: &Env) -> OffsetDateTime {
    env.fakes.clock.now()
}

async fn run(
    _env: &Env,
    state: &UnsubState,
    id: &JobId,
    attempt: u32,
) -> Result<RunResponse, Box<dyn std::error::Error>> {
    Ok(run_job(state, id, Delivery { attempt }).await?)
}

// ---------------------------------------------------------------------------
// UN-01 AC1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac1_due_job_runs_exactly_once() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Sent);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::OneClickAccepted)
    );
    assert_eq!(env.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn un_01_ac1_duplicate_delivery_is_noop() -> TestResult {
    let env = setup(SendResult::Retryable {
        code: JobOutcomeCode::TimedOut,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Retryable {
                code: JobOutcomeCode::TimedOut,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    // First delivery: retryable, so the job stays Running.
    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Retry);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Running);
    // The same attempt delivered again does nothing and sends nothing.
    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Running);
    assert_eq!(env.calls.load(Ordering::SeqCst), 1, "one send only");
    Ok(())
}

#[tokio::test]
async fn un_01_ac1_missing_job_returns_200() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    let state = state_with(&env, vec![]);
    let missing = JobId(Uuid::new_v4());
    assert_eq!(run(&env, &state, &missing, 0).await?, RunResponse::Done);
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC2 batching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac2_batched_jobs_send_one_request() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    let a = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        7,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    let b = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        7,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    let c = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        7,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    env.fakes.scheduler.schedule(&b, due(&env)).await?;
    env.fakes.scheduler.schedule(&c, due(&env)).await?;

    assert_eq!(run(&env, &state, &a, 0).await?, RunResponse::Done);
    assert_eq!(
        env.calls.load(Ordering::SeqCst),
        1,
        "one request for the list"
    );

    for sibling in [b, c] {
        let stored = job(&env, &sibling).await?;
        assert_eq!(stored.status, JobStatus::Sent);
        assert_eq!(
            stored.outcome.map(|o| o.code),
            Some(JobOutcomeCode::Batched)
        );
        assert!(
            stored.target.is_none(),
            "a batched sibling drops its target"
        );
    }
    let cancelled: Vec<String> = env
        .fakes
        .scheduler
        .events()
        .into_iter()
        .filter_map(|e| match e {
            testkit::SchedulerEvent::Cancelled(name, _) => Some(name.0),
            testkit::SchedulerEvent::Scheduled(_, _) => None,
        })
        .collect();
    assert!(cancelled.contains(&ports::TaskName::for_job(&b).0));
    assert!(cancelled.contains(&ports::TaskName::for_job(&c).0));
    Ok(())
}

#[tokio::test]
async fn un_01_ac2_job_after_sent_sibling_sends_nothing() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    // A sibling of the same list already sent.
    let sent = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        9,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    assert_eq!(run(&env, &state, &sent, 0).await?, RunResponse::Done);
    let queued = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        9,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    assert_eq!(run(&env, &state, &queued, 0).await?, RunResponse::Done);
    let stored = job(&env, &queued).await?;
    assert_eq!(stored.status, JobStatus::Sent);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::Batched)
    );
    assert_eq!(
        env.calls.load(Ordering::SeqCst),
        1,
        "the sibling sent nothing"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC3 every outcome recorded and logged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac3_every_terminal_outcome_logged_and_stored() -> TestResult {
    // sent
    let (env, state, id) = one_job(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let _ = run(&env, &state, &id, 0).await?;
    assert_eq!(
        events(&capture.text(), "unsub_job_outcome"),
        1,
        "sent logs one outcome"
    );
    assert!(job(&env, &id).await?.outcome.is_some());
    drop(_guard);

    // needs_attention
    let (env, state, id) = one_job(SendResult::NeedsAttention {
        reason: NeedsAttentionReason::UnsubscribeFailed,
        code: JobOutcomeCode::HttpRejected,
    })
    .await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let _ = run(&env, &state, &id, 0).await?;
    assert_eq!(events(&capture.text(), "unsub_job_outcome"), 1);
    assert_eq!(job(&env, &id).await?.status, JobStatus::NeedsAttention);
    drop(_guard);

    // failed
    let (env, state, id) = one_job(SendResult::TokenRevoked).await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let _ = run(&env, &state, &id, 0).await?;
    assert_eq!(events(&capture.text(), "unsub_job_outcome"), 1);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Failed);
    drop(_guard);

    // expired
    let (env, state, id) = one_job(SendResult::TokenRevoked).await?;
    expire(&env, &id).await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let _ = run(&env, &state, &id, 0).await?;
    assert_eq!(events(&capture.text(), "unsub_job_outcome"), 1);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Expired);
    drop(_guard);
    Ok(())
}

fn events(text: &str, name: &str) -> usize {
    text.matches(name).count()
}

async fn one_job(
    result: SendResult,
) -> Result<(Env, UnsubState, JobId), Box<dyn std::error::Error>> {
    let env = setup(result).await?;
    let state = state_with(&env, vec![sender(&env, result)]);
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        3,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    Ok((env, state, id))
}

/// Make a seeded queued job overdue.
async fn expire(env: &Env, id: &JobId) -> Result<(), Box<dyn std::error::Error>> {
    let v = env.ports.store.jobs().get(id).await?.ok_or("missing")?;
    let mut record = v.record;
    record.expires_at = env.fakes.clock.now() - Duration::seconds(1);
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::Matches(v.version))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC4 no stored access token, no session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac4_access_token_minted_at_run_not_stored() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    // The identity fake mints the canary access token for our refresh token.
    env.fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));
    let calls = Arc::new(AtomicUsize::new(0));
    let state = state_with(
        &env,
        vec![Arc::new(MintingSender {
            calls: Arc::clone(&calls),
        })],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the token was minted at run time"
    );
    let dump = serde_json::to_string(&env.fakes.store.export_json())?;
    assert!(
        !dump.contains(CANARY_ACCESS),
        "no record holds the access token"
    );
    assert!(
        !dump.contains(CANARY_REFRESH),
        "no record holds the refresh token in the clear"
    );
    Ok(())
}

#[tokio::test]
async fn un_01_ac4_runs_without_session() -> TestResult {
    let (env, state, id) = one_job(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    let collections: Vec<&'static str> = env
        .fakes
        .store
        .export_json()
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert!(!collections.contains(&"sessions"), "no session was needed");
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC6 / ASVS V7.6.1 revoked grant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac6_revoked_refresh_token_fails_with_sign_in_item() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    env.fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));
    let calls = Arc::new(AtomicUsize::new(0));
    let state = state_with(&env, vec![Arc::new(MintingSender { calls })]);
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::TokenInvalid)
    );

    let item = only_item(&env).await?;
    assert_eq!(item.reason_code, NeedsAttentionReason::SignInRequired);
    Ok(())
}

#[tokio::test]
async fn un_01_ac6_mailbox_set_needs_sign_in() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    env.fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));
    let calls = Arc::new(AtomicUsize::new(0));
    let state = state_with(&env, vec![Arc::new(MintingSender { calls })]);
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    let _ = run(&env, &state, &id, 0).await?;

    let mailbox = env
        .ports
        .store
        .mailboxes()
        .get(&env.mailbox)
        .await?
        .ok_or("missing mailbox")?;
    assert_eq!(mailbox.record.status, MailboxStatus::NeedsSignIn);
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-05 AC1 item carries sender, link and reason
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_05_ac1_failed_job_creates_item_with_sender_link_reason() -> TestResult {
    let (env, state, id) = one_job(SendResult::NeedsAttention {
        reason: NeedsAttentionReason::UnsubscribeFailed,
        code: JobOutcomeCode::HttpRejected,
    })
    .await?;
    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);

    let item = only_item(&env).await?;
    assert_eq!(item.reason_code, NeedsAttentionReason::UnsubscribeFailed);
    let display = open_item(&env, &item, ports::store::aad_fields::NA_SENDER_DISPLAY).await?;
    assert_eq!(display, SENDER_DISPLAY);
    let link_ct = item.link.as_ref().ok_or("item has no link")?;
    let link_aad = ports::Aad {
        user: env.user,
        scope: item.item_id.0.to_string(),
        field: ports::store::aad_fields::NA_LINK,
    };
    let wrapped = user(&env).await?;
    let raw = env
        .ports
        .keys
        .open(&env.user, &wrapped.wrapped_data_key, &link_aad, &link_ct.0)
        .await?;
    assert_eq!(String::from_utf8(raw)?, TARGET);
    Ok(())
}

async fn user(env: &Env) -> Result<UserRecord, Box<dyn std::error::Error>> {
    Ok(env
        .ports
        .store
        .users()
        .get(&env.user)
        .await?
        .ok_or("missing user")?
        .record)
}

async fn open_item(
    env: &Env,
    item: &NeedsAttentionRecord,
    field: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    let aad = ports::Aad {
        user: env.user,
        scope: item.item_id.0.to_string(),
        field,
    };
    let wrapped = user(env).await?.wrapped_data_key;
    let raw = env
        .ports
        .keys
        .open(&env.user, &wrapped, &aad, &item.sender_display.0)
        .await?;
    Ok(String::from_utf8(raw)?)
}

async fn only_item(env: &Env) -> Result<NeedsAttentionRecord, Box<dyn std::error::Error>> {
    let page = env
        .ports
        .store
        .needs_attention()
        .by_user(
            &env.user,
            ports::PageRequest {
                limit: 10,
                after: None,
            },
        )
        .await?;
    assert_eq!(page.items.len(), 1, "exactly one Needs Attention item");
    Ok(page.items.into_iter().next().ok_or("missing item")?.record)
}

// ---------------------------------------------------------------------------
// UN-05 AC2 no loops
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_05_ac2_retry_until_final_attempt_then_needs_attention() -> TestResult {
    let env = setup(SendResult::Retryable {
        code: JobOutcomeCode::TimedOut,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Retryable {
                code: JobOutcomeCode::TimedOut,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    for attempt in 0..3 {
        assert_eq!(run(&env, &state, &id, attempt).await?, RunResponse::Retry);
    }
    assert_eq!(run(&env, &state, &id, 3).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::RetriesExhausted)
    );
    Ok(())
}

#[tokio::test]
async fn un_05_ac2_job_never_requeues_itself() -> TestResult {
    let env = setup(SendResult::Retryable {
        code: JobOutcomeCode::HttpRejected,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Retryable {
                code: JobOutcomeCode::HttpRejected,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    let _ = run(&env, &state, &id, 0).await?;

    let scheduled = env
        .fakes
        .scheduler
        .events()
        .into_iter()
        .filter(|e| matches!(e, testkit::SchedulerEvent::Scheduled(_, _)))
        .count();
    assert_eq!(scheduled, 0, "unsub never schedules a task");
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-2, JOB-1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv_2_terminal_job_has_expires_at() -> TestResult {
    let (env, state, id) = one_job(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let _ = run(&env, &state, &id, 0).await?;
    let stored = job(&env, &id).await?;
    assert!(stored.status.is_terminal());
    assert_eq!(
        stored.expires_at,
        due(&env) + Duration::days(30),
        "a terminal job keeps its outcome for 30 days"
    );
    Ok(())
}

#[tokio::test]
async fn job_1_no_access_token_on_job_record() -> TestResult {
    let (env, state, id) = one_job(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let _ = run(&env, &state, &id, 0).await?;
    let json = serde_json::to_value(&job(&env, &id).await?)?;
    let mut keys = Vec::new();
    let mut stack = vec![json];
    while let Some(v) = stack.pop() {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    keys.push(k);
                    stack.push(val);
                }
            }
            serde_json::Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    for key in keys {
        assert!(
            !key.to_ascii_lowercase().contains("token"),
            "job has a token field {key}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn job_1_terminal_job_keeps_only_outcome() -> TestResult {
    let (env, state, id) = one_job(SendResult::NeedsAttention {
        reason: NeedsAttentionReason::UnsubscribeFailed,
        code: JobOutcomeCode::HttpRejected,
    })
    .await?;
    let _ = run(&env, &state, &id, 0).await?;
    let stored = job(&env, &id).await?;
    assert!(stored.target.is_none(), "the target is cleared");
    assert!(
        stored.sender_display.is_none(),
        "the sender display is cleared once the item is written"
    );
    assert!(stored.outcome.is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.3.1 ownership
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v8_3_1_runner_rechecks_mailbox_owner() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    // A second user owns the mailbox the job names.
    let other = seed_user(&env.fakes).await?;
    let other_box = seed_mailbox(
        &env.ports,
        &env.fakes,
        &other,
        MailboxStatus::Connected,
        false,
    )
    .await?;
    let id = seed_job(
        &env,
        &env.user,
        &other_box,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    assert_eq!(run(&env, &state, &id, 0).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::OwnerMismatch)
    );
    assert_eq!(env.calls.load(Ordering::SeqCst), 0, "nothing was sent");
    assert!(
        env.ports
            .store
            .needs_attention()
            .count_for_user(&env.user)
            .await?
            == 0,
        "no item for an owner mismatch"
    );
    assert!(capture.text().contains("job_owner_mismatch"));
    Ok(())
}

#[tokio::test]
async fn asvs_v7_6_1_revoked_grant_ends_job_with_sign_in_item() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    env.fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));
    let calls = Arc::new(AtomicUsize::new(0));
    let state = state_with(&env, vec![Arc::new(MintingSender { calls })]);
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;

    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let _ = run(&env, &state, &id, 0).await?;
    let item = only_item(&env).await?;
    assert_eq!(item.reason_code, NeedsAttentionReason::SignInRequired);
    assert!(capture.text().contains("sign_in_required"));
    Ok(())
}

// ---------------------------------------------------------------------------
// The route
// ---------------------------------------------------------------------------

fn route_state(env: &Env) -> UnsubState {
    state_with(env, vec![])
}

fn request(
    path: &str,
    headers: &[(&str, &str)],
) -> Result<Request<Body>, Box<dyn std::error::Error>> {
    let mut builder = Request::builder().method("POST").uri(path);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    Ok(builder.body(Body::empty())?)
}

async fn call(
    state: UnsubState,
    req: Request<Body>,
) -> Result<(StatusCode, String), Box<dyn std::error::Error>> {
    let response = unsub::router(state).oneshot(req).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, String::from_utf8(bytes.to_vec())?))
}

#[tokio::test]
async fn api_int_1_rejects_missing_token() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let path = format!("/internal/v1/unsubscribe-jobs/{}/run", Uuid::new_v4());
    let (status, body) = call(route_state(&env), request(&path, &[])?).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty(), "401 has an empty body");
    assert!(capture.text().contains("internal_auth_failed"));
    Ok(())
}

/// ASVS V16.3.2: every failed internal authentication writes one security
/// event, and the token itself never reaches the log.
#[tokio::test]
async fn asvs_v16_3_2_internal_auth_failure_logged() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let path = format!("/internal/v1/unsubscribe-jobs/{}/run", Uuid::new_v4());

    // No token at all: refused and logged.
    let (missing, _) = call(route_state(&env), request(&path, &[])?).await?;
    assert_eq!(missing, StatusCode::UNAUTHORIZED);

    // A well-formed token for the wrong caller: also refused and logged.
    let token = env.fakes.caller.mint(AUDIENCE, "someone-else@example.com");
    let (wrong, _) = call(
        route_state(&env),
        request(&path, &[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(wrong, StatusCode::UNAUTHORIZED);

    let text = capture.text();
    assert_eq!(
        text.matches("internal_auth_failed").count(),
        2,
        "each refusal writes one security event: {text}"
    );
    assert!(!text.contains(&token), "the bearer token is never logged");
    Ok(())
}

#[tokio::test]
async fn api_int_1_rejects_wrong_audience() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    // Minted for a different audience.
    let token = env.fakes.caller.mint("worker-audience", CALLER);
    let path = format!("/internal/v1/unsubscribe-jobs/{}/run", Uuid::new_v4());
    let (status, _) = call(
        route_state(&env),
        request(&path, &[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn api_int_1_rejects_wrong_caller_email() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    let token = env.fakes.caller.mint(AUDIENCE, "someone-else@example.com");
    let path = format!("/internal/v1/unsubscribe-jobs/{}/run", Uuid::new_v4());
    let (status, _) = call(
        route_state(&env),
        request(&path, &[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn api_int_1_valid_caller_runs_and_returns_json() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        1,
        JobMethod::OneClick,
        JobStatus::Queued,
        due(&env),
        due(&env) + Duration::hours(1),
    )
    .await?;
    let token = env.fakes.caller.mint(AUDIENCE, CALLER);
    let path = format!("/internal/v1/unsubscribe-jobs/{}/run", id.0);
    let (status, body) = call(
        state,
        request(&path, &[("authorization", &format!("Bearer {token}"))])?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "{}");
    assert_eq!(job(&env, &id).await?.status, JobStatus::Sent);
    Ok(())
}

#[tokio::test]
async fn api_int_1_rejects_non_uuid_job_id() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    let token = env.fakes.caller.mint(AUDIENCE, CALLER);
    let (status, _) = call(
        route_state(&env),
        request(
            "/internal/v1/unsubscribe-jobs/not-a-uuid/run",
            &[("authorization", &format!("Bearer {token}"))],
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn api_int_1_release_build_has_no_test_routes() -> TestResult {
    let env = setup(SendResult::TokenRevoked).await?;
    assert_eq!(unsub::ROUTE_TEMPLATES.len(), 1, "exactly one route");
    assert_eq!(
        unsub::ROUTE_TEMPLATES[0],
        "/internal/v1/unsubscribe-jobs/{job_id}/run"
    );
    // An unmatched path is a 404: there are no test hooks.
    let (status, _) = call(route_state(&env), request("/internal/v1/test", &[])?).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// SW-05 AC5: the claim and an undo cancel race; exactly one wins.
#[tokio::test]
async fn sw_05_ac5_claim_and_cancel_exactly_one_wins() -> TestResult {
    for cancel_first in [false, true] {
        let env = setup(SendResult::Sent {
            code: JobOutcomeCode::OneClickAccepted,
        })
        .await?;
        let state = state_with(
            &env,
            vec![sender(
                &env,
                SendResult::Sent {
                    code: JobOutcomeCode::OneClickAccepted,
                },
            )],
        );
        let id = seed_job(
            &env,
            &env.user,
            &env.mailbox,
            1,
            JobMethod::OneClick,
            JobStatus::Queued,
            due(&env),
            due(&env) + Duration::hours(1),
        )
        .await?;

        if cancel_first {
            assert!(undo_cancel(&env, &id).await?);
            let _ = run(&env, &state, &id, 0).await?;
            let stored = job(&env, &id).await?;
            assert_eq!(stored.status, JobStatus::Cancelled);
            assert_eq!(env.calls.load(Ordering::SeqCst), 0, "cancel won: no send");
        } else {
            let _ = run(&env, &state, &id, 0).await?;
            let cancel_won = undo_cancel(&env, &id).await?;
            let stored = job(&env, &id).await?;
            assert_eq!(stored.status, JobStatus::Sent);
            assert!(!cancel_won, "the run won: cancel is refused");
            assert_eq!(env.calls.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}

/// The api's undo: a conditional `Queued -> Cancelled` write. Returns true
/// when this writer won the race.
async fn undo_cancel(env: &Env, id: &JobId) -> Result<bool, Box<dyn std::error::Error>> {
    let Some(v) = env.ports.store.jobs().get(id).await? else {
        return Ok(false);
    };
    if v.record.status != JobStatus::Queued {
        return Ok(false);
    }
    let mut record = v.record;
    record.status = JobStatus::Cancelled;
    match env
        .ports
        .store
        .jobs()
        .put(&record, Precondition::Matches(v.version))
        .await
    {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// ASVS V16.5.3: any non-`Sent` send result never yields status `Sent`.
#[tokio::test]
async fn asvs_v16_5_3_job_never_marked_sent_on_error() -> TestResult {
    let results = [
        SendResult::Retryable {
            code: JobOutcomeCode::TimedOut,
        },
        SendResult::Retryable {
            code: JobOutcomeCode::HttpRejected,
        },
        SendResult::NeedsAttention {
            reason: NeedsAttentionReason::UnsubscribeFailed,
            code: JobOutcomeCode::HttpRejected,
        },
        SendResult::NeedsAttention {
            reason: NeedsAttentionReason::SignInRequired,
            code: JobOutcomeCode::TokenInvalid,
        },
        SendResult::TokenRevoked,
    ];
    for result in results {
        let (env, state, id) = one_job(result).await?;
        // Run every attempt so a retryable result reaches its terminal state.
        for attempt in 0..4 {
            let _ = run(&env, &state, &id, attempt).await?;
        }
        let stored = job(&env, &id).await?;
        assert_ne!(
            stored.status,
            JobStatus::Sent,
            "a {result:?} result must never end Sent"
        );
    }
    Ok(())
}

/// OBS-EV AC3 (unsub): a delivery that finds a job a successful undo has
/// already cancelled sends nothing and logs the benign race at `op` level; it
/// must **not** fire the A1 metric `unsub_after_undo`, which pages on any event
/// (S10 8, T-1114).
#[tokio::test]
async fn obs_ev_ac3_unsub_after_undo_and_history_missing_emitted() -> TestResult {
    let env = setup(SendResult::Sent {
        code: JobOutcomeCode::OneClickAccepted,
    })
    .await?;
    let state = state_with(
        &env,
        vec![sender(
            &env,
            SendResult::Sent {
                code: JobOutcomeCode::OneClickAccepted,
            },
        )],
    );
    let id = seed_job(
        &env,
        &env.user,
        &env.mailbox,
        5,
        JobMethod::OneClick,
        JobStatus::Cancelled,
        due(&env),
        due(&env) + Duration::days(30),
    )
    .await?;

    // A successful undo writes `Cancelled` before its delete runs, so the
    // record can still be seen by a delivery that races the undo.
    let versioned = env
        .ports
        .store
        .jobs()
        .get(&id)
        .await?
        .ok_or("missing job")?;
    let mut record = versioned.record;
    record.outcome = Some(ports::store::JobOutcome {
        code: JobOutcomeCode::Cancelled,
        at: env.fakes.clock.now(),
    });
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::Matches(versioned.version))
        .await?;

    let (capture, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    let response = run(&env, &state, &id, 0).await?;
    assert_eq!(response, RunResponse::Done);
    assert_eq!(
        env.calls.load(Ordering::SeqCst),
        0,
        "an undone job is never sent"
    );
    // Nothing was sent, so this is the designed race, not an unrecoverable
    // action: it is a non-A1 `op` log, never the paging `unsub_after_undo`.
    assert_eq!(events(&capture.text(), "unsub_after_undo"), 0);
    assert_eq!(events(&capture.text(), "unsub.run"), 1);
    Ok(())
}
