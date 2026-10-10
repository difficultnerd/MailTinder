//! Shared job helpers (T-601b).
//!
//! [`cancel_queued_job`] is the conditional `queued -> cancelled` write that
//! the undo race (T-606) and account deletion (T-803) reuse; the runner's claim
//! (T-701) is the other side of the same conditional write.

use domain::{apply, Applied, JobEvent, JobId, JobState, JobStatus};
use ports::{JobOutcome, JobOutcomeCode, JobRecord, Precondition, StoreError, TaskName};

use crate::error::ApiError;
use crate::state::AppState;

/// The Cloud Tasks task name for a job; T-605a schedules with the same name.
#[must_use]
pub fn task_name_for(job: &JobId) -> TaskName {
    TaskName::for_job(job)
}

/// What happened when a cancel was attempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelResult {
    /// The conditional write won and the Cloud Task is deleted.
    Cancelled,
    /// The runner claimed the job first, or it was already terminal; nothing
    /// changed and no task was touched.
    NotQueued(JobStatus),
    /// There is no such job.
    Missing,
}

/// Cancel a job that is still `queued` (S3, INV-2).
///
/// The write is conditional on the version read here, so a cancel can never
/// take a job the runner has already claimed (`queued -> running`, T-701), and
/// calling it twice is safe (T-606, T-803). The Cloud Task is deleted only
/// after that write succeeded. A `PreconditionFailed` is never retried: it
/// means someone else moved the job on, and the fresh status is reported back.
///
/// # Errors
///
/// `ApiError::Internal` on a store failure or when the scheduler is
/// unavailable (the job record is then already terminal, so the delivered task
/// finds nothing to run).
pub async fn cancel_queued_job(app: &AppState, job: &JobId) -> Result<CancelResult, ApiError> {
    let Some(versioned) = app.ports.store.jobs().get(job).await? else {
        return Ok(CancelResult::Missing);
    };
    if versioned.record.status != JobStatus::Queued {
        return Ok(CancelResult::NotQueued(versioned.record.status));
    }
    let now = app.ports.clock.now();
    let state = JobState {
        status: versioned.record.status,
        attempts: versioned.record.attempts,
        due_at: versioned.record.due_at,
        expires_at: versioned.record.expires_at,
    };
    // The domain owns the terminal retention rule (S3 terminal job, S10 6.3).
    match apply(state, JobEvent::Cancel { now }, &app.tunables) {
        Ok(Applied::Changed(next)) => {
            let record = JobRecord {
                status: next.status,
                outcome: Some(JobOutcome {
                    code: JobOutcomeCode::Cancelled,
                    at: now,
                }),
                // A terminal job carries no target (S5 jobs row).
                target: None,
                expires_at: next.expires_at,
                ..versioned.record
            };
            match app
                .ports
                .store
                .jobs()
                .put(&record, Precondition::Matches(versioned.version))
                .await
            {
                Ok(_) => {}
                Err(StoreError::PreconditionFailed) => {
                    // The runner claimed it between the read and the write.
                    return Ok(match app.ports.store.jobs().get(job).await? {
                        Some(latest) => CancelResult::NotQueued(latest.record.status),
                        None => CancelResult::Missing,
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
        // A terminal status, or an already-expired queued job: leave it alone.
        Ok(Applied::NoOp) | Err(_) => {
            return Ok(CancelResult::NotQueued(versioned.record.status));
        }
    }
    // Only now, with the write won, is the task safe to delete.
    match app.ports.scheduler.cancel(&task_name_for(job)).await {
        // `NotFound` and `AlreadyRunning` are normal outcomes, not errors: a
        // task that already fired finds a terminal job and does nothing.
        Ok(_) => Ok(CancelResult::Cancelled),
        Err(_) => Err(ApiError::Internal),
    }
}
