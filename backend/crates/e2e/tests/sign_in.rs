#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 1 (AU-03 AC1): in a real browser, an invited user opens the invite
//! link, signs in with Google and lands on the Feed. Run only by
//! `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`).
//!
//! Every step prints its elapsed time, and the current URL once the browser is
//! up, so a slow step on a cold CI runner is visible; on any timeout the
//! harness writes a screenshot, the page text/source, the semantics tree, the
//! console log and the ChromeDriver tail into `target/e2e-logs` (T-1101a).

use std::io::Write as _;
use std::time::{Duration, Instant};

use e2e::{finish_journey, FakeGoogle, Stack, TestControl, Ui};

/// The invited copy shown on the Sign-in screen (S2 AU-03 AC1; `Copy.invited`).
const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";
/// The Google button's Semantics label (XC-03).
const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The sender of the newest seeded message, shown on the Feed card.
const NEWEST_SENDER: &str = "Acme News";

/// A cold GitHub runner can take a long time to paint the app before the
/// semantics tree exposes a control, so the first wait is the generous one.
const APP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// The Feed after the OAuth round-trip, once the api has fetched and classified.
const FEED_TIMEOUT: Duration = Duration::from_secs(120);
/// How long to wait for the tapped button to react before re-tapping it.
const TAP_ACK_TIMEOUT: Duration = Duration::from_secs(10);
/// The OAuth round-trip from the invite screen back into the app.
const REDIRECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Print a step line with its elapsed time. `writeln!` to stdout, not
/// `println!` (banned by Clippy); `--nocapture` shows it in CI as it happens.
fn step(start: Instant, name: &str) {
    report(start, name, None);
}

/// As [step], also printing the browser's current URL.
async fn step_url(ui: &Ui, start: Instant, name: &str) {
    let url = ui
        .current_url()
        .await
        .map_or_else(|_| "(unavailable)".to_owned(), |u| u.to_string());
    report(start, name, Some(&url));
}

/// One `[e2e] step=...` line, flushed so it interleaves correctly with the
/// services' logs.
fn report(start: Instant, name: &str, url: Option<&str>) {
    let elapsed = start.elapsed().as_millis();
    let mut out = std::io::stdout().lock();
    let _ = match url {
        Some(url) => writeln!(out, "[e2e] step={name} elapsed={elapsed}ms url={url}"),
        None => writeln!(out, "[e2e] step={name} elapsed={elapsed}ms"),
    };
    let _ = out.flush();
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_03_ac1_e2e_invited_user_lands_on_feed() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let control = TestControl::connect(&stack)?;

    google.reset().await?;
    google.register_client(&stack).await?;
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
    step(start, "seeded");

    let token = control.create_invite("invitee@example.com").await?;
    step(start, "invite_created");

    let ui = Ui::open(&stack, &format!("/#/invite?t={token}")).await?;
    step_url(&ui, start, "browser_opened").await;

    ui.wait_for_text(INVITED_COPY, APP_LOAD_TIMEOUT).await?;
    step_url(&ui, start, "app_loaded").await;

    // Tap, then confirm the control reacted: the button swaps to a spinner, so
    // its label disappears as soon as the handler runs. If it did not, the click
    // was lost - re-tap once before giving up.
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    if ui
        .wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
        .await
        .is_err()
    {
        step_url(&ui, start, "tap_retry").await;
        ui.tap(CONTINUE_WITH_GOOGLE).await?;
        ui.wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
            .await?;
    }
    step_url(&ui, start, "sign_in_clicked").await;

    // OAuth round-trip: the browser leaves the invite screen and comes back.
    let app_origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.wait_for_url(&app_origin, "/invite", REDIRECT_TIMEOUT)
        .await?;
    step_url(&ui, start, "redirect_returned").await;

    ui.wait_for_text(NEWEST_SENDER, FEED_TIMEOUT).await?;
    step_url(&ui, start, "feed_loaded").await;

    finish_journey(ui).await?;
    Ok(())
}
