//! AU-03 AC1: full-page OAuth redirects are driven outside the Flutter app.

use std::time::Duration;

use e2e::{E2eError, FakeGoogle, Stack, TestControl, Ui};

const FIXTURES: [&str; 3] = [
    "one-click-covered",
    "one-click-plus-mailto",
    "esp-pass-dmarc-fail",
];
const NEWEST_SENDER: &str = "Acme Deals CANARY-esp-pass-dmarc-fail-display";
const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_03_ac1_e2e_invited_user_lands_on_feed() -> Result<(), E2eError> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::new(&stack);
    let control = TestControl::new(&stack);
    let sub = "e2e-invitee";
    google
        .seed_account(sub, "invitee@example.com", true)
        .await?;
    google.seed_messages(sub, &FIXTURES).await?;
    google.select_account_for_next_authorize(sub).await?;
    let token = control.create_invite("invitee@example.com").await?;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("t", &token)
        .finish();
    let ui = Ui::open(&stack, &format!("/invite?{query}")).await?;
    let journey = async {
        ui.wait_for_text(INVITED_COPY, Duration::from_secs(30))
            .await?;
        ui.tap("Continue with Google").await?;
        ui.wait_for_text(NEWEST_SENDER, Duration::from_secs(60))
            .await
    }
    .await;
    // Delete the browser session even when the assertion or OAuth flow fails.
    let closed = ui.close().await;
    journey?;
    closed
}

#[test]
fn au_03_ac1_e2e_journey_uses_three_real_ordered_corpus_fixtures() -> Result<(), E2eError> {
    let corpus = testkit::corpus::load().map_err(E2eError::Control)?;
    let cases = FIXTURES
        .iter()
        .map(|id| {
            corpus
                .cases
                .iter()
                .find(|case| case.spec.id == *id)
                .ok_or_else(|| E2eError::Control("journey fixture missing".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(cases.len(), 3);
    assert!(cases
        .windows(2)
        .all(|pair| pair[0].spec.date < pair[1].spec.date));
    let newest = cases
        .last()
        .ok_or_else(|| E2eError::Control("journey fixtures empty".into()))?;
    assert_eq!(
        newest.seed_message().from_display,
        NEWEST_SENDER,
        "the browser assertion must identify the newest actual corpus message"
    );
    Ok(())
}
