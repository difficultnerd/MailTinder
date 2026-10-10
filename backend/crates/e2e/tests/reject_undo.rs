#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 4 (SW-05 AC2): in a real browser the user rejects a one-click list
//! message and undoes it before the job's due time, so the unsubscribe is
//! cancelled and no request ever leaves for the sender.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the Firestore emulator, the testbed and ChromeDriver. The
//! seeded message is the corpus case `one-click-covered` with its
//! `List-Unsubscribe` target rewritten to this run's testbed TLS listener
//! (T-1101g): `unsub`'s e2e egress accepts the literal loopback IP and trusts
//! only that run's CA, so a request that did leave would be recorded and caught.
//!
//! The due time is the api's e2e due delay (`MT_E2E_UNSUB_DELAY_S`, T-1101g):
//! the reject queues a job seconds ahead instead of the production five
//! minutes, so the journey waits for a near-term due time rather than advancing
//! a clock. The undo lands in the first second, well inside that window.

use std::error::Error;
use std::time::Duration;

use e2e::{
    finish_journey, signed_in_user, EventLog, FakeGoogle, Stack, Testbed, Ui, FEED_TIMEOUT,
    REJECT_BUTTON,
};

/// The account this journey signs in as; unique so it never sees another
/// journey's mail. Reserved domains only.
const SUB: &str = "sub-reject-undo-e2e";
const EMAIL: &str = "reject-undo-e2e@example.com";
/// The corpus case whose headers classify as a covered one-click list message.
const FIXTURE: &str = "one-click-covered";
/// The target the corpus case ships with, replaced for this run.
const FIXTURE_TARGET: &str = "https://u.example.com/weekly";
/// The testbed route the seeded message points at (S10 6.2).
const ROUTE: &str = "/oneclick/200";
/// The sender of [`FIXTURE`]: the text the Feed card shows, so its return proves
/// the card came back.
const SENDER: &str = "Acme News";
/// A deterministic `internal_date` (S10 1 rule 2: the harness never reads the
/// wall clock for seeded data).
const INTERNAL_DATE: &str = "2026-10-02T09:00:00+00:00";
/// The start of the toast the Feed shows while the unsubscribe is queued (S2
/// SW-03 AC2). The delay it names is the api's, so only these words are
/// asserted: a covered one-click reject reads "Trashed. Unsubscribing in ...".
const QUEUED_TOAST: &str = "Trashed. Unsubscribing in";
/// The Undo control's Semantics label: the toast's action and the Feed's Undo
/// button are the same call.
const UNDO: &str = "Undo";
/// The label a seeded message carries (S10 3.3).
const INBOX: &str = "INBOX";
/// How long to keep polling the testbed past the job's due time (T-1101g).
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// The api's due delay for a queued unsubscribe (`MT_E2E_UNSUB_DELAY_S`,
/// T-1101g): the test configuration queues a job seconds ahead rather than the
/// production five minutes.
fn due_delay() -> Result<Duration, Box<dyn Error>> {
    let raw = std::env::var("MT_E2E_UNSUB_DELAY_S").unwrap_or_else(|_| "3".to_owned());
    Ok(Duration::from_secs(raw.parse()?))
}

#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn sw_05_ac2_e2e_undo_before_due_sends_nothing() -> Result<(), Box<dyn Error>> {
    let stack = Stack::from_env()?;
    let testbed_https = std::env::var("MT_E2E_TESTBED_HTTPS_URL")?;
    let due = due_delay()?;
    let google = FakeGoogle::connect(&stack)?;
    let testbed = Testbed::connect(&stack)?;
    // One reset and one client registration for the run; the account and its
    // one-click message are seeded before sign-in, and the testbed is cleared so
    // an earlier journey's traffic can never pass for this journey's.
    google.reset().await?;
    google.register_client(&stack).await?;
    google.seed_account(SUB, EMAIL, true).await?;
    google
        .seed_one_click_message(
            EMAIL,
            FIXTURE,
            FIXTURE_TARGET,
            ROUTE,
            &testbed_https,
            INTERNAL_DATE,
        )
        .await?;
    testbed.reset().await?;

    // Read only the metric events this journey writes.
    let events = EventLog::new();
    let mark = events.mark();

    let ui: Ui = signed_in_user(&stack, SUB, EMAIL, &[]).await?;
    ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;

    // Reject: the api trashes the message and queues the unsubscribe (S2 SW-03
    // AC2), and the Feed says so.
    ui.tap(REJECT_BUTTON).await?;
    ui.wait_for_text(QUEUED_TOAST, FEED_TIMEOUT).await?;

    // Undo before the due time: the card comes back (SW-05 AC2).
    ui.tap(UNDO).await?;
    ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;

    // Past the due time a cancelled job must have sent nothing at all: the poll
    // waits out the due time and stops early only for a request, which is the
    // failure it looks for.
    let records = testbed
        .wait_for_request(ROUTE, due + DELIVERY_TIMEOUT)
        .await?;
    assert!(
        records.is_empty(),
        "undo must cancel the job: the testbed saw {records:?}"
    );

    // Provider read-back: the message is back in INBOX with its exact seeded
    // label set (S3: a reversal restores the previous labels).
    let messages = google.message_labels(EMAIL).await?;
    assert_eq!(
        messages.len(),
        1,
        "the mailbox should hold exactly the seeded message"
    );
    let mut labels = messages[0].clone();
    labels.sort();
    assert_eq!(
        labels,
        vec![INBOX.to_owned()],
        "the undone message's label set is not the seeded one"
    );

    // S10 8: a cancelled job logs no outcome. The log is read only after the due
    // time and the zero-request check, so this cannot pass vacuously.
    let outcomes: Vec<_> = events
        .since_mark(mark)?
        .into_iter()
        .filter(|event| event.event_type == "unsub_outcome")
        .collect();
    assert!(
        outcomes.is_empty(),
        "a cancelled unsubscribe must log no outcome: {outcomes:?}"
    );

    finish_journey(ui).await?;
    Ok(())
}
