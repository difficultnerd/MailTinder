#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 3 (FD-02 AC1, AC2): a mixed Feed across two mailboxes.
//!
//! Run only by `scripts/e2e.sh`. Mailbox A is seeded with fixtures at t1 and t3,
//! mailbox B at t2; after A signs in and B is linked through the UI, the Feed
//! interleaves the two mailboxes by received time, newest first, each card
//! showing its mailbox address (S10 3.3).

use e2e::{
    app_origin, signed_in_user, FakeGoogle, Stack, Ui, CONNECTED_ACCOUNTS, CONTINUE_WITH_GOOGLE,
    FEED_TIMEOUT, KEEP_BUTTON, REDIRECT_TIMEOUT, SETTINGS_TAB,
};

/// Per-journey `sub`s so journeys never see each other's mail.
const SUB_A: &str = "sub-fd-02-a";
const SUB_B: &str = "sub-fd-02-b";
/// Reserved-domain accounts (S10 5).
const EMAIL_A: &str = "fda@example.com";
const EMAIL_B: &str = "fdb@example.com";

/// Corpus senders, one per seeded card.
/// A at t3, the newest overall (case `esp-pass-dmarc-fail`).
const A_NEWEST: &str = "Acme Deals";
/// B at t2 (case `personal-one-to-one`).
const B_MIDDLE: &str = "Maya Chen";
/// A at t1, the oldest (case `notice-no-header`).
const A_OLDEST: &str = "Account Security";

/// Settings copy (S9 7, 7.1) and the step-up panel title.
const ADD_GMAIL: &str = "Add Gmail";
const CONFIRM_ITS_YOU: &str = "Confirm it's you";
/// The Feed tab label.
const TAB_FEED: &str = "Feed";
/// The empty-Feed copy.
const NOTHING_TO_TRIAGE: &str = "Nothing to triage.";

/// Seed A (t1, t3) and B (t2), sign A in, link B through the UI and return a
/// `Ui` on the Feed with both mailboxes' mail loaded.
async fn two_mailbox_feed() -> Result<Ui, Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;
    google.seed_account(SUB_A, EMAIL_A, true).await?;
    google.seed_account(SUB_B, EMAIL_B, true).await?;
    google
        .seed_messages_at(
            SUB_A,
            &[("notice-no-header", 1), ("esp-pass-dmarc-fail", 3)],
        )
        .await?;
    google
        .seed_messages_at(SUB_B, &[("personal-one-to-one", 2)])
        .await?;

    // Sign in with A (its fixtures are already seeded).
    let ui = signed_in_user(&stack, SUB_A, EMAIL_A, &[]).await?;

    // Settings -> Connected accounts -> Add Gmail. The step-up re-authenticates
    // A in a popup, then the link authorisation signs in B in the main window.
    ui.tap(SETTINGS_TAB).await?;
    ui.wait_for_text(CONNECTED_ACCOUNTS, FEED_TIMEOUT).await?;
    ui.tap(CONNECTED_ACCOUNTS).await?;
    ui.wait_for_text(ADD_GMAIL, FEED_TIMEOUT).await?;

    google.select_account_for_next_authorize(SUB_A).await?;
    ui.tap(ADD_GMAIL).await?;
    ui.wait_for_text(CONFIRM_ITS_YOU, FEED_TIMEOUT).await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.switch_to_popup().await?;
    ui.switch_to_main().await?;

    // The overlay clears once the step-up is confirmed; the main window then
    // navigates to the link authorisation, which signs in B.
    ui.wait_for_text_absent(CONFIRM_ITS_YOU, FEED_TIMEOUT)
        .await?;
    google.select_account_for_next_authorize(SUB_B).await?;
    ui.wait_for_url(&app_origin(&stack), "/google", REDIRECT_TIMEOUT)
        .await?;
    ui.wait_for_text(CONNECTED_ACCOUNTS, FEED_TIMEOUT).await?;

    // Back to the Feed; a reload fetches both mailboxes.
    ui.tap(TAB_FEED).await?;
    ui.reload().await?;
    ui.wait_for_text(KEEP_BUTTON, FEED_TIMEOUT).await?;
    Ok(ui)
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac1_e2e_two_mailboxes_interleaved_newest_first(
) -> Result<(), Box<dyn std::error::Error>> {
    let ui = two_mailbox_feed().await?;

    // The card in focus is the newest overall, then the other mailbox's mail,
    // then A's oldest: t3 (A), t2 (B), t1 (A).
    ui.wait_for_semantic_text(A_NEWEST, FEED_TIMEOUT).await?;
    ui.tap(KEEP_BUTTON).await?;
    ui.wait_for_semantic_text(B_MIDDLE, FEED_TIMEOUT).await?;
    ui.tap(KEEP_BUTTON).await?;
    ui.wait_for_semantic_text(A_OLDEST, FEED_TIMEOUT).await?;
    ui.tap(KEEP_BUTTON).await?;
    ui.wait_for_text(NOTHING_TO_TRIAGE, FEED_TIMEOUT).await?;

    ui.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac2_e2e_cards_show_mailbox() -> Result<(), Box<dyn std::error::Error>> {
    let ui = two_mailbox_feed().await?;

    // Each card shows the mailbox address it came from (FD-02 AC2).
    ui.wait_for_semantic_text(A_NEWEST, FEED_TIMEOUT).await?;
    ui.wait_for_semantic_text(EMAIL_A, FEED_TIMEOUT).await?;
    ui.tap(KEEP_BUTTON).await?;
    ui.wait_for_semantic_text(B_MIDDLE, FEED_TIMEOUT).await?;
    ui.wait_for_semantic_text(EMAIL_B, FEED_TIMEOUT).await?;
    ui.tap(KEEP_BUTTON).await?;
    ui.wait_for_semantic_text(A_OLDEST, FEED_TIMEOUT).await?;
    ui.wait_for_semantic_text(EMAIL_A, FEED_TIMEOUT).await?;

    ui.close().await?;
    Ok(())
}
