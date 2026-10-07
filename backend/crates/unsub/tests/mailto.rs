//! Service integration tests for the `mailto:` sender (T-703).
//!
//! Every test drives the real `run_job` runner with a `MailtoSender` over the
//! in-memory fakes, so the whole path (claim, quota, mint, send, terminal
//! state) is exercised. Each test returns `Result`; an assertion failure is the
//! test failing.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::sync::Arc;

use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason, Provider,
    ProviderSubjectId, UserId,
};
use obs::Sensitive;
use ports::store::{JobOutcomeCode, JobRecord, NeedsAttentionRecord, Precondition};
use ports::{
    Ciphertext, Clock, IdError, KeyService, MailProvider, MailboxCtx, MailboxRecord, MessageQuery,
    Ports, Rng, ServerStore, UserRecord,
};
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use svc_common::mint::seal_refresh_token;
use testkit::mailbox::{MAIL_TINDER_LABEL, SYS_SENT};
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::Duration;
use unsub::mailto::MailtoSender;
use unsub::runner::{run_job, Delivery, RunResponse, UnsubState};
use unsub::sender::UnsubSender;
use uuid::Uuid;

const AUDIENCE: &str = "unsub-audience";
const CALLER: &str = "tasks@mailtinder.iam.gserviceaccount.com";
const CANARY_ACCESS: &str = "CANARY-access-token-0001";
const CANARY_REFRESH: &str = "CANARY-refresh-token-0001";
const SENDER_DISPLAY: &str = "Example News";
const TARGET: &str = "mailto:unsub@example.com";

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Tracing caches each callsite's interest process-wide and a scoped capture is
/// seen by one thread only, so the test that reads captured logs holds this.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Env {
    ports: Ports,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
}

fn state(env: &Env) -> UnsubState {
    UnsubState {
        ports: env.ports.clone(),
        senders: vec![Arc::new(MailtoSender) as Arc<dyn UnsubSender>],
        auth: InternalAuthConfig {
            audience: AUDIENCE.to_owned(),
            allowed_caller_email: CALLER.to_owned(),
        },
    }
}

async fn setup() -> Result<Env, Box<dyn std::error::Error>> {
    let (ports, fakes) = fake_ports();
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(&ports, &fakes, &user, MailboxStatus::Connected, true).await?;
    Ok(Env {
        ports,
        fakes,
        user,
        mailbox,
    })
}

/// Let the identity fake mint the canary access token for our refresh token.
/// The fake pops one scripted response per `refresh` call, so a test that mints
/// more than once must script more than one.
fn mint_canary(env: &Env) {
    mint_canary_times(env, 1);
}

fn mint_canary_times(env: &Env, times: usize) {
    for _ in 0..times {
        env.fakes
            .identity
            .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));
    }
}

async fn seed_user(fakes: &Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin: false,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

async fn seed_mailbox(
    ports: &Ports,
    fakes: &Fakes,
    user: &UserId,
    status: MailboxStatus,
    with_refresh: bool,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let mailbox = MailboxId::new(fakes.rng.uuid_v4());
    let mut record = MailboxRecord {
        mailbox_id: mailbox,
        user_id: *user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new("sub-1")?,
        email_address: Ciphertext(vec![7u8; 24]),
        status,
        linked_at: fakes.clock.now(),
        is_primary: true,
        refresh_token: None,
    };
    if with_refresh {
        let user_record = fakes
            .store
            .users()
            .get(user)
            .await?
            .ok_or("missing user")?
            .record;
        record.refresh_token = Some(
            seal_refresh_token(
                ports,
                &user_record,
                &mailbox,
                &Sensitive::new(CANARY_REFRESH.to_owned()),
            )
            .await?,
        );
    }
    fakes
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(mailbox)
}

/// A queued, due `mailto:` job for `mailbox`. The distinct `list_seed` keeps
/// every job in its own batch group.
async fn seed_job(
    env: &Env,
    mailbox: &MailboxId,
    list_seed: u8,
    target: &str,
) -> Result<JobId, Box<dyn std::error::Error>> {
    let job_id = JobId(Uuid::new_v4());
    let sealed = job_record::seal_target(
        &env.ports,
        &env.user,
        &job_id,
        &Sensitive::new(target.to_owned()),
    )
    .await?;
    let display = job_record::seal_sender_display(
        &env.ports,
        &env.user,
        &job_id,
        &Sensitive::new(SENDER_DISPLAY.to_owned()),
    )
    .await?;
    let now = env.fakes.clock.now();
    let record = JobRecord {
        job_id,
        user_id: env.user,
        mailbox_id: *mailbox,
        list_key_hash: ports::ListKeyHash([list_seed; 32]),
        method: JobMethod::Mailto,
        target: Some(sealed),
        sender_display: Some(display),
        due_at: now,
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: now + Duration::hours(1),
    };
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(job_id)
}

async fn job(env: &Env, id: &JobId) -> Result<JobRecord, Box<dyn std::error::Error>> {
    Ok(env
        .ports
        .store
        .jobs()
        .get(id)
        .await?
        .ok_or("missing job")?
        .record)
}

async fn mailbox_record(
    env: &Env,
    id: &MailboxId,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
    Ok(env
        .ports
        .store
        .mailboxes()
        .get(id)
        .await?
        .ok_or("missing mailbox")?
        .record)
}

async fn only_item(env: &Env) -> Result<NeedsAttentionRecord, Box<dyn std::error::Error>> {
    let page = env
        .ports
        .store
        .needs_attention()
        .by_user(
            &env.user,
            ports::PageRequest {
                limit: 10,
                after: None,
            },
        )
        .await?;
    assert_eq!(page.items.len(), 1, "exactly one Needs Attention item");
    Ok(page.items.into_iter().next().ok_or("missing item")?.record)
}

fn mail_ctx(mailbox: MailboxId) -> MailboxCtx {
    MailboxCtx {
        mailbox,
        access_token: Sensitive::new(CANARY_ACCESS.to_owned()),
    }
}

/// A queued job run once. Returns the terminal status.
async fn run_once(
    env: &Env,
    state: &UnsubState,
    mailbox: &MailboxId,
    list_seed: u8,
) -> Result<JobStatus, Box<dyn std::error::Error>> {
    let id = seed_job(env, mailbox, list_seed, TARGET).await?;
    assert_eq!(
        run_job(state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );
    Ok(job(env, &id).await?.status)
}

// ---------------------------------------------------------------------------
// UN-01 AC4 / AC5: a fresh access token, minted and not stored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac4_mailto_token_minted_not_stored() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Sent);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::MailtoSent)
    );
    // The send can only have happened with a freshly minted token.
    assert_eq!(
        env.fakes.mailbox.calls(MailOp::SendMailto),
        1,
        "the send used a fresh token"
    );
    let dump = serde_json::to_string(&env.fakes.store.export_json())?;
    assert!(
        !dump.contains(CANARY_ACCESS),
        "no record holds the access token"
    );
    assert!(
        !dump.contains(CANARY_REFRESH),
        "no record holds the refresh token in the clear"
    );
    Ok(())
}

#[tokio::test]
async fn un_01_ac5_mailto_uses_minted_token() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    // Only the value the identity fake mints can reach the provider. Revoking
    // exactly that value makes the send fail, so the failure proves the minted
    // token was the one presented (UN-01 AC5).
    env.fakes.mailbox.revoke_token(CANARY_ACCESS);
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::TokenInvalid)
    );
    // A provider `Unauthorized` also moves the mailbox to `needs_sign_in`.
    assert_eq!(
        mailbox_record(&env, &env.mailbox).await?.status,
        MailboxStatus::NeedsSignIn
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-01 AC6: a revoked grant ends the job failed with "Sign in again"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac6_mailto_revoked_token_sign_in_item() -> TestResult {
    let env = setup().await?;
    env.fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::TokenInvalid)
    );
    assert_eq!(
        mailbox_record(&env, &env.mailbox).await?.status,
        MailboxStatus::NeedsSignIn
    );
    let item = only_item(&env).await?;
    assert_eq!(item.reason_code, NeedsAttentionReason::SignInRequired);
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-03 AC1: the address, subject and body come from the URI, same mailbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_03_ac1_sends_to_address_subject_body_from_uri() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    let state = state(&env);
    let target = "mailto:unsub@example.com?subject=Remove%20me&body=Please%20remove";
    let id = seed_job(&env, &env.mailbox, 1, target).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let sent = env.fakes.mailbox.sent();
    assert_eq!(sent.len(), 1, "exactly one mailto message was sent");
    assert_eq!(sent[0].mailbox, env.mailbox);
    assert_eq!(sent[0].to, "unsub@example.com");
    assert_eq!(sent[0].subject.as_deref(), Some("Remove me"));
    assert_eq!(sent[0].body.as_deref(), Some("Please remove"));
    Ok(())
}

#[tokio::test]
async fn un_03_ac1_sends_from_same_mailbox() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    let other = seed_mailbox(
        &env.ports,
        &env.fakes,
        &env.user,
        MailboxStatus::Connected,
        true,
    )
    .await?;
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let sent = env.fakes.mailbox.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].mailbox, env.mailbox, "only the job's mailbox sent");
    assert!(
        sent.iter().all(|record| record.mailbox != other),
        "the other mailbox sent nothing"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-03 AC2 / INV-5: left in Sent with the label, never deleted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_03_ac2_sent_message_labelled_mail_tinder() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    let ctx = mail_ctx(env.mailbox);
    let gmail: &dyn MailProvider = env.ports.gmail.as_ref();
    // Create the "Mail Tinder" label first so its ID is known; the adapter (and
    // the fake that mirrors it) files the sent message under it.
    let label_id = gmail.ensure_label(&ctx, MAIL_TINDER_LABEL).await?;
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let page = gmail
        .list_messages(
            &ctx,
            &MessageQuery {
                label: Some(SYS_SENT.to_owned()),
                ..MessageQuery::default()
            },
            None,
            10,
        )
        .await?;
    assert_eq!(page.items.len(), 1, "the sent message is left in Sent");
    let labels = &page.items[0].labels;
    assert!(labels.contains(SYS_SENT));
    assert!(
        labels.contains(&label_id),
        "the sent message carries the Mail Tinder label"
    );
    Ok(())
}

#[tokio::test]
async fn inv_5_mailto_path_never_deletes() -> TestResult {
    let env = setup().await?;
    mint_canary(&env);
    // `MailProvider` has no delete method (INV-5); this proves the mailto path
    // never removes a message either.
    let kept = env.fakes.mailbox.seed(
        &env.mailbox,
        SeedMessage {
            from_display: "List".to_owned(),
            from_address: "list@example.org".to_owned(),
            subject: "newsletter".to_owned(),
            raw_headers: vec![],
            facts: domain::HeaderFacts::default(),
            preview_text: "hello".to_owned(),
            internal_date: env.fakes.clock.now(),
            labels: vec!["INBOX".to_owned()],
        },
    );
    let state = state(&env);
    let id = seed_job(&env, &env.mailbox, 1, TARGET).await?;

    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );

    let ctx = mail_ctx(env.mailbox);
    assert!(
        env.fakes.mailbox.get_meta(&ctx, &kept).await.is_ok(),
        "the seeded message was not deleted"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V2.3.2 / V2.4.1: the shared per-mailbox daily cap
// ---------------------------------------------------------------------------

/// The 101st mailto job of a UTC day is refused; another mailbox is unaffected,
/// and the next UTC day is allowed again.
#[tokio::test]
async fn asvs_v2_3_2_mailto_daily_cap_per_mailbox() -> TestResult {
    let _serial = SERIAL.lock().await;
    let env = setup().await?;
    mint_canary_times(&env, 200);
    let other = seed_mailbox(
        &env.ports,
        &env.fakes,
        &env.user,
        MailboxStatus::Connected,
        true,
    )
    .await?;
    let state = state(&env);
    let clock = obs::arc(obs::FixedClock(env.fakes.clock.now()));
    let (capture, _guard) = obs::capture("unsub", clock);

    for seed in 0u8..100 {
        assert_eq!(
            run_once(&env, &state, &env.mailbox, seed).await?,
            JobStatus::Sent,
            "job {seed} is within the cap"
        );
    }
    let over = seed_job(&env, &env.mailbox, 200, TARGET).await?;
    assert_eq!(
        run_job(&state, &over, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );
    let stored = job(&env, &over).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::Refused)
    );
    assert!(
        capture.text().contains("rate_limit_hit"),
        "a rate_limit_hit security event was written"
    );

    // Another mailbox has its own counter.
    assert_eq!(
        run_once(&env, &state, &other, 201).await?,
        JobStatus::Sent,
        "another mailbox is unaffected"
    );
    assert_eq!(
        env.fakes.mailbox.sent().len(),
        101,
        "the other mailbox really sent"
    );

    // The next UTC day starts a fresh window.
    env.fakes.clock.advance(Duration::days(1));
    assert_eq!(
        run_once(&env, &state, &env.mailbox, 202).await?,
        JobStatus::Sent,
        "the next day is allowed"
    );
    assert_eq!(
        env.fakes.mailbox.sent().len(),
        102,
        "the next day really sent"
    );
    Ok(())
}

/// Two `UnsubState` values over one store share the count, so the cap holds
/// across Cloud Run instances (V2.4.1) rather than per instance.
#[tokio::test]
async fn asvs_v2_4_1_mailto_quota_shared_across_instances() -> TestResult {
    let env = setup().await?;
    mint_canary_times(&env, 200);
    let instance_a = state(&env);
    let instance_b = state(&env);

    for seed in 0u8..60 {
        assert_eq!(
            run_once(&env, &instance_a, &env.mailbox, seed).await?,
            JobStatus::Sent
        );
    }
    for seed in 60u8..100 {
        assert_eq!(
            run_once(&env, &instance_b, &env.mailbox, seed).await?,
            JobStatus::Sent
        );
    }
    assert_eq!(
        env.fakes.mailbox.sent().len(),
        100,
        "every job within the cap sent"
    );
    // The 101st unit is counted by the second instance's state over the shared
    // counter, so it is refused.
    let over = seed_job(&env, &env.mailbox, 201, TARGET).await?;
    assert_eq!(
        run_job(&instance_b, &over, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );
    let stored = job(&env, &over).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::Refused)
    );
    Ok(())
}
