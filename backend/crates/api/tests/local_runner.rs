//! T-1112b: the e2e-only local job runner.
//!
//! Every test starts a tiny `axum` server on a free loopback port as the
//! delivery target and runs the runner's loop in a background task. The whole
//! file needs the `testkit` feature (`reqwest` and `startup_e2e`); without it
//! it compiles to nothing.
#![cfg(feature = "testkit")]
#![allow(clippy::too_many_lines)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adapters_gcp::SystemClock;
use api::local_runner::{LocalJobRunner, LocalJobRunnerConfig, RunnerError};
use api::startup_e2e::e2e_tunables;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use domain::{
    plan_swipe, Classification, HeaderFacts, JobId, LabelSet, MailboxId, MessageClass, MessageId,
    MessageMeta, RuleId, SenderKey, SenderStats, SwipeAction, SwipeIds, SwipeInput, Tunables,
    UnsubscribeOptions,
};
use ports::{Clock, JobScheduler};
use time::OffsetDateTime;
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

/// A boxed error for the tests; every `?` converts into it.
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// The recording delivery target: a status per request, plus what the runner
/// sent.
#[derive(Default)]
struct Target {
    /// The scripted response statuses; once empty every request returns 200.
    script: Mutex<VecDeque<u16>>,
    /// The status returned for each request, in order (the assertion surface).
    statuses: Mutex<Vec<u16>>,
    /// The `Authorization` header of each request.
    auth: Mutex<Vec<String>>,
    /// The `X-CloudTasks-TaskRetryCount` header of each request.
    retries: Mutex<Vec<String>>,
    /// When each request arrived, for back-off timing checks.
    times: Mutex<Vec<Instant>>,
}

impl Target {
    fn script(&self, statuses: impl IntoIterator<Item = u16>) {
        let mut script = self
            .script
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        script.extend(statuses);
    }

    fn count(&self) -> usize {
        self.statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn statuses(&self) -> Vec<u16> {
        self.statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn retries(&self) -> Vec<String> {
        self.retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn first_auth(&self) -> Option<String> {
        self.auth
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
    }

    /// The gap between the first two requests, if both arrived.
    fn first_gap(&self) -> Option<Duration> {
        let times = self
            .times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match (times.first(), times.get(1)) {
            (Some(first), Some(second)) => Some(second.duration_since(*first)),
            _ => None,
        }
    }
}

/// The delivery target handler: record the request, then answer with the next
/// scripted status (200 by default).
async fn handle(State(target): State<Arc<Target>>, headers: HeaderMap) -> Response {
    let status = target
        .script
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop_front()
        .unwrap_or(200);
    target
        .statuses
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(status);
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    target
        .auth
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(header("authorization"));
    target
        .retries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(header("x-cloudtasks-taskretrycount"));
    target
        .times
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Instant::now());
    StatusCode::from_u16(status)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        .into_response()
}

/// Bind a target on a free loopback port and serve it in the background.
async fn spawn_target(target: Arc<Target>) -> Result<SocketAddr, std::io::Error> {
    let app = Router::new()
        .route("/internal/v1/unsubscribe-jobs/:job_id/run", post(handle))
        .with_state(target);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(addr)
}

/// Build a runner against `base` (all tests use the literal e2e token), reading
/// "now" from the real `Clock` port.
fn runner(base: &Url, time_scale: f64) -> Result<LocalJobRunner, RunnerError> {
    LocalJobRunner::new(
        LocalJobRunnerConfig {
            unsub_base: base.clone(),
            caller_token: "test-e2e-caller-token".to_owned(),
            time_scale,
        },
        Arc::new(SystemClock),
    )
}

/// The current wall time from the `Clock` port (only the port may read time).
fn now() -> OffsetDateTime {
    SystemClock.now()
}

/// Poll `cond` until it holds or `timeout` elapses.
async fn wait_for(cond: impl Fn() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

#[tokio::test]
async fn local_runner_delivers_a_due_job_once() -> TestResult {
    let target = Arc::new(Target::default());
    let addr = spawn_target(Arc::clone(&target)).await?;
    let base = Url::parse(&format!("http://{addr}"))?;
    let running = Arc::new(runner(&base, 1.0)?);

    let job = JobId(Uuid::new_v4());
    running
        .schedule(&job, now())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let handle = tokio::spawn({
        let running = Arc::clone(&running);
        async move { running.run().await }
    });

    assert!(
        wait_for(|| target.count() >= 1, Duration::from_secs(3)).await,
        "the due job was not delivered"
    );
    // Give any wrong extra delivery time to happen.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        target.count(),
        1,
        "a due job must be delivered exactly once"
    );
    assert_eq!(
        target.first_auth().as_deref(),
        Some("Bearer test-e2e-caller-token")
    );
    assert_eq!(target.retries().first().map(String::as_str), Some("0"));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn local_runner_does_not_deliver_a_cancelled_job() -> TestResult {
    let target = Arc::new(Target::default());
    let addr = spawn_target(Arc::clone(&target)).await?;
    let base = Url::parse(&format!("http://{addr}"))?;
    let running = Arc::new(runner(&base, 1.0)?);

    let job = JobId(Uuid::new_v4());
    let name = running
        .schedule(&job, now())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    // Cancel before the loop even starts: the job must never be delivered.
    let outcome = running
        .cancel(&name)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    assert_eq!(outcome, ports::CancelOutcome::Cancelled);

    let handle = tokio::spawn({
        let running = Arc::clone(&running);
        async move { running.run().await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(target.count(), 0, "a cancelled job must never be delivered");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn local_runner_retries_after_5xx_with_scaled_backoff() -> TestResult {
    let target = Arc::new(Target::default());
    target.script([500, 200]);
    let addr = spawn_target(Arc::clone(&target)).await?;
    let base = Url::parse(&format!("http://{addr}"))?;
    // Scale 100: the 30 s production back-off becomes 0.3 s.
    let running = Arc::new(runner(&base, 100.0)?);

    let job = JobId(Uuid::new_v4());
    running
        .schedule(&job, now())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let handle = tokio::spawn({
        let running = Arc::clone(&running);
        async move { running.run().await }
    });

    // Unscaled, 30 s would not fit in this window: the retry proves the scale.
    assert!(
        wait_for(|| target.count() >= 2, Duration::from_secs(3)).await,
        "the 5xx was not retried"
    );
    assert_eq!(target.statuses(), vec![500, 200]);
    assert_eq!(target.retries(), vec!["0".to_owned(), "1".to_owned()]);
    // And the retry waits for the back-off (0.3 s scaled) rather than firing
    // immediately.
    let gap = target.first_gap().unwrap_or_default();
    assert!(
        gap >= Duration::from_millis(200),
        "the retry was not delayed by the scaled back-off: {gap:?}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn local_runner_does_not_retry_a_4xx() -> TestResult {
    let target = Arc::new(Target::default());
    target.script([400]);
    let addr = spawn_target(Arc::clone(&target)).await?;
    let base = Url::parse(&format!("http://{addr}"))?;
    let running = Arc::new(runner(&base, 100.0)?);

    let job = JobId(Uuid::new_v4());
    running
        .schedule(&job, now())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let handle = tokio::spawn({
        let running = Arc::clone(&running);
        async move { running.run().await }
    });

    assert!(
        wait_for(|| target.count() >= 1, Duration::from_secs(2)).await,
        "the job was not delivered"
    );
    // Longer than the first scaled back-off (0.3 s): a wrong retry would show.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(target.count(), 1, "a 4xx must not be retried");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn local_runner_refuses_a_non_loopback_target() -> TestResult {
    let non_loopback_ip = Url::parse("http://10.0.0.1:8080")?;
    assert_eq!(
        runner(&non_loopback_ip, 1.0).err(),
        Some(RunnerError::NotLoopback)
    );

    // A hostname that is not an IP literal is refused too.
    let hostname = Url::parse("https://unsub.example.com")?;
    assert!(runner(&hostname, 1.0).is_err());

    // A loopback target is accepted.
    let loopback = Url::parse("http://127.0.0.1:9")?;
    assert!(runner(&loopback, 1.0).is_ok());
    Ok(())
}

#[test]
fn e2e_unsub_delay_override_changes_due_at() -> TestResult {
    let now = time::macros::datetime!(2026-10-05 00:00 UTC);

    // `MT_E2E_UNSUB_DELAY_S=2`: a reject plans `due_at` two seconds ahead.
    let overridden = e2e_tunables(|name| (name == "MT_E2E_UNSUB_DELAY_S").then(|| "2".to_owned()));
    assert_eq!(overridden.unsub_delay, Duration::from_secs(2));
    assert_eq!(
        reject_due_at(&overridden, now)?,
        now + time::Duration::seconds(2)
    );

    // Unset: the production delay (S2 `UNSUB_DELAY`, five minutes).
    let production = e2e_tunables(|_| None);
    assert_eq!(production.unsub_delay, Tunables::default().unsub_delay);
    assert_eq!(
        reject_due_at(&production, now)?,
        now + time::Duration::minutes(5)
    );
    Ok(())
}

/// Plan one one-click-list reject with `tunables` and return the queued
/// unsubscribe's `due_at`.
fn reject_due_at(
    tunables: &Tunables,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, Box<dyn std::error::Error + Send + Sync>> {
    let facts = HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(Url::parse("https://unsub.example.com/one")?),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some("list-1".to_owned()),
        feedback_id: None,
        precedence_bulk: false,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    };
    let meta = MessageMeta {
        mailbox: MailboxId(Uuid::new_v4()),
        id: MessageId::new("m1")?,
        internal_date: now,
        from_display: String::new(),
        from_address: "a@example.com".to_owned(),
        sender: SenderKey::from_address("a@example.com"),
        subject: String::new(),
        labels: LabelSet::new(),
        facts,
    };
    let badge = Classification {
        class: MessageClass::List,
        bulk_score: 0,
        bulk_reason: String::new(),
        confidence: None,
        probabilities: None,
    };
    let stats = SenderStats::default();
    let plan = plan_swipe(&SwipeInput {
        action: SwipeAction::Reject,
        meta: &meta,
        badge: &badge,
        stats: &stats,
        ids: SwipeIds {
            job_id: JobId(Uuid::new_v4()),
            rule_id: RuleId(Uuid::new_v4()),
        },
        source_swipe: Uuid::new_v4(),
        now,
        tunables,
    });
    let unsub = plan
        .unsubscribe
        .ok_or_else(|| std::io::Error::other("reject did not plan an unsubscribe"))?;
    Ok(unsub.due_at)
}
