//! SSRF table for the one-click target (T-702, S10 6.2, ASVS V1.3.6/V12).
//!
//! These cases run with the **production** `unsub` policy and a fake resolver:
//! no override, no testbed, no real network. A refused case must end Needs
//! Attention with `one_click_address_refused` and make no connection.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason, Provider,
    ProviderSubjectId, UserId,
};
use egress::{ProdEgress, Resolver, Service};
use obs::Sensitive;
use ports::store::{JobRecord, Precondition};
use ports::{
    Ciphertext, Clock, EgressError, HttpEgress, KeyService, MailboxRecord, Ports, Rng, ServerStore,
    UserRecord,
};
use svc_common::internal_auth::InternalAuthConfig;
use svc_common::job_record;
use time::Duration as TimeDuration;
use unsub::one_click::OneClickSender;
use unsub::runner::{run_job, Delivery, RunResponse, UnsubState};
use unsub_testbed::SSRF_CASES;
use uuid::Uuid;

const AUDIENCE: &str = "unsub-audience";
const CALLER: &str = "tasks@mailtinder.iam.gserviceaccount.com";

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct StubResolver {
    scripts: Mutex<HashMap<String, Vec<IpAddr>>>,
    calls: AtomicUsize,
}

impl StubResolver {
    fn map(&self, host: &str, addrs: Vec<IpAddr>) {
        self.scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(host.to_owned(), addrs);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Resolver for StubResolver {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, EgressError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(host)
            .cloned()
            .ok_or(EgressError::DnsFailed)
    }
}

struct Env {
    ports: Ports,
    user: UserId,
}

/// The production `unsub` policy over the fake resolver.
fn production(resolver: Arc<StubResolver>) -> Result<Arc<dyn HttpEgress>, EgressError> {
    Ok(Arc::new(ProdEgress::new(
        Service::Unsub,
        resolver as Arc<dyn Resolver>,
    )?))
}

async fn setup(
    egress: Arc<dyn HttpEgress>,
    target: &str,
) -> Result<(Env, JobId), Box<dyn std::error::Error>> {
    let (mut ports, fakes) = testkit::fake_ports();
    ports.egress = egress;
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
    let mailbox = MailboxId::new(fakes.rng.uuid_v4());
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id: mailbox,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: ProviderSubjectId::new("sub-1")?,
                email_address: Ciphertext(vec![7u8; 24]),
                status: MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;

    let job_id = JobId(Uuid::new_v4());
    let sealed =
        job_record::seal_target(&ports, &user, &job_id, &Sensitive::new(target.to_owned())).await?;
    let display = job_record::seal_sender_display(
        &ports,
        &user,
        &job_id,
        &Sensitive::new("Example News".to_owned()),
    )
    .await?;
    let now = fakes.clock.now();
    ports
        .store
        .jobs()
        .put(
            &JobRecord {
                job_id,
                user_id: user,
                mailbox_id: mailbox,
                list_key_hash: ports::ListKeyHash([1u8; 32]),
                method: JobMethod::OneClick,
                target: Some(sealed),
                sender_display: Some(display),
                due_at: now,
                status: JobStatus::Queued,
                attempts: 0,
                outcome: None,
                expires_at: now + TimeDuration::hours(1),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok((Env { ports, user }, job_id))
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

/// Run one delivery of a job targeting `target`; assert it refused with the
/// address-check reason and that the resolver was consulted exactly
/// `expected_lookups` times (never more, never after a refusal).
async fn assert_refused(
    target: &str,
    resolver: Arc<StubResolver>,
    expected_lookups: usize,
) -> TestResult {
    let (env, id) = setup(production(Arc::clone(&resolver))?, target).await?;
    let state = state(&env);
    assert_eq!(
        run_job(&state, &id, Delivery { attempt: 0 }).await?,
        RunResponse::Done
    );
    let stored = env
        .ports
        .store
        .jobs()
        .get(&id)
        .await?
        .ok_or("missing job")?
        .record;
    assert_eq!(stored.status, JobStatus::NeedsAttention, "for {target}");
    assert_eq!(resolver.calls(), expected_lookups, "lookups for {target}");
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
    assert_eq!(page.items.len(), 1, "for {target}");
    assert_eq!(
        page.items[0].record.reason_code,
        NeedsAttentionReason::OneClickAddressRefused,
        "for {target}"
    );
    Ok(())
}

/// The S10 6.2 table: every refused range, in both address and literal form
/// (ASVS V1.3.6, V12.2.1). No request is ever sent.
#[tokio::test]
async fn un_02_ac4_private_targets_refused() -> TestResult {
    // Literal hosts and reserved names: refused before any DNS lookup.
    let literals = [
        "https://127.0.0.1/",
        "https://10.0.0.1/",
        "https://172.16.0.1/",
        "https://192.168.1.1/",
        "https://100.64.0.1/",
        "https://169.254.169.254/",
        "https://metadata.google.internal/",
        "https://[::1]/",
        "https://[fe80::1]/",
        "https://[fd00::1]/",
        "https://[::ffff:127.0.0.1]/",
        "https://2130706433/",
        "https://0177.0.0.1/",
    ];
    for target in literals {
        let resolver = Arc::new(StubResolver::default());
        assert_refused(target, resolver, 0).await?;
    }

    // Host names the fake resolver maps to forbidden addresses: the lookup
    // happens once and the answer is refused before any connection.
    for case in SSRF_CASES {
        let resolver = Arc::new(StubResolver::default());
        resolver.map(case.host, case.resolves_to.to_vec());
        assert_refused(&format!("https://{}/", case.host), resolver, 1).await?;
    }
    Ok(())
}

/// A `http://` target and an internal name are refused by the production policy
/// in both the unit table and the service path (ASVS V12.2.1, V13.2.5).
#[tokio::test]
async fn ssrf_http_and_internal_names_refused() -> TestResult {
    let http = [
        "http://lists.example.test/unsubscribe?token=abc",
        "http://127.0.0.1/",
        "http://metadata.google.internal/",
    ];
    for target in http {
        let resolver = Arc::new(StubResolver::default());
        resolver.map(
            "lists.example.test",
            vec!["93.184.216.34".parse::<IpAddr>()?],
        );
        assert_refused(target, resolver, 0).await?;
    }
    let internal = [
        "https://svc.internal/unsubscribe",
        "https://printer.local/unsubscribe",
        "https://localhost/unsubscribe",
        "https://metadata/unsubscribe",
    ];
    for target in internal {
        let resolver = Arc::new(StubResolver::default());
        assert_refused(target, resolver, 0).await?;
    }
    Ok(())
}
