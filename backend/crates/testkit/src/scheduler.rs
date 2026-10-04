//! A recording job scheduler fake.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use domain::JobId;
use ports::{CancelOutcome, JobScheduler, SchedError, TaskName};
use time::OffsetDateTime;

/// The state of a scheduled task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Deleted,
}

/// A recorded scheduler event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerEvent {
    Scheduled(TaskName, OffsetDateTime),
    Cancelled(TaskName, CancelOutcome),
}

/// A fake scheduler with no timers; tests drive it with `due`.
pub struct FakeJobScheduler {
    tasks: Mutex<BTreeMap<TaskName, (JobId, OffsetDateTime, TaskState)>>,
    log: Mutex<Vec<SchedulerEvent>>,
}

impl FakeJobScheduler {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(BTreeMap::new()),
            log: Mutex::new(Vec::new()),
        }
    }

    /// Pending tasks with `due_at <= now`.
    pub fn due(&self, now: OffsetDateTime) -> Vec<(TaskName, JobId)> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"));
        tasks
            .iter()
            .filter(|(_, (_, due, state))| *state == TaskState::Pending && *due <= now)
            .map(|(name, (job, _, _))| (name.clone(), *job))
            .collect()
    }

    /// Pending -> Running.
    pub fn start(&self, task: &TaskName) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"));
        if let Some((_, _, state)) = tasks.get_mut(task) {
            if *state == TaskState::Pending {
                *state = TaskState::Running;
            }
        }
    }

    /// Running -> Done.
    pub fn finish(&self, task: &TaskName) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"));
        if let Some((_, _, state)) = tasks.get_mut(task) {
            if *state == TaskState::Running {
                *state = TaskState::Done;
            }
        }
    }

    pub fn events(&self) -> Vec<SchedulerEvent> {
        self.log
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"))
            .clone()
    }
}

impl Default for FakeJobScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JobScheduler for FakeJobScheduler {
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError> {
        let name = TaskName::for_job(job);
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"));
        if !tasks.contains_key(&name) {
            tasks.insert(name.clone(), (*job, due_at, TaskState::Pending));
            self.log
                .lock()
                .unwrap_or_else(|_| panic!("scheduler poisoned"))
                .push(SchedulerEvent::Scheduled(name.clone(), due_at));
        }
        Ok(name)
    }

    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"));
        let outcome = match tasks.get_mut(task).map(|(_, _, s)| s.clone()) {
            Some(TaskState::Pending) => {
                if let Some(entry) = tasks.get_mut(task) {
                    entry.2 = TaskState::Deleted;
                }
                CancelOutcome::Cancelled
            }
            Some(TaskState::Running) => CancelOutcome::AlreadyRunning,
            _ => CancelOutcome::NotFound,
        };
        self.log
            .lock()
            .unwrap_or_else(|_| panic!("scheduler poisoned"))
            .push(SchedulerEvent::Cancelled(task.clone(), outcome));
        Ok(outcome)
    }
}
