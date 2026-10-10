#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 3 (FD-02 AC1, AC2): a Feed that interleaves two connected mailboxes
//! by received time, each card showing its mailbox. Run only by
//! `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).
//!
//! The Feed is a positional stream: a card is fetched once and its position
//! advances, so the three cards must come from one page. Mailbox A therefore
//! signs in with a single throwaway card (the sign-in page consumes it and sets
//! A's floor). After linking B, a throwaway card sets B's floor too; the
//! journey's fixtures are seeded afterwards, all newer than those cards:
//! A at t1 and t3, B at t2. The Feed's next load returns all three,
//! newest first across both mailboxes, each card naming its mailbox. The card in
//! focus is read from its semantics label: the card behind it is excluded from
//! semantics, so the label is exactly the focused card.

use std::error::Error;
use std::time::Duration;

use e2e::{
    connect_second_mailbox, finish_journey, signed_in_user, EventLog, FakeGoogle, Stack, Ui,
};

/// A's throwaway card: the sign-in Feed load fetches it and advances A's floor.
const FIXTURE_MARKER: &str = "same-sender-list-a";
/// A's oldest journey fixture (t1), a one-click list with a remote pixel.
const FIXTURE_T1: &str = "remote-images";
/// The sender [`FIXTURE_T1`] renders on its card.
const SENDER_T1: &str = "Acme Trackers";
/// B's fixture (t2), between A's two.
const FIXTURE_T2: &str = "esp-pass-dmarc-fail";
/// The sender [`FIXTURE_T2`] renders on its card.
const SENDER_T2: &str = "Acme Deals";
/// A's newest fixture (t3).
const FIXTURE_T3: &str = "one-click-covered";
/// The sender [`FIXTURE_T3`] renders on its card.
const SENDER_T3: &str = "Acme News";

/// Minutes past the harness's deterministic base for each seeded message, in
/// received-time order: the marker first, then A's t1, B's t2 and A's t3.
const MARKER_MINUTE: i64 = 1;
const T1_MINUTE: i64 = 2;
const T2_MINUTE: i64 = 3;
const T3_MINUTE: i64 = 4;

/// The Keep button's Semantics label (SW-01): reads the next card.
const KEEP: &str = "Keep";
/// The Feed after the link, once the api has fetched and classified both
/// mailboxes.
const FEED_TIMEOUT: Duration = Duration::from_secs(120);

/// Signs A in, links B, and lands on the Feed with both mailboxes interleaved.
///
/// Each journey seeds its own accounts so tests never see each other's mail.
/// The step-up the Add Gmail flow raises re-authenticates A, so that login is
/// scripted just before the link.
async fn two_mailbox_feed(
    stack: &Stack,
    sub_a: &str,
    email_a: &str,
    sub_b: &str,
    email_b: &str,
) -> Result<Ui, Box<dyn Error>> {
    let google = FakeGoogle::connect(stack)?;
    google.reset().await?;
    google.register_client(stack).await?;

    // A alone, with one throwaway card for the sign-in Feed load.
    google.seed_account(sub_a, email_a, true).await?;
    google
        .seed_message_at(sub_a, FIXTURE_MARKER, MARKER_MINUTE)
        .await?;
    let mut ui = signed_in_user(stack, sub_a, email_a, &[]).await?;

    // B's mailbox must exist before the link fetches it.
    google.seed_account(sub_b, email_b, true).await?;

    // The step-up popup re-authenticates A before the link starts.
    google.select_account_for_next_authorize(sub_a).await?;
    connect_second_mailbox(&mut ui, stack, email_b).await?;

    // A mailbox with no previously seen card starts in backlog, not the new
    // phase. Give B the same baseline as A before introducing the three new
    // cards, otherwise B's t2 belongs to a different phase and cannot interleave.
    google
        .seed_message_at(sub_b, FIXTURE_MARKER, MARKER_MINUTE)
        .await?;
    let origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.goto(&format!("{origin}/")).await?;
    ui.wait_for_text("Continue", FEED_TIMEOUT).await?;
    ui.tap("Continue").await?;
    ui.wait_for_card(&["Acme Both", email_b], FEED_TIMEOUT)
        .await?;

    // The three cards, all newer than the throwaway card, land in one page.
    google.seed_message_at(sub_a, FIXTURE_T1, T1_MINUTE).await?;
    google.seed_message_at(sub_b, FIXTURE_T2, T2_MINUTE).await?;
    google.seed_message_at(sub_a, FIXTURE_T3, T3_MINUTE).await?;

    // A fresh load at the app root rebuilds the Feed from both floors, so the
    // three new cards interleave within the same phase.
    ui.goto(&format!("{origin}/")).await?;
    Ok(ui)
}

/// Journey 3: the Feed interleaves the two mailboxes newest first.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac1_e2e_two_mailboxes_interleaved_newest_first() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    // Read only the metric events this journey writes (S10 8).
    let events = EventLog::new();
    let mark = events.mark();

    let ui = two_mailbox_feed(
        &stack,
        "sub-mixed-feed-a",
        "mixed-feed-a@example.com",
        "sub-mixed-feed-b",
        "mixed-feed-b@example.com",
    )
    .await?;

    // Newest first across both mailboxes: A's t3, then B's t2, then A's t1; each
    // card carries its mailbox's address (FD-02 AC2).
    ui.wait_for_card(&[SENDER_T3, "mixed-feed-a@example.com"], FEED_TIMEOUT)
        .await?;
    ui.tap(KEEP).await?;
    ui.wait_for_card(&[SENDER_T2, "mixed-feed-b@example.com"], FEED_TIMEOUT)
        .await?;
    ui.tap(KEEP).await?;
    ui.wait_for_card(&[SENDER_T1, "mixed-feed-a@example.com"], FEED_TIMEOUT)
        .await?;

    // Journey 3 emits no metric event today (S10 8).
    let emitted = events.since_mark(mark)?;
    assert!(
        emitted.is_empty(),
        "journey 3 emitted metric events: {emitted:?}"
    );

    finish_journey(ui).await?;
    Ok(())
}

/// FD-02 AC2: every card in the interleaved Feed shows its own mailbox address.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn fd_02_ac2_e2e_cards_show_mailbox() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let ui = two_mailbox_feed(
        &stack,
        "sub-cards-mailbox-a",
        "cards-a@example.com",
        "sub-cards-mailbox-b",
        "cards-b@example.com",
    )
    .await?;

    // Each focused card is labelled with its sender and the address of the
    // mailbox it came from; read all three in turn.
    ui.wait_for_card(&[SENDER_T3, "cards-a@example.com"], FEED_TIMEOUT)
        .await?;
    ui.tap(KEEP).await?;
    ui.wait_for_card(&[SENDER_T2, "cards-b@example.com"], FEED_TIMEOUT)
        .await?;
    ui.tap(KEEP).await?;
    ui.wait_for_card(&[SENDER_T1, "cards-a@example.com"], FEED_TIMEOUT)
        .await?;

    finish_journey(ui).await?;
    Ok(())
}
