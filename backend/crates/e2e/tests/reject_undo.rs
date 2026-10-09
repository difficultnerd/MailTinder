#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 4 (SW-05 AC2): reject then undo before the due time.
//!
//! Run only by `scripts/e2e.sh`. Rejecting a one-click list message queues an
//! unsubscribe five minutes out; Undo before it falls due cancels the job, so
//! no request ever reaches the testbed and the message returns to the INBOX
//! with its exact label set (S10 3.3, 6.3).

use std::time::Duration;

use e2e::{
    signed_in_user, EventLog, FakeGoogle, Stack, TestControl, Testbed, FEED_TIMEOUT, REJECT_BUTTON,
    UNDO_BUTTON,
};

/// A per-journey `sub` so journeys never see each other's mail.
const SUB: &str = "sub-sw-05-a";
/// The seeded account, a reserved domain (S10 5).
const EMAIL: &str = "sw05@example.com";
/// The seeded card's sender (corpus case `one-click-covered`).
const SENDER: &str = "Acme News";
/// The testbed route the message's one-click URL points at.
const ONE_CLICK_PATH: &str = "/oneclick/200";
/// The reject toast before the delay elapses (five minutes, S9).
const TRASHED_UNSUBSCRIBING: &str = "Trashed. Unsubscribing in 5 minutes.";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_05_ac2_e2e_undo_before_due_sends_nothing() -> Result<(), Box<dyn std::error::Error>> {
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
    let id = google
        .seed_one_click_message(SUB, "one-click-covered", &one_click, &["INBOX", "UNREAD"])
        .await?;

    let ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_semantic_text(SENDER, FEED_TIMEOUT).await?;

    // Reject, then undo before the five-minute window runs out.
    ui.tap(REJECT_BUTTON).await?;
    ui.wait_for_text(TRASHED_UNSUBSCRIBING, FEED_TIMEOUT)
        .await?;
    ui.tap(UNDO_BUTTON).await?;
    ui.wait_for_semantic_text(SENDER, FEED_TIMEOUT).await?;

    // Past the due time the cancelled job never fires.
    control.advance_clock(Duration::from_secs(6 * 60)).await?;
    assert!(
        testbed.received(ONE_CLICK_PATH).await?.is_empty(),
        "an undone unsubscribe must never reach the testbed"
    );

    // The message is back in the INBOX with its exact label set.
    let mut got = google.message_labels(SUB, &id).await?;
    got.sort();
    assert_eq!(got, vec!["INBOX".to_owned(), "UNREAD".to_owned()]);

    // Events (S10 8): one swipe (reject) and one undo, and no unsubscribe
    // outcome, because the job was cancelled.
    let events = logs.since_mark(mark)?;
    assert_eq!(events.iter().filter(|e| e.event_type == "swipe").count(), 1);
    assert!(events
        .iter()
        .any(|e| e.event_type == "swipe" && e.outcome.as_deref() == Some("reject")));
    assert_eq!(events.iter().filter(|e| e.event_type == "undo").count(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "unsub_outcome")
            .count(),
        0
    );

    ui.close().await?;
    Ok(())
}
