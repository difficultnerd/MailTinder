#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 8 (AU-05 AC1): in a real browser the user with two connected
//! mailboxes disconnects one while one of its unsubscribes is still queued. The
//! disconnect revokes that mailbox's grant at the provider, cancels its queued
//! job and removes its cards from the Feed.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the Firestore emulator, the testbed and ChromeDriver. Run
//! with `--test-threads=1` (as the script does): the tests share the global
//! `fake-google` and clear the one testbed, so parallel execution would corrupt
//! state.
//!
//! Two time controls carry the journey, both testkit-only (S10 6.3):
//! `/internal/test/unsub-delay` puts the queued job's due time minutes away, so
//! the disconnect below fits inside its window (the run's configured delay is
//! seconds, T-1101g), and `/internal/test/advance-clock` then jumps the api's
//! clock past that due time, so a job that had *not* been cancelled would reach
//! the testbed and fail the journey. The two are undone before the journey ends,
//! because one api process serves every journey of the run.

use std::error::Error;
use std::time::Duration;

use e2e::{
    connect_second_mailbox, document_string, documents_in, finish_journey, firestore_emulator_url,
    mailbox_path_for_subject, note_step, signed_in_user, FakeGoogle, FirestoreDump, Stack,
    TestControl, Testbed, Ui, ADD_GMAIL, APP_LOAD_TIMEOUT, CONFIRM_ITS_YOU, CONTINUE_WITH_GOOGLE,
    E2E_FIRESTORE_PROJECT, FEED_TIMEOUT, LINK_TIMEOUT, REJECT_BUTTON, TAP_ACK_TIMEOUT,
};

/// The two accounts this journey links; unique so they never see another
/// journey's mail. Reserved domains only.
const SUB_A: &str = "sub-au-05-ac1-e2e-a";
const EMAIL_A: &str = "au-05-ac1-e2e-a@example.com";
const SUB_B: &str = "sub-au-05-ac1-e2e-b";
const EMAIL_B: &str = "au-05-ac1-e2e-b@example.com";

/// A throwaway card the sign-in Feed load consumes, so each mailbox has seen a
/// card before the journey's own mail arrives (journey 3's pattern: the Feed is
/// a positional stream).
const FIXTURE_MARKER: &str = "same-sender-list-a";
/// The sender [`FIXTURE_MARKER`] renders on its card.
const MARKER_SENDER: &str = "Acme Both";
/// The minute past the harness's deterministic base for each mailbox's marker.
const MARKER_MINUTE: i64 = 1;

/// The corpus case whose headers classify as a covered one-click list message:
/// the reject queues an unsubscribe whose failure would be visible at the
/// testbed (T-1101g).
const FIXTURE: &str = "one-click-covered";
/// The target the corpus case ships with, replaced for this run.
const FIXTURE_TARGET: &str = "https://u.example.com/weekly";
/// The testbed route the seeded message points at (S10 6.2).
const ROUTE: &str = "/oneclick/200";
/// The sender of [`FIXTURE`]: the text the Feed card shows.
const SENDER: &str = "Acme News";
/// A deterministic `internal_date` (S10 1 rule 2: the harness never reads the
/// wall clock for seeded data).
const INTERNAL_DATE: &str = "2026-10-02T09:00:00+00:00";

/// The Settings tab, the Connected accounts screen and its Disconnect action
/// (S9 7, 7.1). A bottom navigation destination merges its icon's and its text
/// label, so its Semantics label is the label twice over: `tap_merged` matches a
/// part.
const SETTINGS_TAB: &str = "Settings";
const CONNECTED_ACCOUNTS: &str = "Connected accounts";
const DISCONNECT: &str = "Disconnect";
/// The start of the toast the Feed shows while the unsubscribe is queued (S2
/// SW-03 AC2); the delay it names is the api's, so only these words are
/// asserted.
const QUEUED_TOAST: &str = "Trashed. Unsubscribing in";

/// The delay the journey queues B's unsubscribe with: minutes, so the disconnect
/// below (a few browser round trips) finishes long before the job's due time.
const QUEUED_FOR_S: u64 = 240;
/// Past [`QUEUED_FOR_S`]: the jump that makes an uncancelled job due.
const SIX_MINUTES_S: i64 = 6 * 60;
/// How long the api's delivery loop and `unsub` get to deliver a job the clock
/// jump made due (the loop polls every 10 ms; `unsub` then makes the one-click
/// POST). A cancelled job sends nothing, so this is the window the absence of a
/// request is judged over.
const DELIVERY_WINDOW: Duration = Duration::from_secs(5);
/// How long the Confirm-it's-you overlay may take to appear after the
/// disconnect's `403 step_up_required`, before the journey concludes the
/// session's own step-up still covered it (the link above refreshed it).
const STEP_UP_GRACE: Duration = Duration::from_secs(10);

/// The symbolic `fake-google` routes this journey asserts on
/// (`/__fake/events`).
const REVOKE_ROUTE: &str = "oauth.revoke";

/// The prompt `Copy.disconnectQuestion` renders for `address`.
fn disconnect_question(address: &str) -> String {
    format!("Disconnect {address}? Its cards leave your Feed and its queued unsubscribes are cancelled.")
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_05_ac1_e2e_disconnect_revokes_and_cancels_jobs() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let testbed_https = std::env::var("MT_E2E_TESTBED_HTTPS_URL")?;
    let google = FakeGoogle::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;
    let control = TestControl::connect(&stack)?;

    let ui = two_mailboxes(&stack, &google).await?;
    testbed.reset().await?;

    // B's one-click card, newer than every card above, is the one the Feed
    // shows. Queue its unsubscribe with a due time the disconnect below fits
    // inside.
    control.set_unsub_delay(Some(QUEUED_FOR_S)).await?;
    google
        .seed_one_click_message(
            EMAIL_B,
            FIXTURE,
            FIXTURE_TARGET,
            ROUTE,
            &testbed_https,
            INTERNAL_DATE,
        )
        .await?;
    let origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.goto(&format!("{origin}/")).await?;
    ui.wait_for_card(&[SENDER, EMAIL_B], FEED_TIMEOUT).await?;
    ui.tap(REJECT_BUTTON).await?;
    ui.wait_for_text(QUEUED_TOAST, FEED_TIMEOUT).await?;

    // The job is queued for B and still open: the identifier its document and
    // the job carry, read before the disconnect deletes the mailbox.
    let emulator = firestore_emulator_url()?;
    let before = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    let mailbox_b = mailbox_path_for_subject(&before, EMAIL_B)
        .ok_or_else(|| format!("no mailbox document for {EMAIL_B}"))?
        .to_owned();
    let mailbox_b_id = mailbox_b.rsplit('/').next().unwrap_or_default().to_owned();
    assert!(
        !job_documents_for(&before, &mailbox_b_id).is_empty(),
        "the reject must leave a queued job for B"
    );

    // Settings, Connected accounts, then B's own Disconnect. Every row repeats
    // that label, so the harness clicks the one beside B's address.
    ui.tap_merged(SETTINGS_TAB).await?;
    ui.wait_for_text(CONNECTED_ACCOUNTS, TAP_ACK_TIMEOUT)
        .await?;
    ui.tap_merged(CONNECTED_ACCOUNTS).await?;
    ui.wait_for_text(ADD_GMAIL, APP_LOAD_TIMEOUT).await?;
    ui.tap_after(DISCONNECT, EMAIL_B).await?;
    ui.wait_for_text(&disconnect_question(EMAIL_B), TAP_ACK_TIMEOUT)
        .await?;
    // The session may or may not need a fresh sign-in: the link above proved one
    // minutes ago, so the server usually accepts the disconnect straight away.
    // The overlay only appears when it answers `403 step_up_required` (AU-04 AC6).
    ui.tap(DISCONNECT).await?;
    step_up_if_prompted(&ui, &google, SUB_A, &origin).await?;

    // The mailbox's row is gone from Connected accounts, and the run's own
    // unsubscribe delay is back for every journey after this one.
    ui.wait_for_text_absent(EMAIL_B, TAP_ACK_TIMEOUT).await?;
    control.set_unsub_delay(None).await?;

    // AU-05 AC1: the provider grant was revoked.
    let calls = google.calls().await?;
    let revokes = calls
        .iter()
        .filter(|call| call.route == REVOKE_ROUTE)
        .count();
    assert_eq!(
        revokes, 1,
        "disconnecting must revoke exactly that mailbox's grant: {calls:?}"
    );
    assert_eq!(
        google.revocations().await?,
        vec!["refresh".to_owned()],
        "the revoked token must be the refresh token"
    );

    // AU-05 AC1: the queued job is cancelled, so nothing is left for B.
    let after = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    assert!(
        mailbox_path_for_subject(&after, EMAIL_B).is_none(),
        "the disconnected mailbox's document must be gone"
    );
    let remaining = job_documents_for(&after, &mailbox_b_id);
    assert!(
        remaining.is_empty(),
        "the disconnect must cancel the mailbox's queued job: {remaining:?}"
    );

    // Past the due time the job would have been due - the api's own runner
    // delivers within a poll of it - so a job that had survived the disconnect
    // reaches the testbed here. It was cancelled, so nothing arrives.
    control.advance_clock(SIX_MINUTES_S).await?;
    tokio::time::sleep(DELIVERY_WINDOW).await;
    let recorded = testbed.received_any().await?;
    assert!(
        recorded.is_empty(),
        "the cancelled job must not reach the sender: the testbed saw {recorded:?}"
    );
    // Back to real time: one api process serves every journey of the run.
    control.advance_clock(-SIX_MINUTES_S).await?;

    // AU-05 AC1: B's cards are gone from the Feed. The Connected accounts screen
    // is a pushed route, so the bottom navigation is not on screen: a fresh load
    // at the app root rebuilds the Feed (journey 3's way back), and the shell is
    // up before the card is judged absent, so the check cannot pass against a
    // page that never rendered.
    ui.goto(&format!("{origin}/")).await?;
    ui.wait_for_text(NEEDS_ATTENTION_TAB, APP_LOAD_TIMEOUT)
        .await?;
    ui.wait_for_card_absent(&[SENDER, EMAIL_B], FEED_TIMEOUT)
        .await?;

    finish_journey(ui).await?;
    Ok(())
}

/// The Needs Attention tab's label, used only to prove the shell rendered.
const NEEDS_ATTENTION_TAB: &str = "Needs Attention";

/// Sign A in, link B through the UI (journey 3's helper) and land on the Feed
/// with both mailboxes' marker cards consumed.
async fn two_mailboxes(stack: &Stack, google: &FakeGoogle) -> Result<Ui, Box<dyn Error>> {
    google.reset().await?;
    google.register_client(stack).await?;

    // A alone, with one throwaway card for the sign-in Feed load.
    google.seed_account(SUB_A, EMAIL_A, true).await?;
    google
        .seed_message_at(SUB_A, FIXTURE_MARKER, MARKER_MINUTE)
        .await?;
    let mut ui = signed_in_user(stack, SUB_A, EMAIL_A, &[]).await?;

    // B's mailbox must exist before the link fetches it, and the step-up the Add
    // Gmail flow raises re-authenticates A.
    google.seed_account(SUB_B, EMAIL_B, true).await?;
    google.select_account_for_next_authorize(SUB_A).await?;
    connect_second_mailbox(&mut ui, stack, EMAIL_B).await?;

    // B needs the same baseline as A before its own card arrives, otherwise its
    // card belongs to a different Feed phase.
    google
        .seed_message_at(SUB_B, FIXTURE_MARKER, MARKER_MINUTE)
        .await?;
    let origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.goto(&format!("{origin}/")).await?;
    ui.wait_for_text(CONTINUE, FEED_TIMEOUT).await?;
    ui.tap(CONTINUE).await?;
    ui.wait_for_card(&[MARKER_SENDER, EMAIL_B], FEED_TIMEOUT)
        .await?;
    Ok(ui)
}

/// The Feed's Continue button, on the phase boundary the helper above steps
/// across.
const CONTINUE: &str = "Continue";

/// The store's `queued` job status (`domain::JobStatus::Queued`).
const QUEUED_STATUS: &str = "queued";

/// The keys of the mailbox's queued-job documents. A cancelled job keeps its
/// document - target cleared, outcome kept - until its retention expires (S5
/// `jobs` row), so only a job the store still holds as `queued` proves the
/// disconnect left the mailbox's work in place.
fn job_documents_for(documents: &[(String, String)], mailbox_id: &str) -> Vec<String> {
    documents_in(documents, "jobs")
        .into_iter()
        .filter(|(_, json)| json.contains(mailbox_id))
        .filter(|(_, json)| document_string(json, "status").as_deref() == Some(QUEUED_STATUS))
        .map(|(path, _)| path.clone())
        .collect()
}

/// Complete the Confirm-it's-you popup when the disconnect asked for a fresh
/// sign-in, and record which path ran. `sub` is the signed-in user's Google
/// account, the one the step-up must re-authenticate.
async fn step_up_if_prompted(
    ui: &Ui,
    google: &FakeGoogle,
    sub: &str,
    origin: &str,
) -> Result<(), Box<dyn Error>> {
    if ui
        .wait_for_text(CONFIRM_ITS_YOU, STEP_UP_GRACE)
        .await
        .is_err()
    {
        note_step("au_05_ac1: the session's step-up covered the disconnect; no popup");
        return Ok(());
    }
    google.select_account_for_next_authorize(sub).await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.wait_for_popup(TAP_ACK_TIMEOUT).await?;
    ui.wait_for_url(origin, SETTINGS_ROUTE, LINK_TIMEOUT)
        .await?;
    ui.switch_to_main().await?;
    Ok(())
}

/// A route nested under Settings, so the popup's own URL never matches.
const SETTINGS_ROUTE: &str = "/settings";
