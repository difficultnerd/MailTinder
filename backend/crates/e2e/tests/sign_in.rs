#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 1 (AU-03 AC1): in a real browser, an invited user opens the invite
//! link, signs in with Google and lands on the Feed. Run only by
//! `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).

use std::time::Duration;

use e2e::{FakeGoogle, Stack, TestControl, Ui};

/// The invited copy shown on the Sign-in screen (S2 AU-03 AC1; `Copy.invited`).
const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";
/// The Google button's Semantics label (XC-03).
const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The sender of the newest seeded message, shown on the Feed card.
const NEWEST_SENDER: &str = "Acme News";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_03_ac1_e2e_invited_user_lands_on_feed() -> Result<(), Box<dyn std::error::Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let control = TestControl::connect(&stack)?;

    google.reset().await?;
    google
        .seed_account("sub-invitee", "invitee@example.com", true)
        .await?;
    // The last id is the newest message; journey waits for its sender.
    google
        .seed_messages(
            "sub-invitee",
            &[
                "one-click-plus-mailto",
                "esp-pass-dmarc-fail",
                "one-click-covered",
            ],
        )
        .await?;
    google
        .select_account_for_next_authorize("sub-invitee")
        .await?;

    let token = control.create_invite("invitee@example.com").await?;
    let ui = Ui::open(&stack, &format!("/#/invite?t={token}")).await?;

    ui.wait_for_text(INVITED_COPY, Duration::from_secs(30))
        .await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.wait_for_text(NEWEST_SENDER, Duration::from_secs(60))
        .await?;
    ui.close().await?;
    Ok(())
}
