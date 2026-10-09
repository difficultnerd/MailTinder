#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 6 (SW-04 AC2): file a message into a new category.
//!
//! Run only by `scripts/e2e.sh`. The filing sheet's "New category" path applies
//! the provider label and the message leaves the inbox (S10 3.3).

use e2e::{signed_in_user, EventLog, FakeGoogle, Stack, FEED_TIMEOUT, FILE_BUTTON};

/// A per-journey `sub` so journeys never see each other's mail.
const SUB: &str = "sub-sw-04-a";
/// The seeded account, a reserved domain (S10 5).
const EMAIL: &str = "sw04@example.com";
/// The seeded card's sender (corpus case `receipt-same-sender-no-lu`).
const SENDER: &str = "Acme Store";
/// The new category the journey names.
const CATEGORY: &str = "Receipts";
/// The filed toast (S9 section 4).
const FILED: &str = "Filed under Receipts.";
/// The new-category name field's label/hint (S9 section 4).
const CATEGORY_FIELD: &str = "Category name";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_04_ac2_e2e_file_applies_label_and_leaves_inbox(
) -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;

    let logs = EventLog::new();
    let mark = logs.mark();

    google.seed_account(SUB, EMAIL, true).await?;
    let id = google
        .seed_message(SUB, "receipt-same-sender-no-lu")
        .await?;

    let ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_semantic_text(SENDER, FEED_TIMEOUT).await?;

    // File: with no suggestion and an empty category cache the sheet goes
    // straight to the name field (SW-04 AC3), so name the new category and
    // confirm with the sheet's File button.
    ui.tap(FILE_BUTTON).await?;
    ui.wait_for_text(CATEGORY_FIELD, FEED_TIMEOUT).await?;
    ui.type_into(CATEGORY_FIELD, CATEGORY).await?;
    ui.tap(FILE_BUTTON).await?;
    ui.wait_for_text(FILED, FEED_TIMEOUT).await?;

    // The label is on the message and it is no longer in the inbox.
    let labels = google.message_labels(SUB, &id).await?;
    assert!(
        labels.iter().any(|label| label == CATEGORY),
        "the Receipts label was applied"
    );
    assert!(
        !labels.iter().any(|label| label == "INBOX"),
        "the filed message left the inbox"
    );

    // Events (S10 8): one swipe.
    let events = logs.since_mark(mark)?;
    assert_eq!(events.iter().filter(|e| e.event_type == "swipe").count(), 1);

    ui.close().await?;
    Ok(())
}
