#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 6 (SW-04 AC2): in a real browser the user files a message under a
//! new category, the provider label is applied and the message leaves the
//! inbox. Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).
//!
//! The Feed shows one seeded card. Tapping "File" opens the filing sheet; the
//! account has no categories yet, so there is nothing to suggest and the sheet
//! opens straight on the category name field (SW-04 AC3), where the new
//! category's name is typed. Confirming files the message; the fake's Gmail
//! read-back then proves the label is applied and `INBOX` is gone (SW-04 AC2).
//! A file emits no metric event today - the `swipe` assertion is deferred to
//! T-1114 (ADR 0002) - so none may appear after the mark.

use std::error::Error;
use std::time::Duration;

use e2e::{finish_journey, signed_in_user, EventLog, FakeGoogle, Stack};

/// The account this journey signs in as; unique so it never sees another
/// journey's mail.
const SUB: &str = "sub-file-journey";
/// The seeded account's address (`example.com`: reserved for tests).
const EMAIL: &str = "file@example.com";
/// The single corpus case seeded; its sender is the card text `signed_in_user`
/// waits for, and its one-click list class gives the Feed a card to file.
const FIXTURE: &str = "one-click-covered";
/// The category the journey creates.
const CATEGORY: &str = "Receipts";
/// The Feed's File button and the filing sheet's confirm button (XC-03).
const FILE: &str = "File";
/// The filing sheet's name field, identified by its hint text (SW-04 AC3).
const CATEGORY_NAME_FIELD: &str = "Category name";
/// How long the file round-trip (category, label, modify) may take.
const FILE_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_04_ac2_e2e_file_applies_label_and_leaves_inbox() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;

    // Read only the metric events this journey writes.
    let events = EventLog::new();
    let mark = events.mark();

    // Sign in and land on the Feed with the one seeded card.
    let ui = signed_in_user(&stack, SUB, EMAIL, &[FIXTURE]).await?;

    // Tap "File" on the card: the sheet opens on the name field, because a
    // fresh account has no category to suggest (SW-04 AC3). Type the new
    // category's name, then confirm through the sheet's own "File" button - the
    // modal barrier blocks the card's buttons while the sheet is open, so the
    // label is unambiguous.
    ui.tap(FILE).await?;
    ui.type_into(CATEGORY_NAME_FIELD, CATEGORY).await?;
    ui.tap(FILE).await?;

    // The confirmation toast names the category the api filed under.
    ui.wait_for_text(&format!("Filed under {CATEGORY}."), FILE_TIMEOUT)
        .await?;

    // Provider read-back (SW-04 AC2): the seeded message now carries the label
    // named `CATEGORY` and no longer carries `INBOX`.
    let messages = google.message_labels(EMAIL).await?;
    assert_eq!(
        messages.len(),
        1,
        "the mailbox should hold exactly the seeded message"
    );
    let labels = &messages[0];
    assert!(
        labels.iter().any(|name| name == CATEGORY),
        "the message does not carry a label named {CATEGORY}: {labels:?}"
    );
    assert!(
        !labels.iter().any(|name| name == "INBOX"),
        "the filed message is still in the inbox: {labels:?}"
    );

    // Journey 6 emits no metric event today (S10 8; `swipe` is T-1114).
    let emitted = events.since_mark(mark)?;
    assert!(
        emitted.is_empty(),
        "journey 6 emitted metric events: {emitted:?}"
    );

    finish_journey(ui).await?;
    Ok(())
}
