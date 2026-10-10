#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 2 (AU-07 AC1): a user has one session, so signing in again ends the
//! earlier one; the earlier browser's next request is refused with 401, it
//! wipes and shows the Sign-in default. Run only by `scripts/e2e.sh`
//! (`cargo test -p e2e -- --ignored`).
//!
//! Two browser sessions sign in as the same seeded account. The first lands on
//! the Feed with one card; the second sign-in ends the first's server session.
//! The first then taps "Keep": the swipe is refused, the app wipes and the
//! Sign-in default returns ("Continue with Google" visible, no card text).
//! Journey 2 emits no metric event (S10 8), asserted after the mark.

use std::error::Error;
use std::time::Duration;

use e2e::{finish_journey, signed_in_user, EventLog, FakeGoogle, Stack, Ui};

/// The account both sessions sign in as; unique to this journey so it never
/// sees another journey's mail.
const SUB: &str = "sub-sessions-journey";
const EMAIL: &str = "sessions@example.com";
/// The single corpus case seeded for session 1; its sender is the card's text.
const FIXTURE: &str = "one-click-covered";
/// The sender of [`FIXTURE`], shown on the Feed card and gone after the wipe.
const SENDER: &str = "Acme News";
/// The Google button's Semantics label, shown again after the 401 wipe.
const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The Keep button's Semantics label (SW-01): the request session 1 makes.
const KEEP: &str = "Keep";
/// How long the wiped session takes to paint the Sign-in default.
const WIPE_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_07_ac1_e2e_second_sign_in_ends_first_session() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    // One reset and client registration for both sign-ins; `signed_in_user`
    // seeds the account and drives the OAuth round-trip per session.
    google.reset().await?;
    google.register_client(&stack).await?;

    // Read only the metric events this journey writes.
    let events = EventLog::new();
    let mark = events.mark();

    // Session 1: sign in and land on the Feed with one seeded card.
    let first: Ui = signed_in_user(&stack, SUB, EMAIL, &[FIXTURE]).await?;
    // Session 2: signing in as the same account ends session 1 on the server.
    let second = signed_in_user(&stack, SUB, EMAIL, &[]).await?;

    // Session 1's next request is refused (401): tapping Keep wipes the app and
    // returns it to the Sign-in default.
    first.tap(KEEP).await?;
    first
        .wait_for_text(CONTINUE_WITH_GOOGLE, WIPE_TIMEOUT)
        .await?;
    // No card text survives the wipe.
    first.wait_for_text_absent(SENDER, WIPE_TIMEOUT).await?;

    // Journey 2 emits no metric event (S10 8).
    let emitted = events.since_mark(mark)?;
    assert!(
        emitted.is_empty(),
        "journey 2 emitted metric events: {emitted:?}"
    );

    second.close().await?;
    finish_journey(first).await?;
    Ok(())
}
