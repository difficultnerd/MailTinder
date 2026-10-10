//! `run_job`: the internal route's work (T-701, S7 5.12 API-INT-1).
//!
//! One Cloud Tasks delivery runs one unsubscribe job: load it, re-check
//! ownership, claim it with a conditional write, then hand it to the sender
//! for its method. Every outcome is stored on the job record, written to the
//! security log and emitted as a metric event. The job never re-queues itself;
//! Cloud Tasks is the only retry layer (S3, UN-05 AC2).
//!
//! A terminal transition resets `expires_at` to `now + 30 days`, so a job
//! keeps its outcome for the Feed while JOB-1 still holds (S3, trap 4).

use std::sync::Arc;

use domain::{
    apply, batched_with_sent_sibling, Applied, JobEvent, JobId, JobMethod, JobState, JobStatus,
    NeedsAttentionReason, Tunables,
};
use obs::{metric_event, op_log, OpLog, Pseudonymiser, SecurityEvent, Sensitive};
use ports::store::{JobOutcome, JobOutcomeCode, JobRecord, Precondition, StoreError, Version};
use ports::{Ports, SecretName, TaskName};
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use svc_common::needs_attention::{self, NewItem};
use svc_common::SvcError;
use time::OffsetDateTime;
use url::Url;

use crate::sender::{ClaimedJob, SendResult, UnsubSender};

/// Cloud Tasks `maxAttempts` (S7 5.12) `[TUNABLE in queue config]`.
pub const MAX_ATTEMPTS: u32 = 4;
/// How long a terminal job's outcome is kept (S3, S5 `jobs` row) `[TUNABLE]`.
pub const OUTCOME_RETENTION: time::Duration = time::Duration::days(30);
/// The sender display used when a job carries none (trap 3).
const DEFAULT_SENDER_DISPLAY: &str = "this sender";

/// Everything the internal route needs.
#[derive(Clone)]
pub struct UnsubState {
    pub ports: Ports,
    pub senders: Vec<Arc<dyn UnsubSender>>,
    pub auth: InternalAuthConfig,
}

/// Where this delivery sits in Cloud Tasks' retry schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// `X-CloudTasks-TaskRetryCount`; 0 on the first delivery.
    pub attempt: u32,
}

impl Delivery {
    /// Parse the retry-count header; anything missing or malformed is 0.
    #[must_use]
    pub fn from_retry_count(header: Option<&str>) -> Self {
        let attempt = header
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);
        Self { attempt }
    }
}

/// What the handler answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunResponse {
    /// `200`: the delivery is complete (sent, batched, terminal, or nothing
    /// to do).
    Done,
    /// `503`: a transient failure; Cloud Tasks should try again.
    Retry,
}

/// A failure running a job. The handler answers `503` for every variant, so a
/// transient store or key failure never loses a job.
#[derive(Debug, thiserror::Error)]
pub enum UnsubError {
    #[error("store")]
    Store(#[from] StoreError),
    #[error("service")]
    Svc(#[from] SvcError),
    #[error("domain")]
    Domain(#[from] domain::DomainError),
}

/// Run one delivery of `job_id`.
#[allow(clippy::too_many_lines)]
pub async fn run_job(
    state: &UnsubState,
    job_id: &JobId,
    delivery: Delivery,
) -> Result<RunResponse, UnsubError> {
    let ports = &state.ports;
    let tunables = Tunables::default();
    let now = ports.clock.now();

    // 4. Load the job. Missing means undo raced the task (S7 5.12).
    let Some(versioned) = ports.store.jobs().get(job_id).await? else {
        return Ok(RunResponse::Done);
    };
    let mut job = versioned.record;
    let mut version = versioned.version;

    // A delivery for a job a successful undo has already cancelled (S10 8):
    // the undo removed the swipe and cancelled the job *before* this delivery
    // sent anything, so no unsubscribe left the server. This is the designed,
    // harmless race, never an unrecoverable action, so it is logged at `op`
    // level and not as the A1 metric `unsub_after_undo` (which pages on any
    // event). The job carries `Cancelled` only when a cancel won the race (the
    // runner's own mailbox-removed path writes `MailboxRemoved`), so this
    // cannot be confused with a job that was never queued.
    if job
        .outcome
        .as_ref()
        .is_some_and(|outcome| outcome.code == JobOutcomeCode::Cancelled)
    {
        op_log(&OpLog {
            op: "unsub.run",
            outcome: "cancelled",
            status: Some(200),
            latency_ms: None,
        });
        return Ok(RunResponse::Done);
    }

    // 5. Ownership re-check from the mailbox record (ASVS V8.3.1).
    match ports.store.mailboxes().get(&job.mailbox_id).await? {
        None => {
            finish(
                state,
                &mut job,
                &mut version,
                JobStatus::Cancelled,
                JobOutcomeCode::MailboxRemoved,
                now,
                None,
                None,
                None,
            )
            .await?;
            return Ok(RunResponse::Done);
        }
        Some(mailbox) if mailbox.record.user_id != job.user_id => {
            log_outcome(ports, &job, JobOutcomeCode::OwnerMismatch).await;
            log_security(ports, &job, "job_owner_mismatch", "forbidden").await;
            finish(
                state,
                &mut job,
                &mut version,
                JobStatus::Failed,
                JobOutcomeCode::OwnerMismatch,
                now,
                None,
                None,
                None,
            )
            .await?;
            return Ok(RunResponse::Done);
        }
        Some(_) => {}
    }

    // 6. Expiry: a job past its deadline ends `expired` with an item so it
    // never fails silently (S3, INV-2).
    if !job.status.is_terminal() && now > job.expires_at {
        let display = job_record::open_sender_display(ports, &job.user_id, &job)
            .await
            .ok();
        let link = job_link(ports, &job).await;
        finish(
            state,
            &mut job,
            &mut version,
            JobStatus::Expired,
            JobOutcomeCode::Expired,
            now,
            Some(NeedsAttentionReason::JobExpired),
            display,
            link,
        )
        .await?;
        return Ok(RunResponse::Done);
    }

    // 7. Too early: guards the undo window.
    if now < job.due_at {
        return Ok(RunResponse::Retry);
    }

    // 8. Claim with a single conditional write on the version read above.
    match apply(
        job_state(&job),
        JobEvent::Claim {
            attempt: delivery.attempt,
            now,
        },
        &tunables,
    )? {
        // Terminal, or a duplicate delivery (UN-01 AC1).
        Applied::NoOp => return Ok(RunResponse::Done),
        Applied::Changed(next) => {
            job.status = next.status;
            job.attempts = next.attempts;
            job.expires_at = next.expires_at;
            match ports
                .store
                .jobs()
                .put(&job, Precondition::Matches(version))
                .await
            {
                Ok(v) => version = v,
                Err(StoreError::PreconditionFailed) => {
                    // Undo cancelled it, or another delivery won.
                    let re = ports.store.jobs().get(job_id).await?;
                    return Ok(match re {
                        Some(r) if r.record.status.is_terminal() => RunResponse::Done,
                        _ => RunResponse::Retry,
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // 9. Batching: another job for this list already sent (UN-01 AC2).
    let siblings = ports
        .store
        .jobs()
        .by_user_with_outcome(&job.user_id, 500)
        .await?;
    let pairs: Vec<(JobStatus, bool)> = siblings
        .iter()
        .filter(|v| v.record.job_id != job.job_id)
        .map(|v| (v.record.status, v.record.list_key_hash == job.list_key_hash))
        .collect();
    if batched_with_sent_sibling(&pairs) {
        finish(
            state,
            &mut job,
            &mut version,
            JobStatus::Sent,
            JobOutcomeCode::Batched,
            now,
            None,
            None,
            None,
        )
        .await?;
        return Ok(RunResponse::Done);
    }

    // 10. Decrypt the target and sender display.
    let Ok(target) = job_record::open_target(ports, &job.user_id, &job).await else {
        finish(
            state,
            &mut job,
            &mut version,
            JobStatus::NeedsAttention,
            JobOutcomeCode::Refused,
            now,
            Some(NeedsAttentionReason::UnsubscribeFailed),
            None,
            None,
        )
        .await?;
        return Ok(RunResponse::Done);
    };
    let sender_display = job_record::open_sender_display(ports, &job.user_id, &job)
        .await
        .ok()
        .unwrap_or_else(|| Sensitive::new(DEFAULT_SENDER_DISPLAY.to_owned()));
    let link = https_link(target.expose());

    let Some(sender) = state.senders.iter().find(|s| s.method() == job.method) else {
        finish(
            state,
            &mut job,
            &mut version,
            JobStatus::NeedsAttention,
            JobOutcomeCode::Refused,
            now,
            Some(NeedsAttentionReason::UnsubscribeFailed),
            Some(sender_display),
            link,
        )
        .await?;
        return Ok(RunResponse::Done);
    };

    // 11. Send. A sender that needs the mailbox mints its own token.
    let claimed = ClaimedJob {
        job: job.clone(),
        target,
        sender_display: sender_display.clone(),
    };
    let result = sender.send(ports, &claimed).await;

    // 12. Map the result.
    match result {
        SendResult::Sent { code } => {
            finish(
                state,
                &mut job,
                &mut version,
                JobStatus::Sent,
                code,
                now,
                None,
                Some(sender_display),
                None,
            )
            .await?;
            batch_siblings(state, &job, now).await?;
            Ok(RunResponse::Done)
        }
        SendResult::Retryable { .. } => {
            if delivery.attempt + 1 >= MAX_ATTEMPTS {
                finish(
                    state,
                    &mut job,
                    &mut version,
                    JobStatus::NeedsAttention,
                    JobOutcomeCode::RetriesExhausted,
                    now,
                    Some(NeedsAttentionReason::UnsubscribeFailed),
                    Some(sender_display),
                    link,
                )
                .await?;
                Ok(RunResponse::Done)
            } else {
                // Leave the job `Running` (attempts already recorded) and let
                // Cloud Tasks try again. It never schedules itself.
                Ok(RunResponse::Retry)
            }
        }
        SendResult::NeedsAttention { reason, code } => {
            finish(
                state,
                &mut job,
                &mut version,
                JobStatus::NeedsAttention,
                code,
                now,
                Some(reason),
                Some(sender_display),
                link,
            )
            .await?;
            Ok(RunResponse::Done)
        }
        SendResult::TokenRevoked => {
            finish(
                state,
                &mut job,
                &mut version,
                JobStatus::Failed,
                JobOutcomeCode::TokenInvalid,
                now,
                Some(NeedsAttentionReason::SignInRequired),
                Some(sender_display),
                link,
            )
            .await?;
            Ok(RunResponse::Done)
        }
    }
}

/// Move every still-queued sibling of this list to `Sent` with `Batched` and
/// cancel its Cloud Tasks task (UN-01 AC2). A conflict on one sibling is
/// skipped, never retried.
async fn batch_siblings(
    state: &UnsubState,
    job: &JobRecord,
    now: OffsetDateTime,
) -> Result<(), UnsubError> {
    let queued = state
        .ports
        .store
        .jobs()
        .queued_for_list(&job.user_id, &job.list_key_hash)
        .await?;
    for sibling in queued {
        if sibling.record.job_id == job.job_id {
            continue;
        }
        let mut record = sibling.record;
        record.status = JobStatus::Sent;
        record.outcome = Some(JobOutcome {
            code: JobOutcomeCode::Batched,
            at: now,
        });
        record.target = None;
        record.sender_display = None;
        record.expires_at = now + OUTCOME_RETENTION;
        if state
            .ports
            .store
            .jobs()
            .put(&record, Precondition::Matches(sibling.version))
            .await
            .is_ok()
        {
            // AlreadyRunning and not-found are both fine.
            let _ = state
                .ports
                .scheduler
                .cancel(&TaskName::for_job(&record.job_id))
                .await;
        }
    }
    Ok(())
}

/// The step 13 terminal transition: one conditional update to `status` with
/// `code`, clearing `target`; then the item (when `reason` is set); then a
/// second update clearing `sender_display`. The item is written first so a
/// crash leaves a retryable job rather than a silent one.
#[allow(clippy::too_many_arguments)]
async fn finish(
    state: &UnsubState,
    job: &mut JobRecord,
    version: &mut Version,
    status: JobStatus,
    code: JobOutcomeCode,
    now: OffsetDateTime,
    reason: Option<NeedsAttentionReason>,
    sender_display: Option<Sensitive<String>>,
    link: Option<Url>,
) -> Result<(), UnsubError> {
    job.status = status;
    job.outcome = Some(JobOutcome { code, at: now });
    job.target = None;
    job.expires_at = now + OUTCOME_RETENTION;
    *version = state
        .ports
        .store
        .jobs()
        .put(job, Precondition::Matches(version.clone()))
        .await?;

    if let Some(reason) = reason {
        let display = sender_display
            .as_ref()
            .map_or_else(|| DEFAULT_SENDER_DISPLAY.to_owned(), |s| s.expose().clone());
        needs_attention::raise_item(
            &state.ports,
            NewItem {
                user: &job.user_id,
                mailbox: &job.mailbox_id,
                sender_display: &display,
                link: link.as_ref(),
                reason,
            },
        )
        .await?;
    }

    job.sender_display = None;
    *version = state
        .ports
        .store
        .jobs()
        .put(job, Precondition::Matches(version.clone()))
        .await?;

    log_outcome(&state.ports, job, code).await;
    Ok(())
}

/// The lifecycle view of a job record, for the T-106 state machine.
fn job_state(job: &JobRecord) -> JobState {
    JobState {
        status: job.status,
        attempts: job.attempts,
        due_at: job.due_at,
        expires_at: job.expires_at,
    }
}

/// An https URL, or `None` for a mailto target or anything else (the item's
/// link is https only, S7 5.8).
fn https_link(target: &str) -> Option<Url> {
    let url = Url::parse(target).ok()?;
    (url.scheme() == "https").then_some(url)
}

/// The decrypted target parsed as https, for an item raised before the target
/// was cleared.
async fn job_link(ports: &Ports, job: &JobRecord) -> Option<Url> {
    match job_record::open_target(ports, &job.user_id, job).await {
        Ok(target) => https_link(target.expose()),
        Err(_) => None,
    }
}

/// One `unsub_job_outcome` security event and one `unsub_outcome` metric event
/// per terminal outcome. The user appears only as a pseudonym.
async fn log_outcome(ports: &Ports, job: &JobRecord, code: JobOutcomeCode) {
    let pseudo = pseudo_id(ports, &job.user_id.0).await;
    let outcome = outcome_code_str(code);
    obs::security_event(&SecurityEvent {
        action: "unsub_job_outcome",
        outcome,
        user: pseudo.clone(),
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: Some(method_str(job.method)),
    });
    metric_event(&obs::MetricEvent {
        event_type: "unsub_outcome",
        outcome,
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

/// The S3 outcome codes, exhaustive so a new variant forces a review.
fn outcome_code_str(code: JobOutcomeCode) -> &'static str {
    match code {
        JobOutcomeCode::OneClickAccepted => "one_click_accepted",
        JobOutcomeCode::MailtoSent => "mailto_sent",
        JobOutcomeCode::Redirected => "redirected",
        JobOutcomeCode::AddressRefused => "address_refused",
        JobOutcomeCode::HttpRejected => "http_rejected",
        JobOutcomeCode::TimedOut => "timed_out",
        JobOutcomeCode::RetriesExhausted => "retries_exhausted",
        JobOutcomeCode::TokenInvalid => "token_invalid",
        JobOutcomeCode::Expired => "expired",
        JobOutcomeCode::Cancelled => "cancelled",
        JobOutcomeCode::Batched => "batched",
        JobOutcomeCode::MailboxRemoved => "mailbox_removed",
        JobOutcomeCode::OwnerMismatch => "owner_mismatch",
        JobOutcomeCode::Refused => "refused",
    }
}

/// The S3 job methods, exhaustive (trap 9).
fn method_str(method: JobMethod) -> &'static str {
    match method {
        JobMethod::OneClick => "one_click",
        JobMethod::Mailto => "mailto",
    }
}
