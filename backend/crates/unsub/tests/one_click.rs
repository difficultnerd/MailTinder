//! Service integration tests for the one-click sender against
//! `unsub-testbed` (T-702, S2 UN-02, S10 6.2).
//!
//! Every test drives the real `OneClickSender` through `run_job`, with the
//! real `ProdEgress` one-click client. Testbed tests use the documented test
//! override (S10 6.1: the testbed is loopback and plain-http, which the
//! production policy refuses); the SSRF table in `one_click_ssrf.rs` uses the
//! production policy.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason, Provider,
    ProviderSubjectId, UserId,
};
use egress::{ProdEgress, Resolver, Service, TestOverride};
use obs::Sensitive;
use ports::store::{JobOutcomeCode, JobRecord, Precondition};
use ports::{
    Ciphertext, Clock, EgressError, HttpEgress, KeyService, MailboxRecord, OneClickOutcome, Ports,
    Rng, ServerStore, UserRecord,
};
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use testkit::Fakes;
use time::Duration as TimeDuration;
use unsub::one_click::{
    map_one_click, parse_target, OneClickSender, TargetRejected, ONE_CLICK_ROUTE,
};
use unsub::runner::{run_job, Delivery, RunResponse, UnsubState};
use unsub::sender::SendResult;
use unsub_testbed::{start, start_with_untrusted_cert, Testbed};
use url::Url;
use uuid::Uuid;

const AUDIENCE: &str = "unsub-audience";
const CALLER: &str = "tasks@mailtinder.iam.gserviceaccount.com";
/// The test host name the fake resolver maps to the loopback testbed.
const TEST_HOST: &str = "unsub.test";
const LOCALHOST: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

type TestResult = Result<(), Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------
// Fake resolver
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StubResolver {
    /// Hosts that always answer the same addresses (the common case).
    fixed: Mutex<HashMap<String, Vec<IpAddr>>>,
    /// Hosts with a scripted answer per lookup (the DNS-rebinding case).
    scripts: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
    calls: AtomicUsize,
}

impl StubResolver {
    fn new() -> Self {
        Self::default()
    }

    /// Map `host` to `addrs` for every lookup.
    fn map(&self, host: &str, addrs: Vec<IpAddr>) {
        self.fixed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(host.to_owned(), addrs);
    }

    /// Answer `host` with one list per lookup, in order.
    fn seq(&self, host: &str, answers: Vec<Vec<IpAddr>>) {
        let mut scripts = self
            .scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scripts.insert(host.to_owned(), VecDeque::from(answers));
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Resolver for StubResolver {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, EgressError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = self
            .fixed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(host)
        {
            return Ok(answer.clone());
        }
        let mut scripts = self
            .scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(queue) = scripts.get_mut(host) {
            if let Some(answer) = queue.pop_front() {
                return Ok(answer);
            }
        }
        Err(EgressError::DnsFailed)
    }
}

// ---------------------------------------------------------------------------
// Egress clients
// ---------------------------------------------------------------------------

/// The production `unsub` policy with a fake resolver.
fn production(resolver: Arc<dyn Resolver>) -> Result<Arc<dyn HttpEgress>, EgressError> {
    Ok(Arc::new(ProdEgress::new(Service::Unsub, resolver)?))
}

/// The one-click test policy: the testbed's plain-http socket and nothing else.
fn testbed_policy(testbed: &Testbed, resolver: Arc<dyn Resolver>) -> Arc<dyn HttpEgress> {
    let client = ProdEgress::with_test_override(
        Service::Unsub,
        resolver,
        TestOverride {
            allow_socket: Some(testbed.addr),
            allow_plain_http_to_socket: true,
            extra_root_ca_pem: None,
            host_routes: Vec::new(),
            one_click_timeout: Duration::from_millis(250),
        },
    )
    .unwrap_or_else(|_| panic!("test override client"));
    Arc::new(client)
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Env {
    ports: Ports,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
}

async fn env_with(
    egress: Arc<dyn HttpEgress>,
    with_refresh: bool,
) -> Result<Env, Box<dyn std::error::Error>> {
    let (mut ports, fakes) = testkit::fake_ports();
    ports.egress = egress;
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(
        &ports,
        &fakes,
        &user,
        MailboxStatus::Connected,
        with_refresh,
    )
    .await?;
    Ok(Env {
        ports,
        fakes,
        user,
        mailbox,
    })
}

/// An env plus a job targeting `target`.
async fn env_job(
    egress: Arc<dyn HttpEgress>,
    target: &str,
) -> Result<(Env, JobId), Box<dyn std::error::Error>> {
    let env = env_with(egress, false).await?;
    let id = seed_job(&env, target).await?;
    Ok((env, id))
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
            svc_common::mint::seal_refresh_token(
                ports,
                &user_record,
                &mailbox,
                &Sensitive::new("CANARY-refresh-token-0001".to_owned()),
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

async fn seed_job(env: &Env, target: &str) -> Result<JobId, Box<dyn std::error::Error>> {
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
        &Sensitive::new("Example News".to_owned()),
    )
    .await?;
    let now = env.fakes.clock.now();
    let record = JobRecord {
        job_id,
        user_id: env.user,
        mailbox_id: env.mailbox,
        list_key_hash: ports::ListKeyHash([1u8; 32]),
        method: JobMethod::OneClick,
        target: Some(sealed),
        sender_display: Some(display),
        due_at: now,
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: now + TimeDuration::hours(1),
    };
    env.ports
        .store
        .jobs()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(job_id)
}

fn state(env: &Env) -> UnsubState {
    UnsubState {
        ports: env.ports.clone(),
        senders: vec![Arc::new(OneClickSender)],
        auth: InternalAuthConfig {
            audience: AUDIENCE.to_owned(),
            allowed_caller_email: CALLER.to_owned(),
        },
    }
}

async fn run(
    state: &UnsubState,
    id: &JobId,
    attempt: u32,
) -> Result<RunResponse, Box<dyn std::error::Error>> {
    Ok(run_job(state, id, Delivery { attempt }).await?)
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

async fn item_reason(env: &Env) -> Result<NeedsAttentionReason, Box<dyn std::error::Error>> {
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
    Ok(page.items[0].record.reason_code)
}

/// The testbed target URL for `path`: a domain host (the production host rules
/// refuse IP literals and `localhost`) on the testbed's real port, so the
/// pinned connection lands on the testbed socket.
fn testbed_target(testbed: &Testbed, path: &str) -> String {
    format!("http://{TEST_HOST}:{}{path}", testbed.addr.port())
}

// ---------------------------------------------------------------------------
// map_one_click (unit)
// ---------------------------------------------------------------------------

/// Every row of the task's mapping table, one case per enum variant.
#[test]
fn map_one_click_table() {
    use JobOutcomeCode as C;
    use NeedsAttentionReason as R;
    let cases: Vec<(Result<OneClickOutcome, EgressError>, SendResult)> = vec![
        (
            Ok(OneClickOutcome::Accepted { status: 200 }),
            SendResult::Sent {
                code: C::OneClickAccepted,
            },
        ),
        (
            Ok(OneClickOutcome::Redirected { status: 302 }),
            SendResult::NeedsAttention {
                reason: R::OneClickRedirect,
                code: C::Redirected,
            },
        ),
        (
            Ok(OneClickOutcome::Rejected { status: 400 }),
            SendResult::Retryable {
                code: C::HttpRejected,
            },
        ),
        (
            Ok(OneClickOutcome::TimedOut),
            SendResult::Retryable { code: C::TimedOut },
        ),
        (
            Err(EgressError::Timeout),
            SendResult::Retryable { code: C::TimedOut },
        ),
        (
            Err(EgressError::AddressRefused(ports::RefusedRange::Loopback)),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::AddressRefused,
            },
        ),
        (
            Err(EgressError::IpLiteralHost),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::AddressRefused,
            },
        ),
        (
            Err(EgressError::SchemeNotAllowed),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::Refused,
            },
        ),
        (
            Err(EgressError::CredentialsInUrl),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::Refused,
            },
        ),
        (
            Err(EgressError::PortNotAllowed),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::Refused,
            },
        ),
        (
            Err(EgressError::HostNotAllowed),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::Refused,
            },
        ),
        (
            Err(EgressError::NotPermitted),
            SendResult::NeedsAttention {
                reason: R::OneClickAddressRefused,
                code: C::Refused,
            },
        ),
        (
            Err(EgressError::DnsFailed),
            SendResult::Retryable {
                code: C::HttpRejected,
            },
        ),
        (
            Err(EgressError::Connect),
            SendResult::Retryable {
                code: C::HttpRejected,
            },
        ),
        (
            Err(EgressError::ResponseTooLarge),
            SendResult::Retryable {
                code: C::HttpRejected,
            },
        ),
        (
            Err(EgressError::Tls),
            SendResult::Retryable {
                code: C::HttpRejected,
            },
        ),
        (
            Err(EgressError::PermanentDeleteRefused),
            SendResult::NeedsAttention {
                reason: R::UnsubscribeFailed,
                code: C::Refused,
            },
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(map_one_click(&input), expected, "for {input:?}");
    }
}

/// `parse_target` refuses what it must before any network call (ASVS V1.3.6).
#[test]
fn asvs_v1_3_6_one_click_target_checked_before_connect() {
    assert!(matches!(
        parse_target("not a url"),
        Err(TargetRejected::NotUrl)
    ));
    assert!(matches!(
        parse_target("mailto:unsub@lists.example.test?subject=x"),
        Err(TargetRejected::NotHttps)
    ));
    assert!(matches!(
        parse_target("javascript:alert(1)"),
        Err(TargetRejected::NotHttps)
    ));
    assert!(matches!(
        parse_target("https://user:pw@lists.example.test/unsubscribe"),
        Err(TargetRejected::HasUserinfo)
    ));
    assert!(matches!(
        parse_target("https://user@lists.example.test/unsubscribe"),
        Err(TargetRejected::HasUserinfo)
    ));
    // A clean target parses and keeps its query.
    let ok = parse_target("https://lists.example.test/unsubscribe?token=abc").unwrap();
    assert_eq!(ok.scheme(), "https");
    assert_eq!(ok.host_str(), Some("lists.example.test"));
}

// ---------------------------------------------------------------------------
// UN-01 AC5: one-click needs no mailbox token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_01_ac5_one_click_needs_no_token() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/200"),
    )
    .await?;
    // The mailbox has no refresh token at all: a token-minting path could not
    // succeed, so a delivered POST proves no mailbox token was needed.
    let mailbox = env
        .ports
        .store
        .mailboxes()
        .get(&env.mailbox)
        .await?
        .ok_or("missing mailbox")?;
    assert!(mailbox.record.refresh_token.is_none());

    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Sent);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::OneClickAccepted)
    );
    assert_eq!(testbed.requests_to("/oneclick/200").len(), 1);
    testbed.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-02 AC1: the fixed RFC 8058 request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_02_ac1_post_has_fixed_body_no_cookies_no_auth() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/200"),
    )
    .await?;
    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);

    let records = testbed.requests_to("/oneclick/200");
    assert_eq!(records.len(), 1, "exactly one POST");
    let record = &records[0];
    assert_eq!(record.method, "POST");
    assert_eq!(record.body, b"List-Unsubscribe=One-Click");
    assert!(!record.cookies_present, "no Cookie header");
    let header = |name: &str| {
        record
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(
        header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(header("user-agent"), Some("MailTinder-Unsubscribe/1"));
    for forbidden in ["cookie", "authorization", "referer", "origin"] {
        assert!(header(forbidden).is_none(), "must not send {forbidden}");
        assert!(
            !record.headers.iter().any(|(k, _)| k == forbidden),
            "must not send {forbidden}"
        );
    }
    testbed.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-02 AC3: 2xx marks sent; anything else retries, then Needs Attention
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_02_ac3_2xx_marks_sent() -> TestResult {
    for status in [200_u16, 202, 204] {
        let testbed = start().await?;
        let resolver = Arc::new(StubResolver::new());
        resolver.map(TEST_HOST, vec![LOCALHOST]);
        let path = format!("/oneclick/{status}");
        let (env, id) = env_job(
            testbed_policy(&testbed, resolver),
            &testbed_target(&testbed, &path),
        )
        .await?;
        let state = state(&env);
        assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
        let stored = job(&env, &id).await?;
        assert_eq!(stored.status, JobStatus::Sent, "for {status}");
        assert_eq!(
            stored.outcome.map(|o| o.code),
            Some(JobOutcomeCode::OneClickAccepted),
            "for {status}"
        );
        testbed.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn un_02_ac3_500_then_200_sent_on_retry() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/500-then-200/case"),
    )
    .await?;
    let state = state(&env);
    // Attempt 0: the 500 is transient, so the job stays Running.
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Retry);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Running);
    // Attempt 1: 200.
    assert_eq!(run(&state, &id, 1).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Sent);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::OneClickAccepted)
    );
    assert_eq!(
        testbed.requests_to("/oneclick/500-then-200/case").len(),
        2,
        "one request per attempt"
    );
    testbed.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn un_02_ac3_400_retries_then_needs_attention() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/400"),
    )
    .await?;
    let state = state(&env);
    for attempt in 0..3 {
        assert_eq!(run(&state, &id, attempt).await?, RunResponse::Retry);
    }
    assert_eq!(run(&state, &id, 3).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::RetriesExhausted)
    );
    assert_eq!(
        item_reason(&env).await?,
        NeedsAttentionReason::UnsubscribeFailed
    );
    assert_eq!(
        testbed.requests_to("/oneclick/400").len(),
        4,
        "one request per attempt"
    );
    testbed.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn un_02_ac3_four_failures_go_to_needs_attention() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/fail-n/4/case"),
    )
    .await?;
    let state = state(&env);
    for attempt in 0..3 {
        assert_eq!(run(&state, &id, attempt).await?, RunResponse::Retry);
    }
    assert_eq!(run(&state, &id, 3).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        testbed.requests_to("/oneclick/fail-n/4/case").len(),
        4,
        "the testbed saw exactly four requests"
    );
    assert_eq!(
        item_reason(&env).await?,
        NeedsAttentionReason::UnsubscribeFailed
    );
    testbed.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn un_02_ac3_timeout_retries() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/hang"),
    )
    .await?;
    let state = state(&env);
    // The test policy's 250 ms timeout fires on the never-answering route.
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Retry);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::Running, "a timeout is retryable");
    assert_eq!(testbed.requests_to("/oneclick/hang").len(), 1);
    testbed.release_hanging();
    testbed.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-02 AC4: no redirects; the resolved address is pinned
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_02_ac4_3xx_not_followed_not_retried() -> TestResult {
    for code in [301_u16, 302, 303, 307, 308] {
        let testbed = start().await?;
        let resolver = Arc::new(StubResolver::new());
        resolver.map(TEST_HOST, vec![LOCALHOST]);
        let path = format!("/oneclick/redirect/{code}");
        let (env, id) = env_job(
            testbed_policy(&testbed, resolver),
            &testbed_target(&testbed, &path),
        )
        .await?;
        let state = state(&env);
        // A 3xx is terminal on the first delivery: no retry, no redirect.
        assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
        let stored = job(&env, &id).await?;
        assert_eq!(stored.status, JobStatus::NeedsAttention, "for {code}");
        assert_eq!(
            stored.outcome.map(|o| o.code),
            Some(JobOutcomeCode::Redirected),
            "for {code}"
        );
        assert_eq!(
            item_reason(&env).await?,
            NeedsAttentionReason::OneClickRedirect,
            "for {code}"
        );
        assert_eq!(testbed.requests_to(&path).len(), 1, "for {code}");
        assert_eq!(
            testbed.requests_to("/landed").len(),
            0,
            "the redirect target is never contacted: {code}"
        );
        assert_eq!(testbed.requests().len(), 1, "one request in total: {code}");
        testbed.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn un_02_ac4_dns_rebinding_uses_pinned_address() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    // First answer: the allowed testbed socket. Second: a metadata address.
    resolver.seq(
        TEST_HOST,
        vec![vec![LOCALHOST], vec!["169.254.169.254".parse::<IpAddr>()?]],
    );
    let (env, id) = env_job(
        testbed_policy(&testbed, Arc::clone(&resolver) as Arc<dyn Resolver>),
        &testbed_target(&testbed, "/oneclick/200"),
    )
    .await?;
    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Sent);
    assert_eq!(
        resolver.calls(),
        1,
        "the address is resolved once and pinned"
    );
    assert_eq!(
        testbed.requests_to("/oneclick/200").len(),
        1,
        "the connection used the first, checked address"
    );
    testbed.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS rows
// ---------------------------------------------------------------------------

/// ASVS V12.2.1: https only; an `http` target is refused with zero connections.
#[tokio::test]
async fn asvs_v12_2_1_http_target_refused() -> TestResult {
    // Unit: the production one-click client refuses a plain-http URL before
    // any DNS lookup or connection.
    let resolver = Arc::new(StubResolver::new());
    resolver.map(
        "lists.example.test",
        vec!["93.184.216.34".parse::<IpAddr>()?],
    );
    let client = production(Arc::clone(&resolver) as Arc<dyn Resolver>)?;
    let url = Url::parse("http://lists.example.test/unsubscribe")?;
    assert_eq!(
        client.one_click_post(&url).await.unwrap_err(),
        EgressError::SchemeNotAllowed
    );
    assert_eq!(resolver.calls(), 0, "refused before any DNS lookup");

    // Service integration: with the production policy the job ends Needs
    // Attention and no connection is made.
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        production(Arc::clone(&resolver) as Arc<dyn Resolver>)?,
        "http://unsub.test/oneclick/200",
    )
    .await?;
    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
    let stored = job(&env, &id).await?;
    assert_eq!(stored.status, JobStatus::NeedsAttention);
    assert_eq!(
        stored.outcome.map(|o| o.code),
        Some(JobOutcomeCode::Refused)
    );
    assert_eq!(resolver.calls(), 0, "refused before any DNS lookup");
    assert_eq!(
        item_reason(&env).await?,
        NeedsAttentionReason::OneClickAddressRefused
    );
    Ok(())
}

/// ASVS V13.2.4: the `unsub` allowlist has no Drive host and never reaches it.
#[test]
fn asvs_v13_2_4_unsub_never_reaches_drive() {
    let list = egress::allowlist(Service::Unsub);
    assert!(!list.is_empty());
    for endpoint in list {
        assert!(
            !endpoint.path_prefix.contains("drive"),
            "the unsub allowlist must not carry a Drive endpoint"
        );
        assert!(
            !endpoint.host.starts_with("drive"),
            "the unsub allowlist must not carry a Drive host"
        );
    }
    let drive = Url::parse("https://www.googleapis.com/drive/v3/files").unwrap();
    assert!(
        !egress::allows(Service::Unsub, &drive),
        "unsub must not be allowed to call Drive"
    );
    assert!(!egress::allows(
        Service::Unsub,
        &Url::parse("https://gmail.googleapis.com/drive/v3/files").unwrap()
    ));
}

/// ASVS V13.2.5: any other host is refused before connecting, with the
/// production policy.
#[tokio::test]
async fn asvs_v13_2_5_unsub_other_host_refused() -> TestResult {
    let resolver = Arc::new(StubResolver::new());
    // The resolver would answer if it were ever consulted.
    resolver.map("svc.internal", vec!["10.0.0.1".parse::<IpAddr>()?]);
    let (env, id) = env_job(
        production(Arc::clone(&resolver) as Arc<dyn Resolver>)?,
        "https://svc.internal/unsubscribe",
    )
    .await?;
    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
    assert_eq!(job(&env, &id).await?.status, JobStatus::NeedsAttention);
    assert_eq!(resolver.calls(), 0, "refused before any DNS lookup");
    assert_eq!(
        item_reason(&env).await?,
        NeedsAttentionReason::OneClickAddressRefused
    );
    Ok(())
}

/// ASVS V15.3.2: the one-click POST follows no redirect.
#[tokio::test]
async fn asvs_v15_3_2_redirect_not_followed() -> TestResult {
    let testbed = start().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let (env, id) = env_job(
        testbed_policy(&testbed, resolver),
        &testbed_target(&testbed, "/oneclick/redirect/302"),
    )
    .await?;
    let state = state(&env);
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Done);
    assert_eq!(testbed.requests_to("/landed").len(), 0);
    assert_eq!(testbed.requests().len(), 1);
    assert_eq!(
        job(&env, &id).await?.outcome.map(|o| o.code),
        Some(JobOutcomeCode::Redirected)
    );
    testbed.shutdown().await;
    Ok(())
}

/// ASVS V16.3.4: a backend TLS failure is logged, without the target.
#[tokio::test]
async fn asvs_v16_3_4_tls_failure_logged() -> TestResult {
    let testbed = start_with_untrusted_cert().await?;
    let resolver = Arc::new(StubResolver::new());
    resolver.map(TEST_HOST, vec![LOCALHOST]);
    let client = ProdEgress::with_test_override(
        Service::Unsub,
        Arc::clone(&resolver) as Arc<dyn Resolver>,
        TestOverride {
            allow_socket: Some(testbed.https_addr),
            allow_plain_http_to_socket: false,
            // No test CA: the listener's certificate is untrusted.
            extra_root_ca_pem: None,
            host_routes: Vec::new(),
            one_click_timeout: Duration::from_millis(250),
        },
    )?;
    let target = format!(
        "https://{TEST_HOST}:{}/oneclick/200",
        testbed.https_addr.port()
    );
    let (env, id) = env_job(Arc::new(client), &target).await?;
    let state = state(&env);

    let (capture, guard) = obs::capture("unsub", obs::arc(obs::FixedClock(env.fakes.clock.now())));
    assert_eq!(run(&state, &id, 0).await?, RunResponse::Retry);
    let text = capture.text();
    assert!(
        text.contains("egress_tls_failure"),
        "the TLS failure is a security event: {text}"
    );
    assert!(
        text.contains(ONE_CLICK_ROUTE) && text.contains("tls_failure"),
        "the sender files the failure under its route template: {text}"
    );
    assert!(!text.contains(TEST_HOST), "the host never reaches the log");
    assert!(
        !text.contains("/oneclick/200"),
        "the URL never reaches the log"
    );
    drop(guard);
    assert_eq!(job(&env, &id).await?.status, JobStatus::Running);
    testbed.shutdown().await;
    Ok(())
}
