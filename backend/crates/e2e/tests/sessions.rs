#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 2 (AU-07 AC1): signing in again ends the earlier session; its next
//! request is refused and it shows Sign-in. Run only by `scripts/e2e.sh`
//! (`cargo test -p e2e -- --ignored`).

use std::time::Duration;

use e2e::{signed_in_user, FakeGoogle, Stack, Ui, CONTINUE_WITH_GOOGLE, FEED_TAB};

/// A short wait for a state that is already on screen.
const SOON: Duration = Duration::from_secs(20);
/// A long wait for a network round trip.
const LOAD: Duration = Duration::from_secs(60);

/// The corpus case seeded onto the Feed; its sender is [`SENDER`].
const SENDER_FIXTURE: &str = "one-click-covered";
/// The sender shown on the one seeded card.
const SENDER: &str = "Acme News";

/// AU-07 AC1: a second sign-in ends the first session; the first session's next
/// request is refused and it falls back to the Sign-in default with no card.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_07_ac1_e2e_second_sign_in_ends_first_session() -> Result<(), Box<dyn std::error::Error>>
{
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    google.reset().await?;
    google
        .seed_account("sub-au07", "au07@example.com", true)
        .await?;
    google.seed_messages("sub-au07", &[SENDER_FIXTURE]).await?;

    // Session 1: sign in through the invite and land on the Feed.
    let first = signed_in_user(&stack, "sub-au07", "au07@example.com", &[]).await?;
    first.wait_for_text(SENDER, LOAD).await?;

    // Session 2: the same account signs in again, ending session 1 (AU-07 AC1).
    google.select_account_for_next_authorize("sub-au07").await?;
    let second = Ui::open(&stack, "/#/sign-in").await?;
    second.wait_for_text(CONTINUE_WITH_GOOGLE, LOAD).await?;
    second.tap(CONTINUE_WITH_GOOGLE).await?;
    second.wait_for_text(FEED_TAB, LOAD).await?;

    // Session 1's next request is refused: the app wipes and shows the
    // Sign-in default, with no card text left behind.
    first.tap("Keep").await?;
    first.wait_for_text(CONTINUE_WITH_GOOGLE, LOAD).await?;
    first.wait_for_text_absent(SENDER, SOON).await?;

    first.close().await?;
    second.close().await?;
    Ok(())
}
