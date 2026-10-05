//! The shared `JobScheduler` contract suite. Any `JobScheduler`
//! implementation (the fake, the Cloud Tasks adapter) must pass it.
#![allow(
    clippy::many_single_char_names,
    clippy::wildcard_imports,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

use std::sync::Arc;

use async_trait::async_trait;
use domain::JobId;
use ports::{CancelOutcome, JobScheduler, TaskName};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A scheduler under test plus a control handle that simulates dispatch.
pub struct SchedulerTarget {
    pub scheduler: Arc<dyn JobScheduler>,
    pub control: Arc<dyn SchedulerControl>,
}

/// Simulates a dispatched attempt on a task (the fake's `start`/`finish`).
#[async_trait]
pub trait SchedulerControl: Send + Sync {
    /// Mark a pending task as running (a dispatch attempt).
    async fn mark_running(&self, task: &TaskName) -> Result<(), String>;
    /// Mark a running task as done.
    async fn mark_done(&self, task: &TaskName) -> Result<(), String>;
}

fn job(n: u8) -> JobId {
    JobId(Uuid::from_u128(0x9000 + u128::from(n)))
}

fn due() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap_or_else(|_| panic!("valid"))
}

/// Runs every contract case in order on a fresh scheduler, returning
/// `Err("<case>: <what failed>")` on the first failure.
///
/// # Errors
///
/// Returns `Err` with the name of the first failing case.
pub async fn job_scheduler<F, Fut>(make: F) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = SchedulerTarget>,
{
    // schedule returns the job's task name
    {
        let t = make().await;
        let j = job(1);
        let name = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule: {e:?}"))?;
        if name != TaskName::for_job(&j) {
            return Err("schedule returned wrong name".into());
        }
    }

    // scheduling the same job twice returns the same name and does not create
    // a second task
    {
        let t = make().await;
        let j = job(2);
        let n1 = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule1: {e:?}"))?;
        let n2 = t
            .scheduler
            .schedule(&j, due() + Duration::minutes(1))
            .await
            .map_err(|e| format!("schedule2: {e:?}"))?;
        if n1 != n2 {
            return Err("second schedule returned a different name".into());
        }
    }

    // cancel of a pending task is Cancelled
    {
        let t = make().await;
        let j = job(3);
        let name = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule: {e:?}"))?;
        let out = t
            .scheduler
            .cancel(&name)
            .await
            .map_err(|e| format!("cancel: {e:?}"))?;
        if out != CancelOutcome::Cancelled {
            return Err("cancel of pending should be Cancelled".into());
        }
    }

    // cancel again is NotFound
    {
        let t = make().await;
        let j = job(4);
        let name = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule: {e:?}"))?;
        t.scheduler
            .cancel(&name)
            .await
            .map_err(|e| format!("cancel1: {e:?}"))?;
        let out = t
            .scheduler
            .cancel(&name)
            .await
            .map_err(|e| format!("cancel2: {e:?}"))?;
        if out != CancelOutcome::NotFound {
            return Err("second cancel should be NotFound".into());
        }
    }

    // cancel of a running task is AlreadyRunning
    {
        let t = make().await;
        let j = job(5);
        let name = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule: {e:?}"))?;
        t.control
            .mark_running(&name)
            .await
            .map_err(|e| format!("mark_running: {e}"))?;
        let out = t
            .scheduler
            .cancel(&name)
            .await
            .map_err(|e| format!("cancel: {e:?}"))?;
        if out != CancelOutcome::AlreadyRunning {
            return Err("cancel of running should be AlreadyRunning".into());
        }
    }

    // cancel of a done task is NotFound
    {
        let t = make().await;
        let j = job(6);
        let name = t
            .scheduler
            .schedule(&j, due())
            .await
            .map_err(|e| format!("schedule: {e:?}"))?;
        t.control
            .mark_running(&name)
            .await
            .map_err(|e| format!("mark_running: {e}"))?;
        t.control
            .mark_done(&name)
            .await
            .map_err(|e| format!("mark_done: {e}"))?;
        let out = t
            .scheduler
            .cancel(&name)
            .await
            .map_err(|e| format!("cancel: {e:?}"))?;
        if out != CancelOutcome::NotFound {
            return Err("cancel of done should be NotFound".into());
        }
    }

    // cancel of an unknown name is NotFound
    {
        let t = make().await;
        let out = t
            .scheduler
            .cancel(&TaskName("job-unknown".to_owned()))
            .await
            .map_err(|e| format!("cancel: {e:?}"))?;
        if out != CancelOutcome::NotFound {
            return Err("cancel of unknown should be NotFound".into());
        }
    }

    Ok(())
}
