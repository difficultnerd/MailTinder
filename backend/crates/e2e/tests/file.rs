#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 6 (SW-04 AC2, S10 8): filing a message applies the new label and the
//! message leaves the inbox, and the journey emits exactly one `swipe` event.
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).

use std::time::Duration;

use e2e::{seeded_received, signed_in_user, EventLog, FakeGoogle, Stack};

/// A short wait for a state that is already on screen.
const SOON: Duration = Duration::from_secs(20);
/// A long wait for a network round trip.
const LOAD: Duration = Duration::from_secs(60);

/// The journey's own account, so it never sees another journey's mail.
const SUB: &str = "sub-sw04";
const EMAIL: &str = "sw04@example.com";
/// The corpus case seeded onto the Feed; its sender is [`SENDER`].
const FIXTURE: &str = "one-click-covered";
/// The sender shown on the seeded card.
const SENDER: &str = "Acme News";
/// The category the journey creates and files under.
const CATEGORY: &str = "Receipts";
/// The name field's semantics label (the hint) and the file confirmation toast
/// (SW-04 AC2; `Copy.filed`).
const NAME_FIELD: &str = "Category name";
const FILED: &str = "Filed under Receipts.";

/// SW-04 AC2: filing creates the label, applies it to the message, takes the
/// message out of the inbox, and emits exactly one swipe event.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_04_ac2_e2e_file_applies_label_and_leaves_inbox(
) -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;

    google.reset().await?;
    google.seed_account(SUB, EMAIL, true).await?;
    let id = google
        .seed_message(SUB, FIXTURE, &seeded_received(1)?, None)
        .await?;

    let log = EventLog::open()?;
    let mark = log.mark();

    let ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_text(SENDER, LOAD).await?;

    // File opens the sheet. With nothing to suggest it opens the name field
    // directly (SW-04 AC3); otherwise "New category" is the way in (SW-04 AC1).
    ui.tap("File").await?;
    if ui.wait_for_text("New category", SOON).await.is_ok() {
        ui.tap("New category").await?;
    }
    ui.wait_for_text(NAME_FIELD, LOAD).await?;
    ui.type_into(NAME_FIELD, CATEGORY).await?;
    // The sheet's confirm button repeats the card's "File" label; tap the
    // topmost one.
    ui.tap_last("File").await?;
    ui.wait_for_text(FILED, LOAD).await?;

    // fake-google shows a label named "Receipts" on the message and no INBOX
    // label (SW-04 AC2).
    let label_ids = google.message_labels(EMAIL, &id).await?;
    let names = google.label_names(EMAIL).await?;
    let applied: Vec<&str> = label_ids
        .iter()
        .filter_map(|label_id| names.get(label_id).map(String::as_str))
        .collect();
    assert!(
        applied.contains(&CATEGORY),
        "the message carries the new label, saw {applied:?}"
    );
    assert!(
        !label_ids.iter().any(|label| label == "INBOX"),
        "filing takes the message out of the inbox"
    );

    // Events (S10 8): filing is one swipe event and nothing else.
    let events = log.since_mark(mark)?;
    let swipes: Vec<_> = events.iter().filter(|e| e.event_type == "swipe").collect();
    assert_eq!(swipes.len(), 1, "exactly one swipe event");

    ui.close().await?;
    Ok(())
}
