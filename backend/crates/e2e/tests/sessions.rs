#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 2 (AU-07 AC1): signing in again ends the earlier session.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`). A second
//! sign-in of the same account ends the first browser's session on the server;
//! the first browser's next request is refused with `401`, the app wipes and
//! shows the Sign-in default with no card text (S10 3.3, S2 AU-07 AC1).

use std::time::Duration;

use e2e::{
    signed_in_user, tap_continue_with_google, FakeGoogle, Stack, Ui, APP_LOAD_TIMEOUT,
    CONTINUE_WITH_GOOGLE, FEED_TIMEOUT, KEEP_BUTTON,
};

/// A per-journey `sub` so journeys never see each other's mail.
const SUB: &str = "sub-au-07-a";
/// The seeded account, a reserved domain (S10 5).
const EMAIL: &str = "au07@example.com";
/// The sender of the seeded card (corpus case `one-click-covered`).
const CARD_SENDER: &str = "Acme News";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_07_ac1_e2e_second_sign_in_ends_first_session() -> Result<(), Box<dyn std::error::Error>>
{
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;

    // Session 1: invited sign-in for A, landing on the Feed.
    let ui1 = signed_in_user(&stack, SUB, EMAIL, &["one-click-covered"]).await?;
    ui1.wait_for_semantic_text(CARD_SENDER, FEED_TIMEOUT)
        .await?;

    // Session 2: the same account signs in again through the plain Sign-in
    // screen (no invite). The account already exists, so `join` signs it in.
    google.select_account_for_next_authorize(SUB).await?;
    let ui2 = Ui::open(&stack, "/#/").await?;
    ui2.wait_for_text(CONTINUE_WITH_GOOGLE, APP_LOAD_TIMEOUT)
        .await?;
    tap_continue_with_google(&ui2).await?;
    ui2.wait_for_text(KEEP_BUTTON, FEED_TIMEOUT).await?;

    // Session 1's next request is refused with 401: the app wipes and shows the
    // Sign-in default, with no card text left behind.
    ui1.tap(KEEP_BUTTON).await?;
    ui1.wait_for_text(CONTINUE_WITH_GOOGLE, FEED_TIMEOUT)
        .await?;
    ui1.wait_for_text_absent(CARD_SENDER, Duration::from_secs(5))
        .await?;

    ui1.close().await?;
    ui2.close().await?;
    Ok(())
}
