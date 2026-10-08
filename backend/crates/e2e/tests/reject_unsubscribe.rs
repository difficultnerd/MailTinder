#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 5 (UN-01 AC1, UN-01 AC3, UN-02 AC1, S10 8): reject and let the
//! unsubscribe run exactly once, with the exact one-click POST, and the outcome
//! appearing in History after the next Feed load. Run only by `scripts/e2e.sh`.

use std::time::{Duration, Instant};

use e2e::{
    seeded_received, signed_in_user, EventLog, FakeGoogle, MetricEvent, RecordedRequest, Stack,
    TestControl, Testbed, FEED_TAB,
};

/// A short wait for a state that is already on screen.
const SOON: Duration = Duration::from_secs(20);
/// A long wait for a network round trip.
const LOAD: Duration = Duration::from_secs(60);
/// How long to wait for the due job to reach the testbed (asynchronous).
const DELIVERY: Duration = Duration::from_secs(10);

/// The corpus case: one-click, DKIM covers both unsubscribe headers.
const FIXTURE: &str = "one-click-covered";
/// The testbed's one-click route the seeded header points at.
const ROUTE: &str = "/oneclick/200";
/// The sender shown on the card.
const SENDER: &str = "Acme News";
/// The reject toast (UN-01 AC1; `Copy.trashedUnsubscribing`).
const TOAST: &str = "Trashed. Unsubscribing in 5 minutes.";
/// The fixed one-click body (UN-02 AC1, S10 6.2).
const ONE_CLICK_BODY: &str = "List-Unsubscribe=One-Click";

/// What one run of the journey observed.
struct Run {
    /// The requests the testbed recorded for the one-click route.
    records: Vec<RecordedRequest>,
    /// The recorded count after advancing the clock again and refreshing.
    after_second_advance: usize,
    /// Whether History showed a `Sent` unsubscribe entry.
    history_sent: bool,
    /// The metric events written during the journey.
    events: Vec<MetricEvent>,
}

/// Poll the testbed until `route` has a request or the timeout elapses.
async fn poll_route(
    testbed: &Testbed,
    route: &str,
    timeout: Duration,
) -> Result<Vec<RecordedRequest>, e2e::E2eError> {
    let deadline = Instant::now() + timeout;
    loop {
        let records = testbed.received(route).await?;
        if !records.is_empty() || Instant::now() >= deadline {
            return Ok(records);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Reject one one-click list message and let its job run.
async fn run(stack: &Stack, sub: &str, email: &str) -> Result<Run, e2e::E2eError> {
    let google = FakeGoogle::connect(stack)?;
    let control = TestControl::connect(stack)?;
    let testbed = Testbed::connect(stack)?;

    google.reset().await?;
    google.seed_account(sub, email, true).await?;
    let target = stack.testbed_https_url(ROUTE);
    google
        .seed_message(sub, FIXTURE, &seeded_received(1)?, Some(&target))
        .await?;

    let log = EventLog::open()?;
    let mark = log.mark();

    let ui = signed_in_user(stack, sub, email, &[]).await?;
    ui.wait_for_text(SENDER, LOAD).await?;
    ui.tap("Reject").await?;
    ui.wait_for_text(TOAST, LOAD).await?;
    control.advance_clock(Duration::from_secs(6 * 60)).await?;
    let records = poll_route(&testbed, ROUTE, DELIVERY).await?;

    // Pull to refresh the Feed: the next Feed load collects the stored outcome.
    ui.reload().await?;
    ui.wait_for_text(FEED_TAB, LOAD).await?;
    ui.tap("Settings").await?;
    ui.tap("History").await?;
    ui.tap("Unsubscribes").await?;
    let history_sent = ui.wait_for_text("Sent", SOON).await.is_ok();

    // Advancing again and refreshing must not run the job a second time.
    control.advance_clock(Duration::from_secs(6 * 60)).await?;
    ui.reload().await?;
    ui.wait_for_text(FEED_TAB, LOAD).await?;
    let after_second_advance = testbed.received(ROUTE).await?.len();

    let events = log.since_mark(mark)?;
    ui.close().await?;
    Ok(Run {
        records,
        after_second_advance,
        history_sent,
        events,
    })
}

/// UN-01 AC1: a queued unsubscribe runs exactly once after its due time.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac1_e2e_unsubscribe_runs_once() -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let run = run(&stack, "sub-un01a", "un01a@example.com").await?;
    assert_eq!(run.records.len(), 1, "exactly one POST reaches the testbed");
    assert_eq!(
        run.after_second_advance, 1,
        "the job runs once, never again"
    );
    // Events (S10 8): one swipe and one unsub_outcome with outcome sent.
    assert_eq!(
        run.events
            .iter()
            .filter(|e| e.event_type == "swipe")
            .count(),
        1
    );
    let outcomes: Vec<_> = run
        .events
        .iter()
        .filter(|e| e.event_type == "unsub_outcome")
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].outcome.as_deref(), Some("sent"));
    Ok(())
}

/// UN-02 AC1: the one-click POST has the fixed body and no cookies or
/// credentials.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_02_ac1_e2e_one_click_post_is_exact() -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let run = run(&stack, "sub-un02", "un02@example.com").await?;
    let first = run.records.first().ok_or("no one-click request recorded")?;
    assert_eq!(first.method, "POST");
    assert_eq!(
        first.body_text(),
        ONE_CLICK_BODY,
        "body is exactly one-click"
    );
    assert!(!first.has_header("cookie"), "no Cookie header");
    assert!(
        !first.has_header("authorization"),
        "no Authorization header"
    );
    Ok(())
}

/// UN-01 AC3: the outcome appears in History after the next Feed load.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac3_e2e_outcome_in_history_after_feed_load() -> Result<(), Box<dyn std::error::Error>>
{
    let stack = Stack::from_env()?;
    let run = run(&stack, "sub-un01c", "un01c@example.com").await?;
    assert!(
        run.history_sent,
        "History shows the unsubscribe with outcome Sent"
    );
    Ok(())
}
