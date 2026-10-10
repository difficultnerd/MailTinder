#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 7 (NA-01 AC2): in a real browser the user triages an https-only list
//! message, which v1 can only hand over (rejecting it raises a Needs Attention
//! item), opens the Needs Attention tab, marks the item done and the item is
//! deleted.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the Firestore emulator, the testbed and ChromeDriver. Run
//! with `--test-threads=1` (as the script does): the tests share the global
//! `fake-google` and clear the one testbed, so parallel execution would corrupt
//! state.
//!
//! The seeded message is the corpus case `https-only-covered`, unmodified: its
//! `List-Unsubscribe` is an https link with no one-click, so the api raises the
//! item and never fetches the link (v1 has no page handler). The testbed is the
//! proof: it is cleared first and must have recorded nothing at the end.

use std::error::Error;

use e2e::{
    document_string, documents_in, finish_journey, firestore_emulator_url,
    mailbox_path_for_subject, signed_in_user, FakeGoogle, FirestoreDump, Stack, Testbed, Ui,
    APP_LOAD_TIMEOUT, E2E_FIRESTORE_PROJECT, FEED_TIMEOUT, REJECT_BUTTON,
};

/// The account this journey signs in as; unique so it never sees another
/// journey's mail. Reserved domains only.
const SUB: &str = "sub-na-01-ac2-e2e";
const EMAIL: &str = "na-01-ac2-e2e@example.com";
/// The corpus case: an https-only list message whose DKIM covers
/// `List-Unsubscribe` and which has no one-click (`needs_attention_https`).
const FIXTURE: &str = "https-only-covered";
/// The sender of [`FIXTURE`]: the text the Feed card and the item show.
const SENDER: &str = "Acme Pages";
/// The toast a reject of an https-only list message shows (SW-03 AC2;
/// `Copy.trashedUnsubscribeManual`).
const TRASHED_TOAST: &str = "Trashed. The unsubscribe link is in Needs Attention.";
/// The item's reason copy (S9 section 6; `Copy.naHttpsOnlyUnsubscribe`).
const REASON: &str = "This sender needs you to unsubscribe on their website.";
/// The Needs Attention bottom tab and the item's Done action (S9 section 6).
const NEEDS_ATTENTION_TAB: &str = "Needs Attention";
const DONE: &str = "Done";
/// The empty tab's copy (S9 section 6; `Copy.nothingNeedsYou`).
const NOTHING_NEEDS_YOU: &str = "Nothing needs you.";

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn na_01_ac2_e2e_done_deletes_item() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let google = FakeGoogle::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;
    // One reset and one client registration for the run; the account and its
    // message are seeded before sign-in, and the testbed is cleared so an
    // earlier journey's traffic can never pass for this one (S10 6.2).
    google.reset().await?;
    google.register_client(&stack).await?;
    testbed.reset().await?;

    let ui: Ui = signed_in_user(&stack, SUB, EMAIL, &[FIXTURE]).await?;
    ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;

    // Reject: the api trashes the message and raises the item (NA-01 AC1: v1
    // cannot unsubscribe from an https-only list itself).
    ui.tap(REJECT_BUTTON).await?;
    ui.wait_for_text(TRASHED_TOAST, FEED_TIMEOUT).await?;

    // The tab's list is loaded when the shell builds and then only every
    // `refreshEvery` (S9 section 6 `[DEFAULT]`, minutes away), so the tab would
    // still show the list it read before the reject. A fresh load at the app
    // root rebuilds the shell and reads it again - the same way the other
    // journeys re-read the Feed after a state change.
    let origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.goto(&format!("{origin}/")).await?;
    ui.wait_for_text(NEEDS_ATTENTION_TAB, APP_LOAD_TIMEOUT)
        .await?;

    // The tab lists the item with its sender and its reason (S9 section 6). A
    // bottom navigation destination merges its icon's and its text label, so its
    // Semantics label is the label twice over: `tap_merged` matches a part.
    ui.tap_merged(NEEDS_ATTENTION_TAB).await?;
    ui.wait_for_text(REASON, FEED_TIMEOUT).await?;
    ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;

    // The item exists in the emulator before Done, so the check below cannot
    // pass vacuously, and its user is the one Done must leave nothing behind
    // for.
    let emulator = firestore_emulator_url()?;
    let before = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    let user = user_of_the_seeded_mailbox(&before)?;
    let open = items_for(&before, &user);
    assert_eq!(
        open.len(),
        1,
        "the reject must raise exactly one Needs Attention item: {open:?}"
    );

    // Done resolves the item (NA-01 AC2).
    ui.tap(DONE).await?;
    ui.wait_for_text_absent(REASON, FEED_TIMEOUT).await?;
    ui.wait_for_text(NOTHING_NEEDS_YOU, FEED_TIMEOUT).await?;

    // Nothing of that user's remains in the collection, and the item was the
    // only document this journey could have deleted.
    let after = FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?;
    let remaining = items_for(&after, &user);
    assert!(
        remaining.is_empty(),
        "marking an item done must delete it: {remaining:?}"
    );

    // v1 never fetches an https-only link, so the sender's side saw nothing at
    // all (T-1101c: the testbed is the record of what left the stack).
    let recorded = testbed.received_any().await?;
    assert!(
        recorded.is_empty(),
        "an https-only link must never be fetched: the testbed saw {recorded:?}"
    );

    finish_journey(ui).await?;
    Ok(())
}

/// The Needs Attention documents that belong to `user`.
fn items_for(documents: &[(String, String)], user: &str) -> Vec<String> {
    documents_in(documents, "needs_attention")
        .into_iter()
        .filter(|(_, json)| json.contains(user))
        .map(|(path, _)| path.clone())
        .collect()
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
