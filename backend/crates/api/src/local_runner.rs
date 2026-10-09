//! The e2e-only local job runner (T-1112b).
//!
//! A [`JobScheduler`] that keeps queued jobs in memory and, in a background
//! loop, POSTs each one to the local `unsub` internal route
//! (`POST /internal/v1/unsubscribe-jobs/{job_id}/run`, S7 5.12 API-INT-1) once
//! its `due_at` has passed. It is compiled only with the `testkit` feature;
//! production keeps Cloud Tasks (`CloudTasksScheduler`).
//!
//! The undo window is not the runner's to set: the api computes
//! `due_at = now + tunables.unsub_delay` and passes it here (S2 `UNSUB_DELAY`).
//! The runner only delivers when `due_at` passes. For a demo that must not wait
//! five real minutes, `MT_E2E_TIME_SCALE` divides every wait the runner owns:
//! the loop's poll delay, the request timeout and the retry back-off
//! (`time_scale = 1.0` is real time; the e2e demo sets it large).
//!
//! Delivery goes to exactly the configured socket and only when that host is a
//! loopback literal, so an e2e build can never be pointed at a real `unsub`
//! (or anywhere else): a non-loopback target is refused at construction.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use domain::JobId;
use ports::{CancelOutcome, Clock, JobScheduler, SchedError, TaskName};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use time::OffsetDateTime;
use tokio::sync::Notify;
use url::Url;

/// The production Cloud Tasks back-off table, in seconds: `minBackoff` 30 s,
/// doubling, capped at `maxBackoff` 5 min, four attempts in total (S7 5.12,
/// S4 step 4). The entries are the delays before the 2nd, 3rd and 4th attempt;
/// after the table is exhausted the job is failed (no silent infinite retry).
pub const BACKOFF_SECS: [u64; 3] = [30, 60, 120];

/// The path prefix of `unsub`'s one internal route. The job id is appended,
/// then `/run`.
const RUN_PATH_PREFIX: &str = "/internal/v1/unsubscribe-jobs/";

/// How long the loop waits between due-time checks (before scaling). Small: it
/// only bounds delivery latency, it is not a back-off.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The one-click request timeout (S7 5.12), before scaling.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The retry-count header `unsub` reads (`X-CloudTasks-TaskRetryCount`), so the
/// runner reproduces a Cloud Tasks dispatch byte-for-byte.
const RETRY_COUNT_HEADER: &str = "X-CloudTasks-TaskRetryCount";

/// Why a [`LocalJobRunner`] could not be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RunnerError {
    /// The configured target has no host, or its host is not an IP literal.
    #[error("target URL has no IP-literal host")]
    InvalidTarget,
    /// The configured target is not loopback; e2e must never reach a real host.
    #[error("target host is not loopback")]
    NotLoopback,
    /// The HTTP client could not be built.
    #[error("HTTP client could not be built")]
    Client,
}

/// Configuration for the local runner, read from the environment by the e2e
/// start-up (`MT_E2E`). `unsub_base` is `UNSUB_BASE_URL`, `caller_token` is the
/// test identity (`UNSUB_E2E_CALLER_TOKEN`) and `time_scale` is
/// `MT_E2E_TIME_SCALE`.
///
/// No `Debug`: the token must never be formatted into a log.
#[derive(Clone)]
pub struct LocalJobRunnerConfig {
    /// `unsub`'s base URL; its host must be loopback.
    pub unsub_base: Url,
    /// The bearer token `unsub`'s e2e caller verifier accepts.
    pub caller_token: String,
    /// Divides every wait (default `1.0`); larger runs the clock faster.
    pub time_scale: f64,
}

/// A state a tracked job can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Waiting for its `due_at`.
    Pending,
    /// Being delivered; not cancellable (matches Cloud Tasks `AlreadyRunning`).
    Running,
    /// Delivered with a 2xx.
    Done,
    /// A 4xx, or the back-off table exhausted.
    Failed,
}

/// The delivery state of a tracked job, surfaced for the e2e proof (T-1112c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    /// Waiting for its `due_at`.
    Pending,
    /// Being delivered.
    Running,
    /// Delivered with a 2xx.
    Done,
    /// A 4xx, or the back-off table exhausted.
    Failed,
}

/// One tracked job.
struct Entry {
    job: JobId,
    due_at: OffsetDateTime,
    attempt: u32,
    state: State,
}

/// The shared job table.
struct Inner {
    jobs: HashMap<TaskName, Entry>,
}

/// An in-memory [`JobScheduler`] with a loop that delivers due jobs to a local
/// `unsub`.
///
/// Call [`LocalJobRunner::run`] (typically via `tokio::spawn`) to start
/// delivering; until then jobs only accumulate.
pub struct LocalJobRunner {
    base: Url,
    token: String,
    scale: f64,
    clock: Arc<dyn Clock>,
    client: reqwest::Client,
    inner: Arc<Mutex<Inner>>,
    /// Wakes the loop when a job is scheduled, so a due job is delivered
    /// promptly rather than at the next poll tick.
    notify: Arc<Notify>,
}

impl LocalJobRunner {
    /// Build a runner against a loopback `unsub`, reading "now" from `clock`
    /// (S10 1 rule 2: time comes only from the `Clock` port).
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::InvalidTarget`] when `unsub_base` has no
    /// IP-literal host, [`RunnerError::NotLoopback`] when the host is not
    /// loopback, and [`RunnerError::Client`] when the HTTP client cannot be
    /// built. `time_scale` that is non-finite or not positive falls back to
    /// `1.0`.
    pub fn new(cfg: LocalJobRunnerConfig, clock: Arc<dyn Clock>) -> Result<Self, RunnerError> {
        let host = cfg
            .unsub_base
            .host_str()
            .ok_or(RunnerError::InvalidTarget)?;
        let ip: IpAddr = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host)
            .parse()
            .map_err(|_| RunnerError::InvalidTarget)?;
        if !ip.is_loopback() {
            return Err(RunnerError::NotLoopback);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RunnerError::Client)?;
        let scale = if cfg.time_scale.is_finite() && cfg.time_scale > 0.0 {
            cfg.time_scale
        } else {
            1.0
        };
        Ok(Self {
            base: cfg.unsub_base,
            token: cfg.caller_token,
            scale,
            clock,
            client,
            inner: Arc::new(Mutex::new(Inner {
                jobs: HashMap::new(),
            })),
            notify: Arc::new(Notify::new()),
        })
    }

    /// The scaled form of `base`: `base / time_scale`.
    fn scaled(&self, base: Duration) -> Duration {
        Duration::from_secs_f64(base.as_secs_f64() / self.scale)
    }

    /// The `unsub` run URL for a job.
    fn run_url(&self, job: &JobId) -> Url {
        let mut url = self.base.clone();
        url.set_path(&format!("{RUN_PATH_PREFIX}{}/run", job.0.simple()));
        url
    }

    /// Run the delivery loop until the future is dropped (abort the task).
    ///
    /// Each tick it delivers every job whose `due_at` has passed, then sleeps
    /// for one scaled poll interval or until a new job is scheduled.
    pub async fn run(&self) {
        loop {
            for (name, job, attempt) in self.take_due() {
                self.deliver(&name, &job, attempt).await;
            }
            tokio::select! {
                () = tokio::time::sleep(self.scaled(POLL_INTERVAL)) => {}
                () = self.notify.notified() => {}
            }
        }
    }

    /// Claim every pending job whose `due_at` has passed, marking it running so
    /// it is delivered once even if the loop ticks again.
    fn take_due(&self) -> Vec<(TaskName, JobId, u32)> {
        let now = self.clock.now();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        for (name, entry) in &mut inner.jobs {
            if entry.state == State::Pending && entry.due_at <= now {
                entry.state = State::Running;
                out.push((name.clone(), entry.job, entry.attempt));
            }
        }
        out
    }

    /// Deliver one job and record the outcome, rescheduling on a retryable
    /// failure.
    async fn deliver(&self, name: &TaskName, job: &JobId, attempt: u32) {
        let outcome = self.post(job, attempt).await;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = inner.jobs.get_mut(name) else {
            return;
        };
        match outcome {
            Ok(status) if (200..300).contains(&status) => entry.state = State::Done,
            // Any other definitive client error is not retried (Behaviour 2).
            Ok(status) if (400..500).contains(&status) => entry.state = State::Failed,
            // 5xx, timeout or connection error: retry with scaled back-off.
            _ => match BACKOFF_SECS.get(attempt as usize) {
                Some(secs) => {
                    entry.attempt += 1;
                    entry.due_at = self.clock.now() + self.scaled(Duration::from_secs(*secs));
                    entry.state = State::Pending;
                }
                None => entry.state = State::Failed,
            },
        }
    }

    /// POST the job's run route and return the response status. The token is
    /// passed as a build-time checked header value; it is never logged.
    async fn post(&self, job: &JobId, attempt: u32) -> Result<u16, reqwest::Error> {
        let response = self
            .client
            .post(self.run_url(job))
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(RETRY_COUNT_HEADER, attempt.to_string())
            .timeout(self.scaled(REQUEST_TIMEOUT))
            .body("{}")
            .send()
            .await?;
        Ok(response.status().as_u16())
    }

    /// The delivery state of `job`, or `None` when it is not tracked (never
    /// scheduled, or cancelled and removed). Observability for the e2e proof
    /// (T-1112c); the delivery loop uses the private state directly.
    #[must_use]
    pub fn delivery_state(&self, job: &JobId) -> Option<DeliveryState> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .jobs
            .get(&TaskName::for_job(job))
            .map(|entry| match entry.state {
                State::Pending => DeliveryState::Pending,
                State::Running => DeliveryState::Running,
                State::Done => DeliveryState::Done,
                State::Failed => DeliveryState::Failed,
            })
    }
}

#[async_trait]
impl JobScheduler for LocalJobRunner {
    /// Idempotent per job, like Cloud Tasks: a second schedule for the same job
    /// is a success and does not reset the due time.
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError> {
        let name = TaskName::for_job(job);
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.jobs.entry(name.clone()).or_insert(Entry {
                job: *job,
                due_at,
                attempt: 0,
                state: State::Pending,
            });
        }
        self.notify.notify_one();
        Ok(name)
    }

    /// Cancel (undo). A pending job is removed and never delivered; a running
    /// one cannot be stopped; anything else is not found.
    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = match inner.jobs.get(task).map(|e| e.state) {
            Some(State::Pending) => {
                inner.jobs.remove(task);
                CancelOutcome::Cancelled
            }
            Some(State::Running) => CancelOutcome::AlreadyRunning,
            _ => CancelOutcome::NotFound,
        };
        Ok(outcome)
    }
}
