#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 4 (SW-05 AC2, S10 8): reject with unsubscribe then undo before the
//! due time cancels the job - no request ever reaches the testbed - and the
//! message is restored to its exact label set. Run only by `scripts/e2e.sh`.

use std::time::Duration;

use e2e::{signed_in_user, EventLog, FakeGoogle, Stack, TestControl, Testbed};

/// A long wait for a network round trip.
const LOAD: Duration = Duration::from_secs(60);

const SUB: &str = "sub-sw05";
const EMAIL: &str = "sw05@example.com";
/// The corpus case: one-click, DKIM covers both unsubscribe headers.
const FIXTURE: &str = "one-click-covered";
/// The testbed's one-click route the seeded header points at.
const ROUTE: &str = "/oneclick/200";
/// The sender shown on the card.
const SENDER: &str = "Acme News";
/// The reject toast (SW-05 AC2; `Copy.trashedUnsubscribing`).
const TOAST: &str = "Trashed. Unsubscribing in 5 minutes.";

/// SW-05 AC2: undo before the due time cancels the job, so the testbed sees no
/// request, the message is back in INBOX, and the metric events are exactly one
/// `swipe` (reject) and one `undo` - never an `unsub_outcome`.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_05_ac2_e2e_undo_before_due_sends_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let control = TestControl::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;

    google.reset().await?;
    google.seed_account(SUB, EMAIL, true).await?;
    let target = stack.testbed_https_url(ROUTE);
    let id = google
        .seed_message(SUB, FIXTURE, &e2e::seeded_received(1)?, Some(&target))
        .await?;

    let log = EventLog::open()?;
    let mark = log.mark();

    let ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_text(SENDER, LOAD).await?;

    // Reject queues the unsubscribe; the toast states the delay.
    ui.tap("Reject").await?;
    ui.wait_for_text(TOAST, LOAD).await?;
    // Undo before the due time cancels the job (SW-05 AC2).
    ui.tap_last("Undo").await?;
    ui.wait_for_text(SENDER, LOAD).await?;

    // Advancing the clock past the due time must produce no request at all.
    control.advance_clock(Duration::from_secs(6 * 60)).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        testbed.received(ROUTE).await?.is_empty(),
        "undo before the due time must cancel the queued unsubscribe"
    );

    // The message is restored to its exact previous label set, in INBOX.
    let mut labels = google.message_labels(EMAIL, &id).await?;
    labels.sort();
    assert_eq!(labels, vec!["INBOX".to_owned()], "exact label set restored");

    // Events (S10 8): one swipe (reject), one undo, no unsub_outcome.
    let events = log.since_mark(mark)?;
    let swipes: Vec<_> = events.iter().filter(|e| e.event_type == "swipe").collect();
    assert_eq!(swipes.len(), 1, "exactly one swipe event");
    assert_eq!(swipes[0].outcome.as_deref(), Some("reject"));
    assert_eq!(
        events.iter().filter(|e| e.event_type == "undo").count(),
        1,
        "exactly one undo event"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "unsub_outcome")
            .count(),
        0,
        "a cancelled job emits no unsub_outcome"
    );

    ui.close().await?;
    Ok(())
}
