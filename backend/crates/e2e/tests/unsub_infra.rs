#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! T-1101g: the end-to-end unsubscribe plumbing smoke test.
//!
//! Proves the T-1101g wiring before the full journeys (T-1101e): the `api` runs
//! in e2e mode with the local job runner and the `MT_E2E_UNSUB_DELAY_S` due
//! delay, delivers the queued job to the loopback `unsub`, and `unsub`
//! completes exactly one TLS one-click POST to the testbed, trusting only that
//! run's CA. Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`),
//! the job that has fake-google, the Firestore emulator, the testbed and
//! ChromeDriver.
//!
//! The one-click message is the corpus case `one-click-covered` with its
//! `List-Unsubscribe` target rewritten to this run's testbed TLS listener (a
//! literal loopback IP, which is the only host `unsub`'s e2e egress allows);
//! every other header, including the DKIM coverage of both unsubscribe headers,
//! is the fixture's.

use std::error::Error;
use std::time::{Duration, Instant};

use base64::Engine as _;
use e2e::{finish_journey, signed_in_user, FakeGoogle, Stack, Testbed, Ui, REJECT_BUTTON};
use serde_json::json;

/// The account this smoke test signs in as; unique so it never sees another
/// journey's mail. Reserved domains only.
const SUB: &str = "sub-unsub-infra-smoke";
const EMAIL: &str = "unsub-infra@example.com";
/// The corpus case whose headers classify as a covered one-click list message.
const FIXTURE: &str = "one-click-covered";
/// The target the corpus case ships with, replaced for this run.
const FIXTURE_TARGET: &str = "https://u.example.com/weekly";
/// The testbed route the seeded message points at.
const ROUTE: &str = "/oneclick/200";
/// The due delay the api plans queued jobs with (`MT_E2E_UNSUB_DELAY_S`).
const DUE_DELAY: Duration = Duration::from_secs(3);
/// How long to keep polling for the POST after the due time.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(20);
/// A deterministic `internal_date` (S10 1 rule 2: the harness never reads the
/// wall clock for seeded data).
const INTERNAL_DATE: &str = "2026-10-02T09:00:00+00:00";

/// The one-click `.eml` for this run: the corpus case with its
/// `List-Unsubscribe` target pointing at the testbed's TLS listener.
fn one_click_eml(testbed_https: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let corpus = testkit::corpus::load().map_err(|e| format!("corpus: {e}"))?;
    let case = corpus
        .case(FIXTURE)
        .ok_or_else(|| format!("no corpus case {FIXTURE}"))?;
    let text = String::from_utf8(case.eml.clone())?;
    let target = format!("{}{ROUTE}", testbed_https.trim_end_matches('/'));
    Ok(text.replace(FIXTURE_TARGET, &target).into_bytes())
}

/// The testbed's read-back control URL.
fn control_url(stack: &Stack, path: &str) -> String {
    format!("{}{path}", stack.testbed.as_str().trim_end_matches('/'))
}

/// Clear the testbed's recorded requests, so this test never sees another
/// journey's traffic (S10 6.2).
async fn reset_testbed(stack: &Stack) -> Result<(), Box<dyn Error>> {
    let response = reqwest::Client::new()
        .post(control_url(stack, "/__testbed/reset"))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("testbed reset: {}", response.status()).into());
    }
    Ok(())
}

/// Seed one raw `.eml` into the stack's fake Gmail for [`EMAIL`].
async fn seed_message(stack: &Stack, eml: &[u8]) -> Result<(), Box<dyn Error>> {
    let url = format!(
        "{}/__fake/gmail/messages",
        stack.fake_google.as_str().trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&json!({
            "email": EMAIL,
            "eml_base64": base64::engine::general_purpose::STANDARD.encode(eml),
            "labels": ["INBOX"],
            "internal_date": INTERNAL_DATE,
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("seed message: {}", response.status()).into());
    }
    Ok(())
}

/// Poll `route` until the testbed has recorded a request or `timeout` passes.
async fn wait_for_post(
    testbed: &Testbed,
    route: &str,
    timeout: Duration,
) -> Result<Vec<e2e::RecordedRequest>, Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        let records = testbed.received(route).await?;
        if !records.is_empty() || Instant::now() >= deadline {
            return Ok(records);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// E2E-INFRA AC1: a queued unsubscribe job becomes due within the e2e delay
/// and the testbed receives exactly one POST.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn e2e_infra_ac1_job_due_and_posted_once() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    // The literal-loopback TLS base of this run's testbed (`unsub` refuses a
    // hostname, so the target must be the IP).
    let testbed_https = std::env::var("MT_E2E_TESTBED_HTTPS_URL")?;
    let google = FakeGoogle::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;

    // One reset and one client registration for the whole run; the account is
    // seeded here so the message has a mailbox to land in before sign-in.
    google.reset().await?;
    google.register_client(&stack).await?;
    google.seed_account(SUB, EMAIL, true).await?;
    reset_testbed(&stack).await?;
    seed_message(&stack, &one_click_eml(&testbed_https)?).await?;

    let ui: Ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.tap(REJECT_BUTTON).await?;

    // The api plans `due_at = now + MT_E2E_UNSUB_DELAY_S`; poll past it.
    let records = wait_for_post(&testbed, ROUTE, DUE_DELAY + DELIVERY_TIMEOUT).await?;
    assert_eq!(
        records.len(),
        1,
        "exactly one POST must reach the testbed within the e2e delay"
    );

    let record = &records[0];
    assert_eq!(record.method, "POST");
    assert_eq!(record.body, b"List-Unsubscribe=One-Click");
    let header = |name: &str| {
        record
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert!(header("cookie").is_none(), "no Cookie header");
    assert!(header("authorization").is_none(), "no Authorization header");

    // Runs once: a second delivery would be a second record.
    tokio::time::sleep(DUE_DELAY).await;
    assert_eq!(
        testbed.received(ROUTE).await?.len(),
        1,
        "the queued unsubscribe runs exactly once"
    );

    finish_journey(ui).await?;
    Ok(())
}
