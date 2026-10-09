//! T-1112c: the reject -> unsubscribe proof.
//!
//! Service integration, everything in process on free loopback ports: the api
//! is a real `AppState` over the testkit fakes, `unsub` runs its *e2e* router
//! (`unsub::startup_e2e`), and the target is the real `unsub-testbed`. The api
//! drives the `LocalJobRunner` wired by `api::startup_e2e::e2e_local_runner`
//! (the T-1112c change): a due job is POSTed to `unsub`, which one-clicks the
//! message's `List-Unsubscribe` URL and the testbed records it.
//!
//! The shared `FakeMailbox`, `InMemoryServerStore`, key service and virtual
//! clock stand in for the fakes `scripts/e2e.sh` runs as child processes, so
//! the proof needs no emulator and no `fake-google`.
#![cfg(feature = "testkit")]
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines
)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use api::local_runner::DeliveryState;
use api::routes::feed::ClassificationPayload;
use api::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use api::sealed::{SealedTokens, TokenType};
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use async_trait::async_trait;
use domain::{
    derive_swipe_ids, Classification, EmailAddress, HeaderFacts, JobId, JobStatus, MailboxId,
    MailboxStatus, MessageClass, MessageId, Provider, ProviderSubjectId, SwipeOutcome,
    UnsubscribeOptions, UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, EgressError, EgressRequest, EgressResponse, HttpEgress, KeyService,
    MailboxRecord, OneClickOutcome, Precondition, Rng, Secrets, ServerStore, SessionHash,
    SessionRecordId, UserRecord,
};
use testkit::{fake_ports, Fakes, InMemoryServerStore, SeedMessage};
use time::Duration;
use url::Url;
use uuid::Uuid;

/// The one-click route the seeded message points at (the testbed's `200`).
const ONE_CLICK_PATH: &str = "/oneclick/200";
/// The e2e caller token the runner presents and `unsub` accepts (>= 16 bytes).
const TOKEN: &str = "test-e2e-caller-token-0123456789";

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A started stack: the api `AppState`, the fakes it and the test share, and
/// the running unsub server, testbed and runner.
struct Env {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
    message: MessageId,
    runner: Arc<api::local_runner::LocalJobRunner>,
    testbed: unsub_testbed::Testbed,
    _unsub_task: tokio::task::JoinHandle<()>,
    _runner_task: tokio::task::JoinHandle<()>,
}

impl Env {
    /// An authenticated session handle for `n` (only the record id varies).
    fn session(&self, n: u128) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    /// The job id a reject under `key` derives.
    fn job_id(&self, key: u128) -> JobId {
        derive_swipe_ids(&self.user, Uuid::from_u128(key)).job_id
    }

    /// Seal a classification token naming the seeded message.
    async fn classification_token(
        &self,
        session: &AuthedSession,
        message_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let payload = ClassificationPayload {
            mailbox_id: self.mailbox.0,
            message_id: message_id.to_owned(),
            header_rules: Classification {
                class: MessageClass::Notice,
                bulk_score: 10,
                bulk_reason: "fixed".to_owned(),
                confidence: None,
                probabilities: None,
            },
            classifier_id: HEADER_RULES_ID.to_owned(),
            issued_at: self.fakes.clock.now(),
            bakeoff: None,
        };
        let wrapped = self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("user record")?
            .record
            .wrapped_data_key;
        let sealer = SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        );
        Ok(sealer
            .seal(
                TokenType::Classification,
                &self.user,
                &wrapped,
                &session.session_record_id,
                self.fakes.clock.now() + Duration::hours(1),
                &payload,
            )
            .await?)
    }

    /// Reject the seeded one-click message under idempotency key `key`.
    async fn reject(
        &self,
        key: u128,
    ) -> Result<SwipeResultDto, Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session(1);
        let token = self
            .classification_token(&session, self.message.as_str())
            .await?;
        let req = SwipeRequest {
            mailbox_id: self.mailbox.0,
            message_id: self.message.as_str().to_owned(),
            action: ActionDto::Reject,
            category_id: None,
            new_category_name: None,
            classification_token: token,
        };
        Ok(api::services::swipe::swipe(&self.app, &session, Uuid::from_u128(key), req).await?)
    }
}

/// The one-click message facts the seeded message carries.
fn one_click_facts(unsubscribe: Url) -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(unsubscribe),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some("news.example.com".to_owned()),
        feedback_id: None,
        precedence_bulk: false,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    }
}

/// Seed the user, mailbox, refresh token and one-click message, then build the
/// api `AppState`, the running `unsub` e2e server, the testbed and the runner.
async fn setup() -> Result<Env, Box<dyn std::error::Error + Send + Sync>> {
    let testbed = unsub_testbed::start().await?;
    let one_click = testbed.https_url(ONE_CLICK_PATH);

    let (mut ports, fakes) = fake_ports();

    // --- unsub e2e state, sharing the store, keys, secrets and clock ---
    let unsub_lookup = {
        let testbed_base = format!("http://{}", testbed.addr);
        move |name: &str| match name {
            "FAKE_GOOGLE_URL" => Some("http://127.0.0.1:1".to_owned()),
            "UNSUB_TESTBED_URL" => Some(testbed_base.clone()),
            "UNSUB_E2E_CALLER_TOKEN" => Some(TOKEN.to_owned()),
            "PORT" => Some("0".to_owned()),
            _ => None,
        }
    };
    let (mut unsub_state, _config) =
        unsub::startup_e2e::build_e2e_ports(Arc::clone(&fakes.store), unsub_lookup)?;
    // Share the api's clock and key service, so the job it sealed can be opened
    // and judged due at the same instant; trust the loopback testbed's TLS cert.
    unsub_state.ports.clock = Arc::clone(&fakes.clock) as Arc<dyn Clock>;
    unsub_state.ports.keys = Arc::clone(&fakes.keys) as Arc<dyn KeyService>;
    unsub_state.ports.secrets = Arc::clone(&fakes.secrets) as Arc<dyn Secrets>;
    unsub_state.ports.egress =
        Arc::new(TrustingEgress::new(&one_click, testbed.test_ca_pem())?) as Arc<dyn HttpEgress>;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let unsub_addr = listener.local_addr()?;
    let _unsub_task = tokio::spawn(async move {
        let _ = axum::serve(listener, unsub::startup_e2e::e2e_router(unsub_state)).await;
    });

    // --- the api's local runner (the wiring under test) ---
    let runner_lookup = {
        let base = format!("http://{unsub_addr}");
        move |name: &str| match name {
            "UNSUB_BASE_URL" => Some(base.clone()),
            "UNSUB_E2E_CALLER_TOKEN" => Some(TOKEN.to_owned()),
            _ => None,
        }
    };
    let runner = api::startup_e2e::e2e_local_runner(
        &runner_lookup,
        Arc::clone(&fakes.clock) as Arc<dyn Clock>,
    )?
    .ok_or("the local runner was not built")?;
    ports.scheduler = Arc::clone(&runner) as Arc<dyn ports::JobScheduler>;
    let _runner_task = tokio::spawn({
        let running = Arc::clone(&runner);
        async move { running.run().await }
    });

    let config = ApiConfig::new(
        "https://mailtinder.test".to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?;
    let app = app_state(Arc::new(ports), Arc::new(config));

    // --- seed the user, mailbox, refresh token and message ---
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
    let mailbox = seed_mailbox(&fakes, user, "sub-a", "a@example.com").await?;
    let refresh = "refresh-sub-a".to_owned();
    app.tokens
        .store_refresh_token(&app, &user, &mailbox, Sensitive::new(refresh.clone()))
        .await?;
    fakes
        .identity
        .script_refresh(&refresh, Ok(format!("access-for-{refresh}")));

    let message = fakes.mailbox.seed(
        &mailbox,
        SeedMessage {
            from_display: "Example News".to_owned(),
            from_address: "news@example.com".to_owned(),
            subject: "Weekly digest".to_owned(),
            raw_headers: Vec::new(),
            facts: one_click_facts(one_click),
            preview_text: "preview".to_owned(),
            internal_date: fakes.clock.now(),
            labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
        },
    );

    Ok(Env {
        app,
        fakes,
        user,
        mailbox,
        message,
        runner,
        testbed,
        _unsub_task,
        _runner_task,
    })
}

/// Seed a connected Gmail mailbox for `user`, sealing its address under the
/// user's wrapped key (the refresh token is stored separately).
async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
    sub: &str,
    email: &str,
) -> Result<MailboxId, Box<dyn std::error::Error + Send + Sync>> {
    let subject = ProviderSubjectId::new(sub)?;
    let address = EmailAddress::parse(email)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes.store.users().get(&user).await?.ok_or("user")?;
    let aad = Aad {
        user,
        scope: mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let sealed = fakes
        .keys
        .seal(
            &user,
            &user_record.record.wrapped_data_key,
            &aad,
            address.as_str().as_bytes(),
        )
        .await?;
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: subject,
                email_address: Ciphertext(sealed),
                status: MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

/// Poll `cond` until it holds or `timeout` passes.
async fn wait_for(cond: impl Fn() -> bool, timeout: StdDuration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
    cond()
}

/// A loopback-only [`HttpEgress`] for the testbed that accepts its self-signed
/// test CA. It allows exactly the testbed's socket, like `unsub`'s e2e
/// `LoopbackEgress`, but adds the testbed's certificate trust the loopback
/// harness needs (the production client has no way to trust a per-process CA).
struct TrustingEgress {
    socket: SocketAddr,
    client: reqwest::Client,
}

impl TrustingEgress {
    fn new(base: &Url, ca_pem: &[u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let ip: IpAddr = base.host_str().ok_or("no host")?.parse()?;
        let port = base.port_or_known_default().ok_or("no port")?;
        // Trust exactly the testbed's test CA as an extra root; certificate
        // verification stays on (ASVS V12.3.2).
        let ca = reqwest::Certificate::from_pem(ca_pem)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(ca)
            .build()?;
        Ok(Self {
            socket: SocketAddr::new(ip, port),
            client,
        })
    }

    fn allowed(&self, url: &Url) -> bool {
        url.host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .and_then(|ip| {
                url.port_or_known_default()
                    .map(|port| SocketAddr::new(ip, port) == self.socket)
            })
            .unwrap_or(false)
    }
}

#[async_trait]
impl HttpEgress for TrustingEgress {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError> {
        if !self.allowed(url) {
            return Err(EgressError::HostNotAllowed);
        }
        let response = self
            .client
            .post(url.clone())
            .header("content-type", "application/x-www-form-urlencoded")
            .timeout(egress::ONE_CLICK_TIMEOUT)
            .body(egress::ONE_CLICK_BODY.to_vec())
            .send()
            .await;
        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                Ok(match status {
                    200..=299 => OneClickOutcome::Accepted { status },
                    300..=399 => OneClickOutcome::Redirected { status },
                    _ => OneClickOutcome::Rejected { status },
                })
            }
            Err(err) if err.is_timeout() => Ok(OneClickOutcome::TimedOut),
            Err(_) => Err(EgressError::Connect),
        }
    }

    async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
        // The one-click flow never fetches; only `one_click_post` is used.
        Err(EgressError::NotPermitted)
    }
}

#[tokio::test]
async fn reject_then_unsubscribe_reaches_the_testbed() -> TestResult {
    let env = setup().await?;
    let result = env.reject(1).await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    assert!(
        result.unsubscribe_due_at.is_some(),
        "a reject of a one-click list must queue an unsubscribe"
    );
    let job_id = env.job_id(1);

    // Past the undo window the runner delivers the job to `unsub`, which POSTs
    // the message's one-click URL and the testbed records it. Wait for the
    // runner to finish the delivery (200 from `unsub`), then inspect the record.
    env.fakes.clock.advance(Duration::minutes(6));
    assert!(
        wait_for(
            || env.runner.delivery_state(&job_id) == Some(DeliveryState::Done),
            StdDuration::from_secs(5),
        )
        .await,
        "the runner did not finish delivering the due job"
    );
    assert!(
        !env.testbed.requests_to(ONE_CLICK_PATH).is_empty(),
        "the testbed recorded no one-click POST"
    );

    let records = env.testbed.requests_to(ONE_CLICK_PATH);
    assert_eq!(records.len(), 1, "exactly one one-click request");
    let record = &records[0];
    assert_eq!(record.method, "POST");
    assert_eq!(record.body, b"List-Unsubscribe=One-Click");
    assert!(!record.cookies_present, "no cookie on a one-click POST");
    assert!(
        !record
            .headers
            .iter()
            .any(|(name, _)| name == "authorization"),
        "no credentials on a one-click POST"
    );

    // The job record finished and the runner tracked it `done`.
    let job = env
        .fakes
        .store
        .jobs()
        .get(&job_id)
        .await?
        .ok_or("job record")?
        .record;
    assert_eq!(job.status, JobStatus::Sent, "the job record is done");
    assert_eq!(
        env.runner.delivery_state(&job_id),
        Some(DeliveryState::Done)
    );

    // A second tick delivers nothing more.
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert_eq!(
        env.testbed.requests_to(ONE_CLICK_PATH).len(),
        1,
        "a due job runs exactly once"
    );
    Ok(())
}

#[tokio::test]
async fn undo_before_the_window_prevents_the_unsubscribe() -> TestResult {
    let env = setup().await?;
    let result = env.reject(1).await?;
    let job_id = env.job_id(1);

    let session = env.session(1);
    let undo = api::services::undo::undo(&env.app, &session, &result.undo_token).await?;
    assert!(
        !undo.unsubscribe_already_sent,
        "the cancel won, so nothing was sent"
    );

    // Run the clock past where the window would have passed.
    env.fakes.clock.advance(Duration::minutes(6));
    tokio::time::sleep(StdDuration::from_millis(300)).await;
    assert!(
        env.testbed.requests_to(ONE_CLICK_PATH).is_empty(),
        "an undone unsubscribe must never reach the testbed"
    );
    assert_eq!(
        env.runner.delivery_state(&job_id),
        None,
        "the cancelled job is no longer tracked"
    );
    assert!(
        env.fakes.store.jobs().get(&job_id).await?.is_none(),
        "the cancelled job record is gone"
    );
    Ok(())
}

#[tokio::test]
async fn api_e2e_without_unsub_env_uses_the_fake_scheduler() -> TestResult {
    let clock = Arc::new(testkit::VirtualClock::new(testkit::T0)) as Arc<dyn Clock>;

    // Neither variable set: keep the fake scheduler.
    assert!(api::startup_e2e::e2e_local_runner(&|_| None, Arc::clone(&clock))?.is_none());
    // Only the token set: still the fake.
    let token_only = |name: &str| (name == "UNSUB_E2E_CALLER_TOKEN").then(|| TOKEN.to_owned());
    assert!(api::startup_e2e::e2e_local_runner(&token_only, Arc::clone(&clock))?.is_none());
    // A blank base counts as unset: still the fake.
    let blank = |name: &str| match name {
        "UNSUB_BASE_URL" => Some("   ".to_owned()),
        "UNSUB_E2E_CALLER_TOKEN" => Some(TOKEN.to_owned()),
        _ => None,
    };
    assert!(api::startup_e2e::e2e_local_runner(&blank, Arc::clone(&clock))?.is_none());
    // Both set: the local runner replaces the fake.
    let both = |name: &str| match name {
        "UNSUB_BASE_URL" => Some("http://127.0.0.1:9".to_owned()),
        "UNSUB_E2E_CALLER_TOKEN" => Some(TOKEN.to_owned()),
        _ => None,
    };
    assert!(
        api::startup_e2e::e2e_local_runner(&both, clock)?.is_some(),
        "with both variables set the local runner is used"
    );

    // And the full e2e build still succeeds without the variables (Behaviour 1).
    let store = Arc::new(InMemoryServerStore::new());
    let lookup = |name: &str| match name {
        "FAKE_GOOGLE_URL" => Some("http://127.0.0.1:1".to_owned()),
        "APP_ORIGIN" => Some("http://127.0.0.1:8080".to_owned()),
        "GOOGLE_OAUTH_CLIENT_ID" => Some("e2e-client".to_owned()),
        _ => None,
    };
    let _ports = api::startup_e2e::build_e2e_ports(store, lookup)?;
    Ok(())
}
