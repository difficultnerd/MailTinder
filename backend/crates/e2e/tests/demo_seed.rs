#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! T-1108b: `scripts/demo_seed.py` against a live stack.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the api and ChromeDriver. The first two drive the seed
//! against the real fake-google control API and count mail through the fake
//! Gmail list; the third opens the URL the seed prints in a real browser and
//! waits for the seeded sender on the Feed (AU-03 AC1).

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use e2e::{Stack, Ui};
use serde_json::{json, Value};

/// The invited copy shown on the Sign-in screen (S2 AU-03 AC1; `Copy.invited`).
const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";
/// The Google button's Semantics label (XC-03).
const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The seeded account (T-1108b).
const EMAIL: &str = "invitee@example.com";
/// The scope the fake Gmail list route requires (S8).
const GMAIL_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";
/// A cold runner can take a long time to paint the app before the semantics
/// tree exposes a control, so the first wait is the generous one.
const APP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// The Feed after the OAuth round-trip, once the api has fetched and classified.
const FEED_TIMEOUT: Duration = Duration::from_secs(120);
/// The OAuth round-trip from the invite screen back into the app.
const REDIRECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for the tapped button to react before re-tapping it.
const TAP_ACK_TIMEOUT: Duration = Duration::from_secs(10);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// Run `scripts/demo_seed.py` against `stack`; return the invite URL it prints.
fn run_demo_seed(stack: &Stack) -> Result<String, Box<dyn Error>> {
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/demo_seed.py"))
        .arg("--fake-google-url")
        .arg(stack.fake_google.as_str().trim_end_matches('/'))
        .arg("--api-url")
        .arg(stack.api_internal.as_str().trim_end_matches('/'))
        .arg("--app-origin")
        .arg(stack.app_url.as_str().trim_end_matches('/'))
        .arg("--json")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "demo_seed.py failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let summary: Value = serde_json::from_slice(&output.stdout)?;
    summary["invite_url"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "demo_seed.py printed no invite_url".into())
}

/// The seeded mailbox's message count for an optional Gmail `q` term.
///
/// A token is issued per call: the seed resets fake-google, which discards any
/// token minted before it.
async fn message_count(stack: &Stack, query: Option<&str>) -> Result<usize, Box<dyn Error>> {
    let base = stack.fake_google.as_str().trim_end_matches('/');
    let client = reqwest::Client::new();
    let issued: Value = client
        .post(format!("{base}/__fake/tokens"))
        .json(&json!({ "email": EMAIL, "scopes": [GMAIL_MODIFY], "ttl_s": 3600 }))
        .send()
        .await?
        .json()
        .await?;
    let token = issued["access_token"].as_str().ok_or("no access token")?;

    let mut url = format!("{base}/gmail/v1/users/me/messages?maxResults=500");
    if let Some(term) = query {
        let encoded: String = url::form_urlencoded::byte_serialize(term.as_bytes()).collect();
        url.push_str("&q=");
        url.push_str(&encoded);
    }
    let listed: Value = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await?
        .json()
        .await?;
    Ok(listed["resultSizeEstimate"].as_u64().unwrap_or(0) as usize)
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn demo_seed_creates_expected_messages() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    run_demo_seed(&stack)?;

    let expected = testkit::corpus::load()?.cases.len();
    assert_eq!(
        message_count(&stack, None).await?,
        expected,
        "the seeded mailbox does not hold the whole corpus"
    );
    // Every swipe type has something to act on (behaviour 2): a newsletter, a
    // promo and personal mail, each a corpus sender.
    for query in [
        "from:updates@example.com",
        "from:offers@example.com",
        "from:maya@example.com",
    ] {
        assert!(
            message_count(&stack, Some(query)).await? >= 1,
            "no seeded message matched {query}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn demo_seed_is_idempotent() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    run_demo_seed(&stack)?;
    let first = message_count(&stack, None).await?;
    run_demo_seed(&stack)?;
    let second = message_count(&stack, None).await?;
    assert!(first > 0, "the first seed left no messages");
    assert_eq!(first, second, "running the seed twice duplicated mail");
    Ok(())
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn demo_check_feed_shows_seeded_sender() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let invite_url = run_demo_seed(&stack)?;
    let token = invite_url
        .split("?t=")
        .nth(1)
        .ok_or("the invite URL carries no token")?;

    // The Feed shows the newest card first. The fake allocates message ids in
    // corpus order and the Feed orders equal timestamps by id, so the first
    // corpus case's sender is the one on the first card.
    let corpus = testkit::corpus::load()?;
    let sender = corpus
        .cases
        .first()
        .ok_or("empty corpus")?
        .spec
        .from_display
        .clone();

    let ui = Ui::open(&stack, &format!("/#/invite?t={token}")).await?;
    ui.wait_for_text(INVITED_COPY, APP_LOAD_TIMEOUT).await?;
    // Tap, then confirm the control reacted: the button swaps to a spinner, so
    // its label disappears as soon as the handler runs. If it did not, the
    // click was lost - re-tap once before giving up.
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    if ui
        .wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
        .await
        .is_err()
    {
        ui.tap(CONTINUE_WITH_GOOGLE).await?;
        ui.wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
            .await?;
    }
    let app_origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.wait_for_url(&app_origin, "/invite", REDIRECT_TIMEOUT)
        .await?;
    ui.wait_for_text(&sender, FEED_TIMEOUT).await?;
    ui.close().await?;
    Ok(())
}
