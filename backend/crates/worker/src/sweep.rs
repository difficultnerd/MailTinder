//! `run_sweep`: the internal route's work (T-706, S7 5.12 API-INT-2).
//!
//! Cloud Scheduler calls the one internal route every 15 minutes. The sweep
//! expires overdue unsubscribe jobs (raising a Needs Attention item so nothing
//! fails silently) and deletes every record whose retention has ended: terminal
//! jobs nobody collected, Needs Attention items, sessions, classifier
//! evaluation records, invites and rate-limit counters (S4 1). Firestore TTL
//! stays the backstop; this sweeper is the control.
//!
//! Every delete is idempotent and every state change is conditional, so two
//! overlapping runs (Cloud Scheduler retries) are safe. One failing step is
//! recorded in `failed_steps` and the next step still runs.

use domain::{JobMethod, JobStatus, NeedsAttentionReason};
use obs::{metric_event, Pseudonymiser, SecurityEvent};
use ports::store::{JobOutcome, JobOutcomeCode, JobRecord, Precondition, StoreError, Version};
use ports::{Ports, SecretName};
use serde::Serialize;
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use svc_common::needs_attention::{self, NewItem};
use svc_common::SvcError;
use time::{Duration, OffsetDateTime};
use url::Url;

/// How many records one collection yields per run `[TUNABLE]`; Firestore's free
/// tier is 20,000 writes a day.
pub const SWEEP_BATCH: u32 = 500;

/// Cloud Scheduler's interval in minutes (S7 5.12) `[TUNABLE]`; tests use it to
/// state JOB-1's bound.
pub const SWEEP_INTERVAL_MIN: i64 = 15;

/// How long a terminal job's outcome is kept (S3, S5 `jobs`) `[TUNABLE]`.
pub const OUTCOME_RETENTION: Duration = Duration::days(30);

/// The sender display used when a job carries none (T-701 trap 3).
const DEFAULT_SENDER_DISPLAY: &str = "this sender";

/// What one sweep did. Counts only: never an ID, link or name (S7 5.12).
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct SweepCounts {
    pub jobs_expired: u64,
    pub jobs_deleted: u64,
    pub needs_attention_deleted: u64,
    pub sessions_deleted: u64,
    pub evals_deleted: u64,
    pub invites_deleted: u64,
    pub rate_limits_deleted: u64,
    /// Some collection hit `SWEEP_BATCH`; the next run continues.
    pub more: bool,
    /// The steps that failed, for example `["sessions"]`; empty on success.
    pub failed_steps: Vec<&'static str>,
}

/// Everything the internal route needs.
#[derive(Clone)]
pub struct WorkerState {
    pub ports: Ports,
    pub auth: InternalAuthConfig,
}

/// Run every step once with one `now`. A failing step adds its name to
/// `failed_steps`, logs the error kind (no values) and the next step still runs.
pub async fn run_sweep(state: &WorkerState) -> SweepCounts {
    let ports = &state.ports;
    let now = ports.clock.now();
    let mut counts = SweepCounts::default();

    if let Err(e) = expire_and_purge_jobs(ports, now, &mut counts).await {
        counts.failed_steps.push("jobs");
        log_step_failure("jobs", &e);
    }
    if let Err(e) = purge_needs_attention(ports, now, &mut counts).await {
        counts.failed_steps.push("needs_attention");
        log_step_failure("needs_attention", &e);
    }
    if let Err(e) = purge_sessions(ports, now, &mut counts).await {
        counts.failed_steps.push("sessions");
        log_step_failure("sessions", &e);
    }
    if let Err(e) = purge_evals(ports, now, &mut counts).await {
        counts.failed_steps.push("classifier_eval");
        log_step_failure("classifier_eval", &e);
    }
    if let Err(e) = purge_invites(ports, now, &mut counts).await {
        counts.failed_steps.push("invites");
        log_step_failure("invites", &e);
    }
    if let Err(e) = purge_rate_limits(ports, now, &mut counts).await {
        counts.failed_steps.push("rate_limits");
        log_step_failure("rate_limits", &e);
    }

    counts
}

/// Expire every overdue job (raising its item) and delete every terminal job
/// whose 30-day outcome retention has ended.
async fn expire_and_purge_jobs(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let due = ports
        .store
        .jobs()
        .expires_by(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if due.len() >= SWEEP_BATCH as usize {
        counts.more = true;
    }

    for versioned in due {
        let job = versioned.record;
        if job.status.is_terminal() {
            // Its outcome retention has ended; delete it conditionally so a
            // concurrent delete is a skip, never an error.
            if delete_ignoring_conflict(
                ports
                    .store
                    .jobs()
                    .delete(&job.job_id, Precondition::Matches(versioned.version))
                    .await,
            )? {
                counts.jobs_deleted += 1;
            }
            continue;
        }
        expire_one_job(ports, &job, versioned.version, now, counts).await?;
    }
    Ok(())
}

/// Expire one non-terminal job: re-check ownership (V8.3.1), write `Expired`
/// conditionally, raise the item, then clear `sender_display`.
async fn expire_one_job(
    ports: &Ports,
    job: &JobRecord,
    version: Version,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    // 1. Ownership re-check from the mailbox record (ASVS V8.3.1).
    let mailbox = ports
        .store
        .mailboxes()
        .get(&job.mailbox_id)
        .await
        .map_err(|_| SvcError::Store)?;
    let owner_matches = matches!(&mailbox, Some(m) if m.record.user_id == job.user_id);
    if !owner_matches {
        if mailbox.is_some() {
            log_security(ports, job, "job_owner_mismatch", "forbidden").await;
        }
        if expire_record(ports, job, version, now).await? {
            log_expired_outcome(ports, job).await;
            counts.jobs_expired += 1;
        }
        return Ok(());
    }

    // 2. Open the target and the sender display before the target is cleared.
    let display = job_record::open_sender_display(ports, &job.user_id, job)
        .await
        .ok();
    let link = match job_record::open_target(ports, &job.user_id, job).await {
        Ok(target) => https_link(target.expose()),
        Err(_) => None,
    };

    // 3. Conditional write: `Expired` with the outcome, target cleared.
    let Some(new_version) = write_expired(ports, job, version, now).await? else {
        // `unsub` or undo changed it a moment ago; skip this record.
        return Ok(());
    };

    // 4. Raise the item with the sender display (or "this sender").
    let display_str = display
        .as_ref()
        .map_or_else(|| DEFAULT_SENDER_DISPLAY.to_owned(), |s| s.expose().clone());
    needs_attention::raise_item(
        ports,
        NewItem {
            user: &job.user_id,
            mailbox: &job.mailbox_id,
            sender_display: &display_str,
            link: link.as_ref(),
            reason: NeedsAttentionReason::JobExpired,
        },
    )
    .await?;

    // 5. Clear `sender_display` in a second conditional write. A failure is
    // logged; the next run's terminal branch deletes the record later.
    let mut cleared = job.clone();
    cleared.status = JobStatus::Expired;
    cleared.outcome = Some(JobOutcome {
        code: JobOutcomeCode::Expired,
        at: now,
    });
    cleared.target = None;
    cleared.sender_display = None;
    cleared.expires_at = now + OUTCOME_RETENTION;
    if ports
        .store
        .jobs()
        .put(&cleared, Precondition::Matches(new_version))
        .await
        .is_err()
    {
        tracing::warn!(
            event = "op",
            route = "worker.sweep",
            outcome = "failure",
            step = "jobs",
            error = "clear_optional"
        );
    }

    // 6. The outcome is stored on the record and written to the security log.
    log_expired_outcome(ports, job).await;
    counts.jobs_expired += 1;
    Ok(())
}

/// Write the `Expired` outcome on the job without raising an item; `false` when
/// the version changed under us.
async fn expire_record(
    ports: &Ports,
    job: &JobRecord,
    version: Version,
    now: OffsetDateTime,
) -> Result<bool, SvcError> {
    Ok(write_expired(ports, job, version, now).await?.is_some())
}

/// The conditional `Expired` write; `None` means `PreconditionFailed`.
async fn write_expired(
    ports: &Ports,
    job: &JobRecord,
    version: Version,
    now: OffsetDateTime,
) -> Result<Option<Version>, SvcError> {
    let mut record = job.clone();
    record.status = JobStatus::Expired;
    record.outcome = Some(JobOutcome {
        code: JobOutcomeCode::Expired,
        at: now,
    });
    record.target = None;
    record.expires_at = now + OUTCOME_RETENTION;
    match ports
        .store
        .jobs()
        .put(&record, Precondition::Matches(version))
        .await
    {
        Ok(v) => Ok(Some(v)),
        Err(StoreError::PreconditionFailed) => Ok(None),
        Err(_) => Err(SvcError::Store),
    }
}

/// Delete every Needs Attention item past its 30-day TTL.
async fn purge_needs_attention(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let ids = ports
        .store
        .needs_attention()
        .expires_by(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if ids.len() >= SWEEP_BATCH as usize {
        counts.more = true;
    }
    for id in ids {
        if delete_ignoring_conflict(
            ports
                .store
                .needs_attention()
                .delete(&id, Precondition::None)
                .await,
        )? {
            counts.needs_attention_deleted += 1;
        }
    }
    Ok(())
}

/// Delete every session past its expiry (idle, absolute or `pre_auth`).
async fn purge_sessions(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let ids = ports
        .store
        .sessions()
        .expires_by(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if ids.len() >= SWEEP_BATCH as usize {
        counts.more = true;
    }
    for id in ids {
        if delete_ignoring_conflict(ports.store.sessions().delete(&id, Precondition::None).await)? {
            counts.sessions_deleted += 1;
        }
    }
    Ok(())
}

/// Delete every classifier evaluation record past its 180-day retention.
async fn purge_evals(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let ids = ports
        .store
        .classifier_eval()
        .expires_by(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if ids.len() >= SWEEP_BATCH as usize {
        counts.more = true;
    }
    for id in ids {
        if delete_ignoring_conflict(
            ports
                .store
                .classifier_eval()
                .delete(&id, Precondition::None)
                .await,
        )? {
            counts.evals_deleted += 1;
        }
    }
    Ok(())
}

/// Delete every invite whose `purge_at` has passed.
async fn purge_invites(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let ids = ports
        .store
        .invites()
        .purge_due(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if ids.len() >= SWEEP_BATCH as usize {
        counts.more = true;
    }
    for id in ids {
        if delete_ignoring_conflict(ports.store.invites().delete(&id, Precondition::None).await)? {
            counts.invites_deleted += 1;
        }
    }
    Ok(())
}

/// Delete every rate-limit counter whose window has ended.
async fn purge_rate_limits(
    ports: &Ports,
    now: OffsetDateTime,
    counts: &mut SweepCounts,
) -> Result<(), SvcError> {
    let deleted = ports
        .store
        .rate_limits()
        .expires_by(now, SWEEP_BATCH)
        .await
        .map_err(|_| SvcError::Store)?;
    if deleted >= u64::from(SWEEP_BATCH) {
        counts.more = true;
    }
    counts.rate_limits_deleted += deleted;
    Ok(())
}

/// `Ok(true)` when the delete happened, `Ok(false)` on a lost race (a skip).
/// The result is consumed by the match, so it is taken by value.
#[allow(clippy::needless_pass_by_value)]
fn delete_ignoring_conflict(result: Result<(), StoreError>) -> Result<bool, SvcError> {
    match result {
        Ok(()) => Ok(true),
        Err(StoreError::PreconditionFailed) => Ok(false),
        Err(_) => Err(SvcError::Store),
    }
}

/// An https URL, or `None` for a mailto target or anything else (the item's
/// link is https only, S7 5.8).
fn https_link(target: &str) -> Option<Url> {
    let url = Url::parse(target).ok()?;
    (url.scheme() == "https").then_some(url)
}

/// One `unsub_job_outcome` security event and one `unsub_outcome` metric event
/// per expired job (UN-01 AC3). The user appears only as a pseudonym.
async fn log_expired_outcome(ports: &Ports, job: &JobRecord) {
    let pseudo = pseudo_id(ports, &job.user_id.0).await;
    obs::security_event(&SecurityEvent {
        action: "unsub_job_outcome",
        outcome: "expired",
        user: pseudo.clone(),
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: Some(method_str(job.method)),
    });
    metric_event(&obs::MetricEvent {
        event_type: "unsub_outcome",
        outcome: "expired",
        user: pseudo,
        provider: Some("gmail"),
    });
}

/// A security event that is not a job outcome (owner mismatch).
async fn log_security(ports: &Ports, job: &JobRecord, action: &'static str, outcome: &'static str) {
    obs::security_event(&SecurityEvent {
        action,
        outcome,
        user: pseudo_id(ports, &job.user_id.0).await,
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: Some(method_str(job.method)),
    });
}

/// The HMAC pseudonym of a user ID, or `None` when the key is unavailable.
async fn pseudo_id(ports: &Ports, user: &uuid::Uuid) -> Option<obs::PseudoId> {
    match ports.secrets.get(SecretName::LogPseudonymHmacKey).await {
        Ok(key) => Some(Pseudonymiser::new(key).pseudo_id(user)),
        Err(_) => None,
    }
}

/// Log a failed step by name and error kind only (no values).
fn log_step_failure(step: &'static str, e: &SvcError) {
    tracing::error!(
        event = "op",
        route = "worker.sweep",
        outcome = "failure",
        step = step,
        error = kind(e)
    );
}

/// The error kind as a static string; never the error's values.
fn kind(e: &SvcError) -> &'static str {
    match e {
        SvcError::Store => "store",
        SvcError::Crypto => "crypto",
        SvcError::NotFound => "not_found",
        SvcError::Invalid(_) => "invalid",
    }
}

/// The S3 job methods, exhaustive so a new variant forces a review.
fn method_str(method: JobMethod) -> &'static str {
    match method {
        JobMethod::OneClick => "one_click",
        JobMethod::Mailto => "mailto",
    }
}
