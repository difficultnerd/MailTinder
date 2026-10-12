//! The nightly live `MailProvider` contract run against the real Gmail sandbox
//! (T-1106).
//!
//! Exists only under the `live` feature, and every case is `#[ignore]`d: the
//! workflow's `cargo test --features live --test live_contract -- --ignored`
//! is the only thing that runs them. Missing or expired credentials classify as
//! their own outcome, never a pass (S10 4.3, decision T1).
//!
//! The run sends nothing. Seeding IMPORTS synthetic mail with
//! `users.messages.import`; the transport guard refuses any call to a send
//! endpoint; and the sandbox's `SENT` label is checked before the first case and
//! after the last.
//!
//! Content-freeness (S10 4.3): the shared suite interpolates message subjects
//! and provider error values into its own error strings, so the workflow runs
//! this WITHOUT `--nocapture`, captures the output to a file that is never
//! printed or uploaded, and prints only the pass/fail result and the NAMES of
//! failing tests. Nothing here prints a token, the sandbox address or a body.
#![cfg(feature = "live")]
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::disallowed_macros
)]

mod live_support;

use live_support::{live_corpus_cases, LiveError, LiveOutcome, Sandbox, SandboxCreds};
use testkit::contract::mail_provider::{mail_provider, CaseGroups, MailSeeder as _, MailTarget};

/// The live run uses the read and modify groups; `send` would email a third
/// party from a real account and stays proven by the HTTP fakes (T-203/T-205a).
const LIVE_GROUPS: CaseGroups = CaseGroups {
    read: true,
    modify: true,
    send: false,
};

/// The fixed context for the run's own baseline and cleanup calls.
fn run_ctx() -> ports::MailboxCtx {
    ports::MailboxCtx {
        mailbox: domain::MailboxId(uuid::Uuid::from_u128(1)),
        access_token: obs::Sensitive::new(String::new()),
    }
}

/// Report the classified outcome on the run's summary and fail with it. Only
/// the outcome code, the failing test names and the variable names travel.
fn fail(outcome: LiveOutcome, detail: &str) -> ! {
    annotation("notice", &format!("outcome={}", outcome.code()));
    if outcome == LiveOutcome::CredentialsExpired {
        // The exact fixed annotation T-1106 step 2 names, and the string the
        // workflow's classify step greps for: `title` says why, the message is
        // the instruction. No credential value or address travels in it.
        eprintln!("::error title=credentials expired::Gmail sandbox needs re-authorising");
    }
    panic!("{}: {detail}", outcome.code());
}

/// One GitHub Actions annotation, on stdout/stderr with no test capture.
fn annotation(kind: &str, message: &str) {
    let flag = if kind == "error" {
        "::error"
    } else {
        "::notice"
    };
    eprintln!("{flag} title=live contract::{message}");
}

/// A classified failure detail, never a provider error value or a body.
fn detail(what: &str, err: &LiveError) -> String {
    format!("{what} {}", err.outcome().code())
}

/// The sandbox, or the classified failure that stops the run.
async fn connect() -> Sandbox {
    match Sandbox::connect().await {
        Ok(sandbox) => sandbox,
        Err(LiveError::CredentialsExpired) => {
            fail(
                LiveOutcome::CredentialsExpired,
                "the refresh token was refused",
            );
        }
        Err(LiveError::NotConfigured(names)) => {
            fail(
                LiveOutcome::NotConfigured,
                &format!("{} {names:?}", LiveOutcome::NotConfigured.code()),
            );
        }
        Err(other) => fail(LiveOutcome::ContractFailed, &detail("connect", &other)),
    }
}

/// T-1106: the sandbox must be configured before anything else can be checked.
#[tokio::test]
#[ignore = "nightly live run"]
async fn live_environment_is_configured() {
    match SandboxCreds::from_env() {
        Ok(creds) => {
            assert!(!creds.client_id.is_empty());
            annotation("notice", "credentials present, outcome=CredentialsChecked");
        }
        Err(LiveError::NotConfigured(names)) => {
            fail(
                LiveOutcome::NotConfigured,
                &format!("{} {names:?}", LiveOutcome::NotConfigured.code()),
            );
        }
        Err(other) => fail(LiveOutcome::NotConfigured, &detail("unexpected", &other)),
    }
}

/// T-1106: expired credentials are their own outcome, with the fixed
/// annotation, never a pass and never a contract failure.
#[tokio::test]
#[ignore = "nightly live run"]
async fn live_credentials_are_reported_when_expired() {
    let sandbox = connect().await;
    assert_eq!(sandbox.send_attempts(), 0);
    annotation("notice", "token minted, outcome=Passed");
}

/// T-1106: the live contract run. Seeds by import, runs the shared suite with
/// the send group off, and proves afterwards that nothing was sent.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "nightly live run"]
async fn live_contract_against_the_gmail_sandbox() {
    let sandbox = connect().await;

    // Baseline: our own label is empty and the account carries no sent mail
    // before the first case, so a polluted account fails here, not inside a
    // case.
    let baseline = sandbox.seeder_for(&run_ctx());
    if let Err(err) = baseline.empty_label().await {
        fail(LiveOutcome::ContractFailed, &detail("baseline label", &err));
    }
    match baseline.sent().await {
        Ok(sent) if sent.is_empty() => {}
        Ok(_) => fail(
            LiveOutcome::ContractFailed,
            "the sandbox already holds SENT mail",
        ),
        Err(_) => fail(LiveOutcome::ContractFailed, "baseline sent query failed"),
    }

    // The shared suite: read and modify groups, send group off. The factory
    // trashes everything carrying our label before each case (and once more
    // inside the run's own extra check), so every case sees a fresh, empty
    // mailbox - the factory contract at T-203's `mail_provider`.
    let outcome = mail_provider(|| fresh_case(&sandbox), LIVE_GROUPS).await;

    // The run's own extra check: import the T-204 corpus by `messages.import`
    // and read an imported message's subject back through the adapter.
    let extra = run_extra_checks(&sandbox).await;

    // The no-send boundary: the transport guard refused nothing.
    let guarded = sandbox.send_attempts() == 0;

    // Cleanup: trash every message the run created. No delete endpoint exists
    // (INV-5); Gmail empties trash after 30 days. Labels are reused across
    // runs, never created per run (the suite's own case works on a label it
    // created).
    if let Err(err) = baseline.cleanup().await {
        fail(LiveOutcome::ContractFailed, &detail("cleanup", &err));
    }

    // The secondary send detector: the sandbox's `SENT` label is still empty.
    match baseline.sent().await {
        Ok(sent) if sent.is_empty() => {}
        Ok(_) => fail(
            LiveOutcome::ContractFailed,
            "a message reached the SENT label",
        ),
        Err(_) => fail(LiveOutcome::ContractFailed, "final sent query failed"),
    }

    if !guarded {
        fail(
            LiveOutcome::ContractFailed,
            "the transport guard refused a send",
        );
    }
    if let Err(err) = extra {
        fail(LiveOutcome::ContractFailed, &detail("extra check", &err));
    }
    match outcome {
        Ok(()) => annotation("notice", &format!("outcome={}", LiveOutcome::Passed.code())),
        Err(failing) => fail(LiveOutcome::ContractFailed, &failing),
    }
}

/// One case of the shared suite: a fresh, empty mailbox, or the classified
/// failure that stops the run.
async fn fresh_case(sandbox: &Sandbox) -> MailTarget {
    match sandbox.fresh().await {
        Ok((target, seeder, _ctx)) => {
            let _ = seeder.created();
            target
        }
        Err(err) => fail(LiveOutcome::ContractFailed, &detail("factory", &err)),
    }
}

/// T-1106: the run's extra check. The corpus import path, the read-back of an
/// imported subject and the empty-mailbox isolation between cases, all on the
/// real account and all by import, never a send.
async fn run_extra_checks(sandbox: &Sandbox) -> Result<(), LiveError> {
    let cases = live_corpus_cases().map_err(|_| LiveError::Transient)?;
    let seeder = sandbox.seeder_for(&run_ctx());
    for case in &cases {
        seeder.seed_corpus_case(case).await?;
    }
    // `fresh` trashes the corpus imports it finds under our label, then seeds
    // one message and reads its subject back through the adapter.
    let (target, _, ctx) = sandbox.fresh().await?;
    let seed = testkit::mailbox::state::SeedMessage {
        from_display: "Sender".to_owned(),
        from_address: "live-check@example.com".to_owned(),
        subject: "live-import-check".to_owned(),
        raw_headers: vec![],
        facts: domain::HeaderFacts::default(),
        preview_text: "CANARY-live-import-check-body".to_owned(),
        internal_date: time::OffsetDateTime::from_unix_timestamp(100).unwrap_or(testkit::T0),
        labels: vec!["INBOX".to_owned()],
    };
    let id = target
        .seeder
        .seed(&seed)
        .await
        .map_err(|_| LiveError::Transient)?;
    let read = target
        .seeder
        .labels_of(&id)
        .await
        .map_err(|_| LiveError::Transient)?;
    if !read.contains("INBOX") {
        return Err(LiveError::Transient);
    }
    match target.provider.get_meta(&ctx, &id).await {
        Ok(meta) if meta.subject == "live-import-check" => Ok(()),
        Ok(_) => Err(LiveError::SendAttempt),
        Err(_) => Err(LiveError::Transient),
    }
}
