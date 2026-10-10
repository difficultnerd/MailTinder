#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 9 (AU-06 AC1): in a real browser the user deletes their account. The
//! deletion runs the app folder file delete and then the token revoke at the
//! provider (the fake records the order), leaves nothing in the store that names
//! the user, and, past the 24-hour horizon, the worker's sweep finds no leftover
//! to remove.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the Firestore emulator and ChromeDriver. Run with
//! `--test-threads=1` (as the script does): the tests share the global
//! `fake-google`, so parallel execution would corrupt state.
//!
//! The clock is jumped 25 hours rather than waited (`/internal/test/advance-clock`,
//! S10 6.3), and put back before the journey ends: one api process serves every
//! journey of the run.

use std::error::Error;
use std::time::Duration;

use e2e::{
    document_string, documents_holding, documents_in, finish_journey, firestore_emulator_url,
    mailbox_path_for_subject, signed_in_user, FakeGoogle, FirestoreDump, Stack, TestControl, Ui,
    APP_LOAD_TIMEOUT, CONFIRM_ITS_YOU, CONTINUE_WITH_GOOGLE, E2E_FIRESTORE_PROJECT, FEED_TIMEOUT,
    REDIRECT_TIMEOUT, TAP_ACK_TIMEOUT,
};

/// The account this journey deletes; unique so it never sees another journey's
/// mail. Reserved domains only.
const SUB: &str = "sub-au-06-ac1-e2e";
const EMAIL: &str = "au-06-ac1-e2e@example.com";
/// One seeded card, so the journey has something to swipe: a keep writes the
/// user state file into the mailbox's app folder, which the deletion then
/// deletes (AU-06 AC1's first recorded call).
const FIXTURE: &str = "one-click-covered";
/// The sender of [`FIXTURE`]: the text the Feed card shows.
const SENDER: &str = "Acme News";
/// The Feed's keep button (SW-01).
const KEEP: &str = "Keep";

/// Settings, its Account row and the account screen (S9 7.5). A bottom
/// navigation destination merges its icon's and its text label, so its Semantics
/// label is the label twice over: `tap_merged` matches a part.
const SETTINGS_TAB: &str = "Settings";
const ACCOUNT: &str = "Account";
const DELETE_ACCOUNT: &str = "Delete account";
/// The two confirmations, asserted by their distinguishing copy (S9 7.5;
/// `Copy.deleteAccountExplain` and `Copy.deleteAccountConfirm`); the modal
/// barrier blocks the row's own `Delete account` while a dialog is open.
const DELETE_EXPLAIN: &str = "Delete your Mail Tinder account?";
const DELETE_CONFIRM: &str = "This can't be undone. Delete your account now?";
const CONTINUE: &str = "Continue";
/// The Sign-in screen's tagline (`Copy.tagline`), which the app shows once the
/// deletion has wiped the session.
const SIGN_IN_TAGLINE: &str = "Swipe through your inbox and clear the mail you don't want.";

/// A route nested under Settings, so the step-up popup's own URL never matches.
const SETTINGS_ROUTE: &str = "/settings";
/// The deletion's own backstop horizon (S2 AU-06 AC1: every server record is
/// swept within 24 hours).
const TWENTY_FIVE_HOURS_S: i64 = 25 * 60 * 60;
/// How long the deletion's round trip (the provider calls, the store deletes and
/// the app's wipe) may take.
const DELETION_TIMEOUT: Duration = Duration::from_secs(60);

/// The symbolic `fake-google` routes the deletion's order is asserted on
/// (`/__fake/events`).
const FOLDER_DELETE_ROUTE: &str = "drive.files.delete";
const REVOKE_ROUTE: &str = "oauth.revoke";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn au_06_ac1_e2e_delete_account_order_and_sweep() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let control = TestControl::connect(&stack)?;
    google.reset().await?;
    google.register_client(&stack).await?;

    let ui: Ui = signed_in_user(&stack, SUB, EMAIL, &[FIXTURE]).await?;
    ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;

    // One keep: the swipe writes the user state file into the mailbox's app
    // folder, so the deletion below has a file to delete (AU-06 AC1 step 1).
    ui.tap(KEEP).await?;
    ui.wait_for_text_absent(SENDER, FEED_TIMEOUT).await?;

    // The user this journey deletes, read while its mailbox document still
    // exists: the `sub` the account was seeded with is the identifier the
    // harness knows (the address is encrypted).
    let emulator = firestore_emulator_url()?;
    let before = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    let user = user_of_the_seeded_mailbox(&before)?;

    // Settings, Account, Delete account, both confirmations (S9 7.5). The
    // deletion needs a fresh sign-in, so the Confirm-it's-you overlay appears
    // and its popup re-authenticates the user (AU-06 AC3).
    ui.tap_merged(SETTINGS_TAB).await?;
    ui.wait_for_text(ACCOUNT, TAP_ACK_TIMEOUT).await?;
    ui.tap(ACCOUNT).await?;
    ui.wait_for_text(DELETE_ACCOUNT, APP_LOAD_TIMEOUT).await?;
    ui.tap(DELETE_ACCOUNT).await?;
    ui.wait_for_text(DELETE_EXPLAIN, TAP_ACK_TIMEOUT).await?;
    ui.tap(CONTINUE).await?;
    ui.wait_for_text(DELETE_CONFIRM, TAP_ACK_TIMEOUT).await?;
    // The step-up's own sign-in is scripted before the popup can open. This
    // handle's own account table is what `select_account_for_next_authorize`
    // reads, so the account is seeded here too (`signed_in_user` seeded the
    // same one on its own handle).
    google.seed_account(SUB, EMAIL, true).await?;
    google.select_account_for_next_authorize(SUB).await?;
    ui.tap(DELETE_ACCOUNT).await?;
    ui.wait_for_text(CONFIRM_ITS_YOU, TAP_ACK_TIMEOUT).await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.wait_for_popup(TAP_ACK_TIMEOUT).await?;
    let origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.wait_for_url(&origin, SETTINGS_ROUTE, REDIRECT_TIMEOUT)
        .await?;
    ui.switch_to_main().await?;

    // The api deleted everything and cleared the session; the app wipes and
    // returns to Sign-in.
    ui.wait_for_text(SIGN_IN_TAGLINE, DELETION_TIMEOUT).await?;

    // AU-06 AC1: the app folder file is deleted before the token is revoked
    // (S10 6.3, row "Account deletion order"). The fake records both calls, in
    // arrival order, on `/__fake/events`.
    let calls = google.calls().await?;
    let folder = call_position(&calls, FOLDER_DELETE_ROUTE)?;
    let revoke = call_position(&calls, REVOKE_ROUTE)?;
    assert!(
        folder < revoke,
        "the app folder file must go before the token: {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.route == FOLDER_DELETE_ROUTE)
            .count(),
        1,
        "exactly one app folder file delete: {calls:?}"
    );
    assert_eq!(
        google.revocations().await?,
        vec!["refresh".to_owned()],
        "the deleted account's refresh token must be revoked once"
    );

    // Past the 24-hour horizon the worker's sweep runs (S2 AU-06 AC1). The
    // request's own deletion already removed the user's records, so the sweep
    // has nothing to finish - which is the point: the check below is over the
    // whole database afterwards.
    control.advance_clock(TWENTY_FIVE_HOURS_S).await?;
    control.sweep().await?;
    // Back to real time: one api process serves every journey of the run.
    control.advance_clock(-TWENTY_FIVE_HOURS_S).await?;

    // AU-06 AC1, DEL-1: no document of any S5 collection names the user, and no
    // job document survives either.
    let after = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    let left = documents_holding(&after, &user);
    assert!(
        left.is_empty(),
        "documents still name the deleted user: {left:?}"
    );
    let jobs: Vec<String> = documents_in(&after, "jobs")
        .into_iter()
        .filter(|(_, json)| json.contains(&user))
        .map(|(path, _)| path.clone())
        .collect();
    assert!(jobs.is_empty(), "jobs survive for the user: {jobs:?}");

    finish_journey(ui).await?;
    Ok(())
}

/// The position of the first provider call on `route`, or an error naming what
/// the fake did record.
fn call_position(calls: &[e2e::FakeCall], route: &str) -> Result<usize, Box<dyn Error>> {
    calls
        .iter()
        .position(|call| call.route == route)
        .ok_or_else(|| format!("fake-google recorded no {route} call: {calls:?}").into())
}

/// The user id of the mailbox this journey linked, read from the mailbox
/// document whose `provider_subject_id` is the `sub` the account was seeded with
/// (the email address is encrypted, so the `sub` is the identifier the harness
/// knows).
fn user_of_the_seeded_mailbox(documents: &[(String, String)]) -> Result<String, Box<dyn Error>> {
    let path = mailbox_path_for_subject(documents, SUB)
        .ok_or_else(|| format!("no mailbox document for {SUB}"))?;
    let document = documents
        .iter()
        .find(|(candidate, _)| candidate == path)
        .map(|(_, json)| json.as_str())
        .ok_or_else(|| format!("mailbox document {path} vanished from the dump"))?;
    document_string(document, "user_id")
        .ok_or_else(|| format!("mailbox document {path} holds no user_id").into())
}
