//! The `JobScheduler` port: deferred background work.

use async_trait::async_trait;
use domain::JobId;
use time::OffsetDateTime;

/// A scheduled task name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskName(pub String);

impl TaskName {
    /// `job-` + simple (no hyphens) UUID, lower case.
    pub fn for_job(job: &JobId) -> TaskName {
        TaskName(format!("job-{}", job.0.simple()))
    }
}

/// The outcome of cancelling a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled,
    NotFound,
    AlreadyRunning,
}

/// A scheduler error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchedError {
    #[error("scheduler unavailable")]
    Unavailable,
    #[error("rejected: {0}")]
    Rejected(&'static str),
}

/// Schedules and cancels deferred jobs.
#[async_trait]
pub trait JobScheduler: Send + Sync {
    /// Idempotent per job.
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError>;
    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError>;
}
