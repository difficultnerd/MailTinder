#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Journey 5 (UN-01 AC1, UN-01 AC3, UN-02 AC1): rejecting a one-click list
//! message queues an unsubscribe that runs exactly once after its due time, the
//! one-click POST is exact, and the next Feed load collects the outcome into
//! History.
//!
//! Run only by `scripts/e2e.sh` (`cargo test -p e2e -- --ignored`), the job that
//! has fake-google, the Firestore emulator, the testbed and ChromeDriver. Run
//! with `--test-threads=1` (as the script does): the tests share the global
//! `fake-google` and clear the one testbed, so parallel execution would corrupt
//! state. Every
//! test seeds the corpus case `one-click-covered` with its `List-Unsubscribe`
//! target rewritten to this run's testbed TLS listener (T-1101g): `unsub`'s e2e
//! egress accepts the literal loopback IP and trusts only that run's CA. Each
//! test owns an account and clears the testbed first, so no test sees another
//! test's mail or traffic (S10 6.2).
//!
//! The journey is one browser flow, but each acceptance criterion gets its own
//! test so its behaviour can be removed and watched failing (S10 10.4): the job
//! runs once (UN-01 AC1), the request is exact (UN-02 AC1), and the outcome
//! reaches History after the next Feed load (UN-01 AC3).

use std::error::Error;
use std::time::{Duration, Instant};

use e2e::{
    finish_journey, note_step, signed_in_user, EventLog, FakeGoogle, LogMark, MetricEvent,
    RecordedRequest, Stack, Testbed, Ui, APP_LOAD_TIMEOUT, FEED_ROUTE, FEED_TIMEOUT, REJECT_BUTTON,
};

/// The corpus case whose headers classify as a covered one-click list message.
const FIXTURE: &str = "one-click-covered";
/// The target the corpus case ships with, replaced for this run.
const FIXTURE_TARGET: &str = "https://u.example.com/weekly";
/// The testbed route the seeded message points at (S10 6.2).
const ROUTE: &str = "/oneclick/200";
/// The sender of [`FIXTURE`]: the text the Feed card shows.
const SENDER: &str = "Acme News";
/// A deterministic `internal_date` per test (S10 1 rule 2: the harness never
/// reads the wall clock for seeded data).
const INTERNAL_DATE: &str = "2026-10-03T09:00:00+00:00";
/// The start of the toast the Feed shows while the unsubscribe is queued (S2
/// SW-03 AC2); the delay it names is the api's, so only these words are
/// asserted.
const QUEUED_TOAST: &str = "Trashed. Unsubscribing in";
/// The exact body a one-click POST carries (UN-02 AC1; S10 6.2).
const ONE_CLICK_BODY: &[u8] = b"List-Unsubscribe=One-Click";
/// The Settings tab, the History entry and the History filter (S9 7, 7.2).
const SETTINGS_TAB: &str = "Settings";
const HISTORY: &str = "History";
const UNSUBSCRIBES: &str = "Unsubscribes";
/// The History row a collected unsubscribe outcome shows (T-307's words).
const HISTORY_ROW: &str = "Unsubscribe: Sent";
/// How long to keep polling the testbed after a job's due time (T-1101g).
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the Feed load a test starts may take to reach the api.
const FEED_RELOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the collected outcome may take to appear in History.
const HISTORY_TIMEOUT: Duration = Duration::from_secs(30);
/// The interval every poll above uses.
const POLL: Duration = Duration::from_millis(200);
/// Slack allowed when checking a job did not run before its due time (the api's
/// timer and this process do not share a clock).
const DUE_TOLERANCE: Duration = Duration::from_secs(1);

/// The api's due delay for a queued unsubscribe (`MT_E2E_UNSUB_DELAY_S`,
/// T-1101g): the test configuration queues a job seconds ahead rather than the
/// production five minutes, so a journey waits for a near-term due time instead
/// of advancing a clock. It must be set (`scripts/e2e.sh` exports it) so this
/// journey waits exactly what the api queued with: a default here could wait
/// seconds against a job due minutes later (T-1101e review F3).
fn due_delay() -> Result<Duration, Box<dyn Error>> {
    let raw = std::env::var("MT_E2E_UNSUB_DELAY_S")
        .map_err(|_| "MT_E2E_UNSUB_DELAY_S must be set for an e2e run")?;
    Ok(Duration::from_secs(raw.trim().parse()?))
}

/// One test's world: the testbed's read-back client and the browser on the Feed.
struct Journey {
    testbed: Testbed,
    ui: Ui,
}

impl Journey {
    /// Seed `sub`/`email` and one covered one-click message pointing at this
    /// run's testbed, clear the testbed, sign in and land on the Feed (S10 3.3).
    async fn start(sub: &str, email: &str) -> Result<Self, Box<dyn Error>> {
        let stack = Stack::from_env()?;
        let testbed_https = std::env::var("MT_E2E_TESTBED_HTTPS_URL")?;
        let google = FakeGoogle::connect(&stack)?;
        let testbed = Testbed::connect(&stack)?;
        google.reset().await?;
        google.register_client(&stack).await?;
        google.seed_account(sub, email, true).await?;
        google
            .seed_one_click_message(
                email,
                FIXTURE,
                FIXTURE_TARGET,
                ROUTE,
                &testbed_https,
                INTERNAL_DATE,
            )
            .await?;
        testbed.reset().await?;
        let ui = signed_in_user(&stack, sub, email, &[]).await?;
        ui.wait_for_text(SENDER, FEED_TIMEOUT).await?;
        Ok(Self { testbed, ui })
    }

    /// Reject the one-click card and wait for the queued toast (SW-03 AC2).
    async fn reject(&self) -> Result<(), Box<dyn Error>> {
        self.ui.tap(REJECT_BUTTON).await?;
        self.ui.wait_for_text(QUEUED_TOAST, FEED_TIMEOUT).await?;
        Ok(())
    }

    /// Wait out the job's due time and return what the testbed recorded on the
    /// one-click route: one POST for a job that ran, nothing for one cancelled.
    /// The due delay is read here from the run's configuration rather than
    /// carried on the journey, so it stays a plain config value.
    async fn wait_for_due(&self, extra: Duration) -> Result<Vec<RecordedRequest>, Box<dyn Error>> {
        Ok(self
            .testbed
            .wait_for_request(ROUTE, due_delay()? + extra)
            .await?)
    }
}

/// True once the api has logged a Feed load since `mark` (T-307's `request`
/// event, route `/api/v1/feed/next`). A Feed load is what collects a stored
/// unsubscribe outcome into History (S10 6.3, option B), so the journey waits
/// for it - and thus proves it - before opening History.
async fn feed_loaded(events: &EventLog, mark: &LogMark) -> Result<bool, Box<dyn Error>> {
    let deadline = Instant::now() + FEED_RELOAD_TIMEOUT;
    loop {
        let loaded = events
            .request_routes_since_mark(mark)?
            .iter()
            .any(|route| route == FEED_ROUTE);
        if loaded || Instant::now() >= deadline {
            return Ok(loaded);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The `unsub_outcome` metric events written since `mark`, once at least one
/// exists (the unsub service logs after its POST completes).
async fn unsub_outcomes(
    events: &EventLog,
    mark: &LogMark,
) -> Result<Vec<MetricEvent>, Box<dyn Error>> {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    loop {
        let outcomes: Vec<MetricEvent> = events
            .since_mark(mark.clone())?
            .into_iter()
            .filter(|event| event.event_type == "unsub_outcome")
            .collect();
        if !outcomes.is_empty() || Instant::now() >= deadline {
            return Ok(outcomes);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// A job that ran must not have run *before* its due time (UN-01 AC1): the
/// first record cannot arrive sooner than `due` after the reject, less
/// [`DUE_TOLERANCE`] for the two clocks' skew. Without this a delay override
/// that was ignored - a job that fired immediately - would still pass.
fn assert_not_before_due(due: Duration, elapsed: Duration) {
    assert!(
        elapsed + DUE_TOLERANCE >= due,
        "the unsubscribe ran before its due time: {elapsed:?} after the reject, due after {due:?}"
    );
}

/// The S3 job outcome code a successful one-click run records (T-701): the
/// `unsub_outcome` metric (and the `unsub_job_outcome` security event) carries
/// this code, while History shows the job's terminal `Sent` status as "Sent"
/// (T-609). The two are different vocabularies: the code, not the display
/// word, is what the service emits.
const ONE_CLICK_ACCEPTED: &str = "one_click_accepted";

/// UN-01 AC1: the queued unsubscribe runs exactly once, after its due time.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac1_e2e_unsubscribe_runs_once() -> Result<(), Box<dyn Error>> {
    let journey = Journey::start("sub-un-01-ac1-e2e", "un-01-ac1-e2e@example.com").await?;
    let due = due_delay()?;
    let events = EventLog::new();
    let mark = events.mark();

    // Time from the reject: the job is due `due` after the api plans it, so the
    // first record cannot arrive before that has elapsed.
    let rejected_at = Instant::now();
    journey.reject().await?;

    // The due time (T-1101g): the job becomes due and the testbed sees it.
    let records = journey.wait_for_due(DELIVERY_TIMEOUT).await?;
    assert_eq!(
        records.len(),
        1,
        "exactly one POST must reach the testbed after the due time"
    );
    assert_not_before_due(due, rejected_at.elapsed());

    // Runs once: a redelivery would be a second record. Watch the count past
    // another due interval instead of sleeping through it.
    let deadline = Instant::now() + due + DELIVERY_TIMEOUT;
    while Instant::now() < deadline {
        assert_eq!(
            journey.testbed.received(ROUTE).await?.len(),
            1,
            "the queued unsubscribe runs exactly once"
        );
        tokio::time::sleep(POLL).await;
    }

    // S10 8: the unsub service writes one `unsub_outcome` metric per run. A
    // one-click POST that the sender accepted is T-701's `one_click_accepted`
    // code (not the History display word "Sent").
    let outcomes = unsub_outcomes(&events, &mark).await?;
    assert_eq!(
        outcomes.len(),
        1,
        "the unsubscribe must emit exactly one outcome: {outcomes:?}"
    );
    assert_eq!(
        outcomes[0].outcome.as_deref(),
        Some(ONE_CLICK_ACCEPTED),
        "the run's outcome is not `{ONE_CLICK_ACCEPTED}`"
    );

    // S10 8: the reject that queued the job emits exactly one `swipe`, carrying
    // only the action (T-1114).
    let swipes: Vec<MetricEvent> = events
        .since_mark(mark.clone())?
        .into_iter()
        .filter(|event| event.event_type == "swipe")
        .collect();
    assert_eq!(
        swipes.len(),
        1,
        "the reject must emit exactly one swipe: {swipes:?}"
    );
    assert_eq!(
        swipes[0].outcome.as_deref(),
        Some("reject"),
        "the swipe carries the rejected action"
    );

    finish_journey(journey.ui).await?;
    Ok(())
}

/// UN-02 AC1: the one-click POST is exact - the fixed body, no cookies and no
/// credentials (S10 6.2).
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_02_ac1_e2e_one_click_post_is_exact() -> Result<(), Box<dyn Error>> {
    let journey = Journey::start("sub-un-02-ac1-e2e", "un-02-ac1-e2e@example.com").await?;

    journey.reject().await?;
    let records = journey.wait_for_due(DELIVERY_TIMEOUT).await?;
    assert_eq!(
        records.len(),
        1,
        "exactly one POST must reach the testbed after the due time"
    );

    let record = &records[0];
    assert_eq!(
        record.method, "POST",
        "the one-click request must be a POST"
    );
    assert_eq!(
        record.body, ONE_CLICK_BODY,
        "the one-click body is not the fixed one"
    );
    let header = |name: &str| {
        record
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    assert!(
        header("cookie").is_none(),
        "the one-click POST carries a Cookie header"
    );
    assert!(
        header("authorization").is_none(),
        "the one-click POST carries an Authorization header"
    );

    finish_journey(journey.ui).await?;
    Ok(())
}

/// UN-01 AC3: the outcome appears in History after the next Feed load.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh"]
async fn un_01_ac3_e2e_outcome_in_history_after_feed_load() -> Result<(), Box<dyn Error>> {
    let journey = Journey::start("sub-un-01-ac3-e2e", "un-01-ac3-e2e@example.com").await?;

    // Mark before the reject: the runner writes the `unsub_outcome` metric as
    // soon as the POST and the store write finish, which can be before the due
    // wait below returns, so a mark taken after it would miss the line and read
    // the log from too far along (T-1101e review F1).
    let events = EventLog::new();
    let mark = events.mark();

    journey.reject().await?;
    let records = journey.wait_for_due(DELIVERY_TIMEOUT).await?;
    assert_eq!(
        records.len(),
        1,
        "the outcome can only be collected once the job has run"
    );

    // The job's outcome is what History shows, and the runner stores it (and
    // logs its metric) once the POST is done: wait for that before loading the
    // Feed, so the collection cannot race the store.
    let outcomes = unsub_outcomes(&events, &mark).await?;
    assert_eq!(
        outcomes.len(),
        1,
        "the job must finish before its outcome can be collected: {outcomes:?}"
    );

    // The next Feed load collects every stored outcome (S10 6.3, option B), and
    // the app's way to load the Feed is pull to refresh. The api's request log
    // says whether the gesture produced one, so this is checked, not assumed;
    // which path ran is recorded for the run's diagnostics.
    let pull_mark = events.mark();
    let pulled =
        journey.ui.pull_to_refresh().await.is_ok() && feed_loaded(&events, &pull_mark).await?;
    if pulled {
        note_step("un_01_ac3: the Feed loaded after a pull to refresh");
    } else {
        // A runner whose ChromeDriver cannot drive a touch drag, or whose
        // browser does not treat it as one, is no reason to lose the criterion:
        // a page reload boots the app and opens the Feed. Either way the Feed
        // load is proven before History is opened.
        note_step("un_01_ac3: no Feed load from a pull to refresh; reloading the page");
        let reload_mark = events.mark();
        journey.ui.reload().await?;
        journey
            .ui
            .wait_for_text(SETTINGS_TAB, APP_LOAD_TIMEOUT)
            .await?;
        if !feed_loaded(&events, &reload_mark).await? {
            return Err("the Feed did not load after the pull or a reload".into());
        }
    }

    // Settings, then History, then the Unsubscribes filter (S9 7.2). A bottom
    // navigation destination merges its icon's and its text label, so its
    // Semantics label is the label twice over: `tap_merged` matches a part.
    journey.ui.tap_merged(SETTINGS_TAB).await?;
    journey.ui.tap_merged(HISTORY).await?;
    journey.ui.tap_merged(UNSUBSCRIBES).await?;
    journey
        .ui
        .wait_for_text(HISTORY_ROW, HISTORY_TIMEOUT)
        .await?;

    finish_journey(journey.ui).await?;
    Ok(())
}
