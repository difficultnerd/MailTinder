#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 5 (UN-01 AC1/AC3, UN-02 AC1): reject and let the unsubscribe run.
//!
//! Run only by `scripts/e2e.sh`. After the due time the queued one-click POST
//! reaches the testbed exactly once, with the fixed body and no cookies or
//! credentials; the next Feed load collects the outcome into History (S10 3.3,
//! 6.3, 8).

use std::time::{Duration, Instant};

use e2e::{
    signed_in_user, EventLog, FakeGoogle, LogMark, RecordedRequest, Stack, TestControl, Testbed,
    Ui, FEED_TIMEOUT, HISTORY, REJECT_BUTTON, SETTINGS_TAB, UNSUBSCRIBES_FILTER,
};

/// A per-journey `sub` so journeys never see each other's mail.
const SUB: &str = "sub-un-01-a";
/// The seeded account, a reserved domain (S10 5).
const EMAIL: &str = "un01@example.com";
/// The seeded card's sender (corpus case `one-click-covered`).
const SENDER: &str = "Acme News";
/// The testbed route the message's one-click URL points at.
const ONE_CLICK_PATH: &str = "/oneclick/200";
/// The reject toast before the delay elapses (five minutes, S9).
const TRASHED_UNSUBSCRIBING: &str = "Trashed. Unsubscribing in 5 minutes.";
/// How long to poll the testbed for the asynchronous delivery.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// A reject-and-run journey, ready for the per-AC assertions.
struct Journey {
    google: FakeGoogle,
    testbed: Testbed,
    control: TestControl,
    logs: EventLog,
    mark: LogMark,
    ui: Ui,
}

/// Poll the testbed for the one-click POST, up to [`DELIVERY_TIMEOUT`].
async fn wait_for_requests(
    testbed: &Testbed,
    path: &str,
) -> Result<Vec<RecordedRequest>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    loop {
        let records = testbed.received(path).await?;
        if !records.is_empty() || Instant::now() >= deadline {
            return Ok(records);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Seed a one-click message, sign in, reject it, advance past the due time and
/// wait for the testbed to record the POST.
async fn reject_and_let_it_run() -> Result<Journey, Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;
    let control = TestControl::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;
    testbed.reset().await?;

    let logs = EventLog::new();
    let mark = logs.mark();

    google.seed_account(SUB, EMAIL, true).await?;
    let one_click = testbed.one_click_url(ONE_CLICK_PATH)?;
    google
        .seed_one_click_message(SUB, "one-click-covered", &one_click, &["INBOX"])
        .await?;

    let ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_semantic_text(SENDER, FEED_TIMEOUT).await?;
    ui.tap(REJECT_BUTTON).await?;
    ui.wait_for_text(TRASHED_UNSUBSCRIBING, FEED_TIMEOUT)
        .await?;

    control.advance_clock(Duration::from_secs(6 * 60)).await?;
    let _ = wait_for_requests(&testbed, ONE_CLICK_PATH).await?;

    Ok(Journey {
        google,
        testbed,
        control,
        logs,
        mark,
        ui,
    })
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac1_e2e_unsubscribe_runs_once() -> Result<(), Box<dyn std::error::Error>> {
    let journey = reject_and_let_it_run().await?;

    assert_eq!(
        journey.testbed.received(ONE_CLICK_PATH).await?.len(),
        1,
        "a queued unsubscribe runs exactly once after its due time"
    );

    // Advancing the clock again and reloading the Feed sends nothing more.
    journey
        .control
        .advance_clock(Duration::from_secs(6 * 60))
        .await?;
    journey.ui.reload().await?;
    journey.ui.wait_for_text(".", FEED_TIMEOUT).await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        journey.testbed.received(ONE_CLICK_PATH).await?.len(),
        1,
        "a due job never runs twice"
    );

    // Events (S10 8): one swipe and one sent unsubscribe outcome.
    let events = journey.logs.since_mark(journey.mark)?;
    assert_eq!(events.iter().filter(|e| e.event_type == "swipe").count(), 1);
    let outcomes: Vec<&str> = events
        .iter()
        .filter(|e| e.event_type == "unsub_outcome")
        .filter_map(|e| e.outcome.as_deref())
        .collect();
    assert_eq!(outcomes, vec!["sent"]);

    journey.ui.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_02_ac1_e2e_one_click_post_is_exact() -> Result<(), Box<dyn std::error::Error>> {
    let journey = reject_and_let_it_run().await?;
    let records = journey.testbed.received(ONE_CLICK_PATH).await?;
    assert_eq!(records.len(), 1, "exactly one one-click request");
    let record = &records[0];
    assert_eq!(record.method, "POST");
    assert_eq!(record.body, b"List-Unsubscribe=One-Click");
    assert!(
        !record.headers.iter().any(|(name, _)| name == "cookie"),
        "no cookie on a one-click POST"
    );
    assert!(
        !record
            .headers
            .iter()
            .any(|(name, _)| name == "authorization"),
        "no credentials on a one-click POST"
    );
    journey.ui.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac3_e2e_outcome_in_history_after_feed_load() -> Result<(), Box<dyn std::error::Error>>
{
    let journey = reject_and_let_it_run().await?;

    // The next Feed load collects the stored outcome, then History shows it
    // under the Unsubscribes filter with outcome "Sent".
    journey.ui.reload().await?;
    journey
        .ui
        .wait_for_text(FEED_TIMEOUT_MARKER, FEED_TIMEOUT)
        .await
        .ok();
    journey.ui.tap(SETTINGS_TAB).await?;
    journey.ui.wait_for_text(HISTORY, FEED_TIMEOUT).await?;
    journey.ui.tap(HISTORY).await?;
    journey
        .ui
        .wait_for_text(UNSUBSCRIBES_FILTER, FEED_TIMEOUT)
        .await?;
    journey.ui.tap(UNSUBSCRIBES_FILTER).await?;
    journey
        .ui
        .wait_for_text("Unsubscribe: Sent", FEED_TIMEOUT)
        .await?;

    let _ = &journey.google;
    journey.ui.close().await?;
    Ok(())
}

/// Any text guaranteed to be on the Feed after a reload (the tab bar), so the
/// reload completes before History is opened.
const FEED_TIMEOUT_MARKER: &str = "Needs Attention";
