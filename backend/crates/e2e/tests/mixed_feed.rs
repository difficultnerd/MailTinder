#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 3 (FD-02 AC1, AC2): the Feed interleaves two mailboxes by received
//! time, newest first, and each card shows its mailbox. Run only by
//! `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).

use std::time::Duration;

use e2e::{seeded_received, signed_in_user, E2eError, FakeGoogle, Stack, CONTINUE_WITH_GOOGLE};

/// A short wait for a state that is already on screen.
const SOON: Duration = Duration::from_secs(20);
/// A long wait for a network round trip.
const LOAD: Duration = Duration::from_secs(60);

/// Mailbox A (the signed-in account) and mailbox B (linked afterwards).
const SUB_A: &str = "sub-fd02-a";
const SUB_B: &str = "sub-fd02-b";
const EMAIL_A: &str = "fd02-a@example.com";
const EMAIL_B: &str = "fd02-b@example.com";

/// The three seeded cards in Feed order, newest first: sender, mailbox. A's
/// messages are at t1 and t3, B's at t2, so the interleaving is t3, t2, t1.
const EXPECTED: [(&str, &str); 3] = [
    ("Account Security", EMAIL_A),
    ("Acme News", EMAIL_B),
    ("Maya Chen", EMAIL_A),
];

/// Run the mixed-Feed journey and return each card's (sender, mailbox) in the
/// order the user sees them.
async fn read_mixed_feed(stack: &Stack) -> Result<Vec<(String, String)>, E2eError> {
    let google = FakeGoogle::connect(stack)?;
    google.reset().await?;
    google.seed_account(SUB_A, EMAIL_A, true).await?;
    google.seed_account(SUB_B, EMAIL_B, true).await?;
    // A: t1 and t3; B: t2. The times interleave the two mailboxes (FD-02 AC1).
    let t1 = seeded_received(1)?;
    let t2 = seeded_received(2)?;
    let t3 = seeded_received(3)?;
    google
        .seed_message(SUB_A, "personal-one-to-one", &t1, None)
        .await?;
    google
        .seed_message(SUB_A, "notice-no-header", &t3, None)
        .await?;
    google
        .seed_message(SUB_B, "one-click-covered", &t2, None)
        .await?;

    let ui = signed_in_user(stack, SUB_A, EMAIL_A, &[]).await?;

    // Link mailbox B: Add Gmail, step up (re-authenticate A), then the link
    // flow selects B.
    google.select_account_for_next_authorize(SUB_A).await?;
    ui.tap("Settings").await?;
    ui.tap("Connected accounts").await?;
    ui.tap("Add Gmail").await?;
    ui.wait_for_text("Confirm it's you", LOAD).await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.switch_to_popup().await?;
    ui.wait_for_text("Confirmed. You can close this window.", LOAD)
        .await?;
    // The link request re-authenticates B in the main window.
    google.select_account_for_next_authorize(SUB_B).await?;
    ui.switch_to_first_window().await?;
    ui.wait_for_text("Connected accounts", LOAD).await?;
    ui.wait_for_text(EMAIL_B, SOON).await?;

    // Back to the Feed: three cards, newest first, each with its mailbox.
    ui.tap("Feed").await?;
    let mut seen = Vec::new();
    for (sender, mailbox) in EXPECTED {
        ui.wait_for_text(sender, LOAD).await?;
        ui.wait_for_text(mailbox, SOON).await?;
        seen.push((sender.to_owned(), mailbox.to_owned()));
        ui.tap("Keep").await?;
    }
    ui.close().await?;
    Ok(seen)
}

/// FD-02 AC1: the Feed interleaves two mailboxes by received time, newest
/// first.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac1_e2e_two_mailboxes_interleaved_newest_first(
) -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let seen = read_mixed_feed(&stack).await?;
    let senders: Vec<&str> = seen.iter().map(|(sender, _)| sender.as_str()).collect();
    let expected: Vec<&str> = EXPECTED.iter().map(|(sender, _)| *sender).collect();
    assert_eq!(senders, expected, "Feed order must be t3, t2, t1");
    Ok(())
}

/// FD-02 AC2: each card shows its mailbox.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac2_e2e_cards_show_mailbox() -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let seen = read_mixed_feed(&stack).await?;
    let mailboxes: Vec<&str> = seen.iter().map(|(_, mailbox)| mailbox.as_str()).collect();
    let expected: Vec<&str> = EXPECTED.iter().map(|(_, mailbox)| *mailbox).collect();
    assert_eq!(mailboxes, expected, "each card must show its mailbox");
    Ok(())
}
