//! The staging smoke checks (T-1105, S10 3.1 "Staging smoke", S10 6.2, S11 7.1).
//!
//! After each deploy to staging, `main` runs every function below against the
//! real platform: Cloud KMS wraps and unwraps, Firestore reads and writes,
//! Cloud Tasks schedules, cancels and delivers, the deployed `unsub` refuses
//! the metadata server as a one-click target, the internal services are not
//! public, and the hosted app and API carry their headers.
//!
//! These are not `#[test]`s: they need real cloud credentials and a deployed
//! platform, so they run from `.github/workflows/smoke.yml` only, never in pull
//! request CI (S10 10.1). `tools/check_ac_coverage.py` finds the acceptance
//! criterion in the function name below.
//!
//! Two rules hold throughout: nothing here is ever pointed at production (its
//! identity exists only in the staging root, `smoke.tf`), and every document a
//! check creates is deleted again, including when the check fails.

use std::sync::Arc;
use std::time::Instant;

use adapters_gcp::{
    production_clock, production_rng, CloudKms, CloudTasksScheduler, EnvelopeKeyService,
    FirestoreConfig, FirestoreStore, GcpHttp, MetadataTokenSource, StaticTokenSource, TasksConfig,
    TokenSource,
};
use domain::{
    JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, NeedsAttentionReason, Provider,
    ProviderSubjectId, Tunables, UserId,
};
use obs::Sensitive;
use ports::store::{aad_fields, Ciphertext, JobRecord, ListKeyHash, MailboxRecord, UserRecord};
use ports::{Aad, CancelOutcome, PageRequest, Ports, Precondition, WrappedKey, MAX_PAGE};
use reqwest::header::{self, HeaderMap};
use time::Duration;
use url::Url;

use crate::unused::Unused;

/// The one acceptance criterion this task proves: UN-02 AC4, in staging.
pub const AC4_CHECK: &str = "un_02_ac4_staging_unsub_refuses_metadata_target";
/// The header check's name.
const HEADERS_CHECK: &str = "hosting_and_api_headers";
/// The internal-service check's name.
const INTERNAL_CHECK: &str = "internal_services_not_public";
/// The KMS check's name.
const KMS_CHECK: &str = "kms_round_trip";
/// The Firestore check's name.
const FIRESTORE_CHECK: &str = "firestore_round_trip";
/// The Cloud Tasks check's name.
const TASKS_CHECK: &str = "tasks_schedule_and_cancel";

/// The AAD scope for the KMS round trip. A constant, never user input.
const SMOKE_SCOPE: &str = "smoke";
/// The AAD field name for the KMS round trip.
const SMOKE_FIELD: &str = "smoke";
/// The first one-click target UN-02 AC4 refuses: the metadata host name.
const METADATA_HOST_TARGET: &str = "https://metadata.google.internal/computeMetadata/v1/";
/// The second: the metadata address as a literal, with no DNS lookup at all.
const METADATA_IP_TARGET: &str = "https://169.254.169.254/";
/// How often a refusal is looked for, and for how long.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// The polling budget. A Cloud Tasks delivery, plus the refusals, fits well
/// inside it; past it the check fails rather than hang.
const POLL_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// The access token variable, set by `smoke.yml` (WIF as `mt-smoke`). When it
/// is absent the binary falls back to the instance metadata server, which is
/// how the same binary would run from inside the project.
pub const ACCESS_TOKEN_ENV: &str = "MT_SMOKE_ACCESS_TOKEN";
/// What [`SmokeConfig`]'s hand-written `Debug` prints in place of the key
/// resource name, the queue name and the three service URLs.
const REDACTED: &str = "<redacted>";

/// The `MT_SMOKE_*` configuration, resolved once at start-up.
///
/// No `Debug`: the struct holds a key resource name and three service URLs, and
/// the privacy rules refuse a derived `Debug` on a struct with an `…_url` or
/// `…_key` field (`.semgrep/privacy.yml`). Nothing logs it, so none is derived.
#[derive(Clone)]
pub struct SmokeConfig {
    /// `MT_SMOKE_PROJECT`: the staging project id.
    pub project: String,
    /// `MT_SMOKE_REGION`: the region every regional service is in.
    pub region: String,
    /// `MT_SMOKE_KMS_KEY`: full resource name of the key that wraps user keys.
    pub kms_key: String,
    /// `MT_SMOKE_QUEUE`: full resource name of the Cloud Tasks queue.
    pub queue: String,
    /// `MT_SMOKE_UNSUB_URL`: the deployed `unsub` Cloud Run URL.
    pub unsub_url: Url,
    /// `MT_SMOKE_WORKER_URL`: the deployed `worker` Cloud Run URL.
    pub worker_url: Url,
    /// `MT_SMOKE_APP_URL`: the staging Hosting URL.
    pub app_url: Url,
    /// `MT_SMOKE_TASKS_INVOKER`: the service account Cloud Tasks signs as.
    pub tasks_invoker: String,
}

impl SmokeConfig {
    /// Resolve every `MT_SMOKE_*` variable from a lookup, so callers and tests
    /// never touch the process environment.
    ///
    /// # Errors
    ///
    /// [`SmokeError::Missing`] for an absent or blank required variable (only
    /// ever its NAME is surfaced), [`SmokeError::Invalid`] for a malformed URL.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, SmokeError> {
        Ok(Self {
            project: required(&lookup, "MT_SMOKE_PROJECT")?,
            region: required(&lookup, "MT_SMOKE_REGION")?,
            kms_key: required(&lookup, "MT_SMOKE_KMS_KEY")?,
            queue: required(&lookup, "MT_SMOKE_QUEUE")?,
            unsub_url: url_of(&lookup, "MT_SMOKE_UNSUB_URL")?,
            worker_url: url_of(&lookup, "MT_SMOKE_WORKER_URL")?,
            app_url: url_of(&lookup, "MT_SMOKE_APP_URL")?,
            tasks_invoker: required(&lookup, "MT_SMOKE_TASKS_INVOKER")?,
        })
    }

    /// As [`SmokeConfig::from_lookup`], over the process environment.
    ///
    /// # Errors
    ///
    /// As [`SmokeConfig::from_lookup`].
    pub fn from_env() -> Result<Self, SmokeError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
}

/// A `Debug` written by hand, as the privacy rules require for a struct whose
/// fields are named `…_key` and `…_url`: the key resource name and the three
/// service URLs are redacted, so a stray `{:?}` cannot leak one into a log. The
/// project and the region are not secrets and stay readable.
impl std::fmt::Debug for SmokeConfig {
    /// Every field is written, none of them with its value unless it is
    /// non-sensitive.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmokeConfig")
            .field("project", &self.project)
            .field("region", &self.region)
            .field("kms_key", &REDACTED)
            .field("queue", &REDACTED)
            .field("unsub_url", &REDACTED)
            .field("worker_url", &REDACTED)
            .field("app_url", &REDACTED)
            .field("tasks_invoker", &REDACTED)
            .finish()
    }
}

/// A smoke failure. The `Display` text never carries a document, a token or a
/// URL with a secret: only a variable name or a check name.
#[derive(Debug, thiserror::Error)]
pub enum SmokeError {
    /// A required environment variable was absent or blank (named only).
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    /// A present but malformed value (named only).
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    /// A check reached a wrong state. The payload is the check's name.
    #[error("check failed: {0}")]
    Failed(&'static str),
    /// A Firestore store error.
    #[error("store unavailable")]
    Store(#[from] ports::StoreError),
    /// A key service error.
    #[error("key service failed")]
    Key(#[from] ports::KeyError),
    /// A scheduler error.
    #[error("scheduler failed")]
    Sched(#[from] ports::SchedError),
    /// An HTTP error.
    #[error("http failed")]
    Http(#[from] reqwest::Error),
    /// A domain value could not be built.
    #[error("invalid domain value")]
    Domain(#[from] domain::DomainError),
}

/// Build the real `Ports`: the `adapters-gcp` Firestore store (T-301), KMS
/// envelope keys (T-302) and Cloud Tasks scheduler (T-304), on the system clock
/// and RNG. Every other port is [`Unused`] (see that module).
///
/// The bearer token comes from `MT_SMOKE_ACCESS_TOKEN` when `smoke.yml` has
/// exchanged the WIF identity for one; otherwise the instance metadata server
/// is used, as on a GCE/Cloud Run host.
///
/// # Errors
///
/// [`SmokeError::Invalid`] when the platform HTTP client cannot be built.
pub fn build_ports(cfg: &SmokeConfig) -> Result<Ports, SmokeError> {
    // The queue must be the one in the configured region: a queue resource
    // name is opaque, so a typo would otherwise enqueue into another region.
    if !cfg
        .queue
        .contains(&format!("/locations/{}/queues/", cfg.region))
    {
        return Err(SmokeError::Invalid("MT_SMOKE_QUEUE"));
    }
    let clock = production_clock();
    let rng = production_rng();

    let tokens: Arc<dyn TokenSource> = match std::env::var(ACCESS_TOKEN_ENV) {
        Ok(value) if !value.trim().is_empty() => Arc::new(StaticTokenSource(Sensitive::new(value))),
        _ => Arc::new(MetadataTokenSource::new(Arc::clone(&clock))),
    };
    let http = Arc::new(GcpHttp::new(tokens).map_err(|_| SmokeError::Invalid("MT_SMOKE_PROJECT"))?);

    let store: Arc<dyn ports::ServerStore> = Arc::new(FirestoreStore::new(
        Arc::clone(&http),
        FirestoreConfig::new(cfg.project.clone()),
    ));
    let keys: Arc<dyn ports::KeyService> = Arc::new(EnvelopeKeyService::new(
        Arc::new(CloudKms::new(Arc::clone(&http), cfg.kms_key.clone())),
        Arc::clone(&rng),
        Arc::clone(&clock),
    ));
    let scheduler: Arc<dyn ports::JobScheduler> = Arc::new(CloudTasksScheduler::new(
        Arc::clone(&http),
        TasksConfig {
            queue: cfg.queue.clone(),
            unsub_base: cfg.unsub_url.clone(),
            invoker_sa_email: cfg.tasks_invoker.clone(),
            audience: cfg.unsub_url.as_str().trim_end_matches('/').to_owned(),
        },
    ));

    let inert: Arc<Unused> = Arc::new(Unused);
    Ok(Ports {
        clock,
        rng,
        gmail: Arc::clone(&inert) as Arc<dyn ports::MailProvider>,
        app_folder: Arc::clone(&inert) as Arc<dyn ports::AppFolderStore>,
        store,
        keys,
        system_keys: Arc::clone(&inert) as Arc<dyn ports::SystemKeyService>,
        scheduler,
        egress: Arc::clone(&inert) as Arc<dyn ports::HttpEgress>,
        identity: Arc::clone(&inert) as Arc<dyn ports::IdentityProvider>,
        invite_mailer: Arc::clone(&inert) as Arc<dyn ports::InviteMailer>,
        models: Vec::new(),
        secrets: Arc::clone(&inert) as Arc<dyn ports::Secrets>,
        caller: Arc::clone(&inert) as Arc<dyn ports::CallerVerifier>,
    })
}

/// KMS wraps a fresh user key and seals a random 32-byte value, which `open`
/// must return unchanged (S11 RB-KMS: after a rotation, `api` can still wrap and
/// unwrap).
///
/// # Errors
///
/// [`SmokeError::Failed`] when the opened bytes differ from the sealed ones.
pub async fn kms_round_trip(p: &Ports) -> Result<(), SmokeError> {
    let user = UserId::new(p.rng.uuid_v4());
    let wrapped = p.keys.new_user_key(&user).await?;
    let plaintext = p.rng.bytes32();
    let aad = Aad {
        user,
        scope: SMOKE_SCOPE.to_owned(),
        field: SMOKE_FIELD,
    };
    let sealed = p.keys.seal(&user, &wrapped, &aad, &plaintext).await?;
    let opened = p.keys.open(&user, &wrapped, &aad, &sealed).await?;
    if opened != plaintext {
        return Err(SmokeError::Failed(KMS_CHECK));
    }
    Ok(())
}

/// Firestore writes a `users/{uuid}` document holding a wrapped key, reads it
/// back and deletes it again (S11 7.1: a failed run never leaves data behind).
///
/// # Errors
///
/// [`SmokeError::Failed`] when the document does not come back, or when the
/// cleanup delete fails after a successful read.
pub async fn firestore_round_trip(p: &Ports) -> Result<(), SmokeError> {
    let user = UserId::new(p.rng.uuid_v4());
    let wrapped = p.keys.new_user_key(&user).await?;
    let record = UserRecord {
        user_id: user,
        created_at: p.clock.now(),
        is_admin: false,
        wrapped_data_key: wrapped,
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    p.store
        .users()
        .put(&record, Precondition::MustNotExist)
        .await?;

    let read = p.store.users().get(&user).await?;
    let outcome = match read {
        Some(versioned) if versioned.record.user_id == user => Ok(()),
        _ => Err(SmokeError::Failed(FIRESTORE_CHECK)),
    };

    // The delete runs whether or not the read matched, so this check never
    // leaves the document behind.
    let deleted = p.store.users().delete(&user, Precondition::None).await;
    outcome?;
    deleted?;
    Ok(())
}

/// Cloud Tasks schedules a task ten minutes out and cancels it, expecting
/// `Cancelled` (S10 6.3: the real Cloud Tasks API is covered only here).
///
/// # Errors
///
/// [`SmokeError::Failed`] on any cancel outcome but `Cancelled`.
pub async fn tasks_schedule_and_cancel(p: &Ports, cfg: &SmokeConfig) -> Result<(), SmokeError> {
    if cfg.queue.trim().is_empty() {
        return Err(SmokeError::Invalid("MT_SMOKE_QUEUE"));
    }
    let job = JobId::new(p.rng.uuid_v4());
    let task = p
        .scheduler
        .schedule(&job, p.clock.now() + Duration::minutes(10))
        .await?;
    match p.scheduler.cancel(&task).await? {
        CancelOutcome::Cancelled => Ok(()),
        CancelOutcome::NotFound | CancelOutcome::AlreadyRunning => {
            Err(SmokeError::Failed(TASKS_CHECK))
        }
    }
}

/// UN-02 AC4 in staging: the deployed `unsub` refuses the metadata server as a
/// one-click target and raises Needs Attention.
///
/// A smoke user owns a mailbox (the runner re-checks ownership from the mailbox
/// record, `unsub` step 5) and two `queued` one-click jobs whose encrypted
/// targets are the metadata host name and the metadata IP literal. Both are
/// scheduled due now, then the job documents are polled every five seconds for
/// up to two minutes: each must end `NeedsAttention`, and a Needs Attention item
/// with reason `one_click_address_refused` must exist. Every document is deleted
/// afterwards, on failure too.
///
/// # Errors
///
/// [`SmokeError::Failed`] when a job does not reach `NeedsAttention`, when no
/// refusal item appears, or when the polling budget is exhausted.
#[allow(clippy::too_many_lines)]
pub async fn un_02_ac4_staging_unsub_refuses_metadata_target(
    p: &Ports,
    cfg: &SmokeConfig,
) -> Result<(), SmokeError> {
    if cfg.queue.trim().is_empty() {
        return Err(SmokeError::Invalid("MT_SMOKE_QUEUE"));
    }
    let user = UserId::new(p.rng.uuid_v4());
    let wrapped = p.keys.new_user_key(&user).await?;
    let now = p.clock.now();
    let tunables = Tunables::default();
    let mailbox = MailboxId::new(p.rng.uuid_v4());

    let user_record = UserRecord {
        user_id: user,
        created_at: now,
        is_admin: false,
        wrapped_data_key: wrapped.clone(),
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    let mailbox_record = MailboxRecord {
        mailbox_id: mailbox,
        user_id: user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new(format!("smoke-{}", user.0.simple()))?,
        email_address: seal_mailbox_field(
            p,
            &user,
            &wrapped,
            &mailbox,
            aad_fields::MAILBOX_EMAIL,
            "smoke@mailtinder.invalid",
        )
        .await?,
        status: MailboxStatus::Connected,
        linked_at: now,
        is_primary: true,
        refresh_token: None,
    };

    let mut jobs: Vec<JobId> = Vec::new();
    let outcome = async {
        p.store
            .users()
            .put(&user_record, Precondition::MustNotExist)
            .await?;
        p.store
            .mailboxes()
            .put(&mailbox_record, Precondition::MustNotExist)
            .await?;
        for (index, target) in [METADATA_HOST_TARGET, METADATA_IP_TARGET]
            .into_iter()
            .enumerate()
        {
            let job_id = JobId::new(p.rng.uuid_v4());
            let sealed = svc_common::job_record::seal_target(
                p,
                &user,
                &job_id,
                &Sensitive::new(target.to_owned()),
            )
            .await
            .map_err(|_| SmokeError::Failed(AC4_CHECK))?;
            let record = JobRecord {
                job_id,
                user_id: user,
                mailbox_id: mailbox,
                // A list of its own per job, so batching (UN-01 AC2) can never
                // absorb one of them.
                list_key_hash: ListKeyHash([u8::try_from(index).unwrap_or(0); 32]),
                method: JobMethod::OneClick,
                target: Some(sealed),
                sender_display: None,
                due_at: now,
                status: JobStatus::Queued,
                attempts: 0,
                outcome: None,
                expires_at: now + tunables.job_ttl,
            };
            p.store
                .jobs()
                .put(&record, Precondition::MustNotExist)
                .await?;
            p.scheduler.schedule(&job_id, now).await?;
            jobs.push(job_id);
        }
        await_refusal(p, &user, &jobs).await
    }
    .await;

    // `finally`: the same cleanup whether the checks above passed or failed.
    let cleaned = cleanup(p, &user, &mailbox, &jobs).await;
    outcome.and(cleaned)
}

/// The unauthenticated `unsub` and `worker` URLs must not answer 200 from the
/// public internet: 403 (Cloud Run IAM) or 404 (not routed) only.
///
/// # Errors
///
/// [`SmokeError::Failed`] when either service answers 200, or anything else.
pub async fn internal_services_not_public(cfg: &SmokeConfig) -> Result<(), SmokeError> {
    let client = http_client()?;
    for base in [&cfg.unsub_url, &cfg.worker_url] {
        let response = client.get(base.clone()).send().await?;
        let status = response.status().as_u16();
        if !matches!(status, 403 | 404) {
            return Err(SmokeError::Failed(INTERNAL_CHECK));
        }
    }
    Ok(())
}

/// The hosted app and the API behind it carry the headers `firebase.json` and
/// the session route guarantee: the app's security headers, and a session
/// response that is uncacheable and sets a `__session` cookie with the exact
/// attributes. No redirect is ever followed.
///
/// # Errors
///
/// [`SmokeError::Failed`] when a header is missing or wrong, or when the
/// session response is not 200.
pub async fn hosting_and_api_headers(cfg: &SmokeConfig) -> Result<(), SmokeError> {
    let client = http_client()?;

    let hosting = client.get(cfg.app_url.clone()).send().await?;
    check_hosting_headers(hosting.headers())?;

    let mut session_url = cfg.app_url.clone();
    session_url.set_path("/api/v1/session");
    let session = client.get(session_url).send().await?;
    check_session_response(session.status().as_u16(), session.headers())?;
    Ok(())
}

/// The app's header set, checked against whatever response carried it.
///
/// # Errors
///
/// [`SmokeError::Failed`] when any required header is absent or wrong.
pub fn check_hosting_headers(headers: &HeaderMap) -> Result<(), SmokeError> {
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(headers_failed)?;
    if !csp.contains("require-trusted-types-for") {
        return Err(headers_failed());
    }
    if headers.get(header::STRICT_TRANSPORT_SECURITY).is_none() {
        return Err(headers_failed());
    }
    let nosniff = headers
        .get(header::X_CONTENT_TYPE_OPTIONS)
        .and_then(|value| value.to_str().ok());
    if !nosniff.is_some_and(|value| value.eq_ignore_ascii_case("nosniff")) {
        return Err(headers_failed());
    }
    if headers.get(header::REFERRER_POLICY).is_none() {
        return Err(headers_failed());
    }
    Ok(())
}

/// `GET /api/v1/session`: 200, `Cache-Control: no-store`, and a `Set-Cookie`
/// for `__session` with `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/` and no
/// `Domain`.
///
/// # Errors
///
/// [`SmokeError::Failed`] for any of the above.
pub fn check_session_response(status: u16, headers: &HeaderMap) -> Result<(), SmokeError> {
    if status != 200 {
        return Err(headers_failed());
    }
    let no_store = headers
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"));
    if !no_store {
        return Err(headers_failed());
    }

    let cookie = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("__session="))
        .ok_or_else(headers_failed)?;
    let cookie = cookie.to_ascii_lowercase();
    for attribute in ["secure", "httponly", "samesite=lax", "path=/"] {
        if !cookie.contains(attribute) {
            return Err(headers_failed());
        }
    }
    if cookie.contains("domain=") {
        return Err(headers_failed());
    }
    Ok(())
}

/// The header check's one failure value.
fn headers_failed() -> SmokeError {
    SmokeError::Failed(HEADERS_CHECK)
}

/// Poll until every job is `NeedsAttention` and one refusal item per job
/// exists, or the budget runs out.
async fn await_refusal(p: &Ports, user: &UserId, jobs: &[JobId]) -> Result<(), SmokeError> {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        if refusal_observed(p, user, jobs).await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(SmokeError::Failed(AC4_CHECK));
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// True when every job ended `NeedsAttention` and needs_attention holds at
/// least one `one_click_address_refused` item per job.
async fn refusal_observed(p: &Ports, user: &UserId, jobs: &[JobId]) -> Result<bool, SmokeError> {
    for job in jobs {
        match p.store.jobs().get(job).await? {
            Some(versioned) if versioned.record.status == JobStatus::NeedsAttention => {}
            _ => return Ok(false),
        }
    }
    let page = p
        .store
        .needs_attention()
        .by_user(
            user,
            PageRequest {
                limit: MAX_PAGE,
                after: None,
            },
        )
        .await?;
    let refused = page
        .items
        .iter()
        .filter(|item| item.record.reason_code == NeedsAttentionReason::OneClickAddressRefused)
        .count();
    Ok(refused >= jobs.len())
}

/// Delete every document the UN-02 AC4 check created, in dependency order.
async fn cleanup(
    p: &Ports,
    user: &UserId,
    mailbox: &MailboxId,
    jobs: &[JobId],
) -> Result<(), SmokeError> {
    let items = p
        .store
        .needs_attention()
        .by_user(
            user,
            PageRequest {
                limit: MAX_PAGE,
                after: None,
            },
        )
        .await?;
    for item in &items.items {
        p.store
            .needs_attention()
            .delete(&item.record.item_id, Precondition::None)
            .await?;
    }
    for job in jobs {
        p.store.jobs().delete(job, Precondition::None).await?;
    }
    p.store
        .mailboxes()
        .delete(mailbox, Precondition::None)
        .await?;
    p.store.users().delete(user, Precondition::None).await?;
    Ok(())
}

/// Seal one field under a mailbox's scope (`scope = mailbox_id`).
async fn seal_mailbox_field(
    p: &Ports,
    user: &UserId,
    wrapped: &WrappedKey,
    mailbox: &MailboxId,
    field: &'static str,
    value: &str,
) -> Result<Ciphertext, SmokeError> {
    let aad = Aad {
        user: *user,
        scope: mailbox.to_string(),
        field,
    };
    Ok(Ciphertext(
        p.keys.seal(user, wrapped, &aad, value.as_bytes()).await?,
    ))
}

/// The plain HTTP client for the public endpoints: no redirects (S10 6.2 a
/// one-click 3xx is a failure, and these checks must see the first response),
/// rustls, a short timeout.
fn http_client() -> Result<reqwest::Client, SmokeError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| SmokeError::Failed(HEADERS_CHECK))
}

/// A required, non-empty `MT_SMOKE_*` variable.
fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, SmokeError> {
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(SmokeError::Missing(name))
}

/// A required URL variable.
fn url_of(lookup: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<Url, SmokeError> {
    let raw = required(lookup, name)?;
    Url::parse(&raw).map_err(|_| SmokeError::Invalid(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    /// A complete `MT_SMOKE_*` lookup, with `missing` removed.
    fn lookup(missing: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        let values = [
            ("MT_SMOKE_PROJECT", "staging-project"),
            ("MT_SMOKE_REGION", "us-central1"),
            (
                "MT_SMOKE_KMS_KEY",
                "projects/p/locations/r/keyRings/k/cryptoKeys/data-key-kek",
            ),
            (
                "MT_SMOKE_QUEUE",
                "projects/p/locations/r/queues/unsubscribe",
            ),
            ("MT_SMOKE_UNSUB_URL", "https://unsub-x.run.app"),
            ("MT_SMOKE_WORKER_URL", "https://worker-x.run.app"),
            ("MT_SMOKE_APP_URL", "https://staging.web.app"),
            (
                "MT_SMOKE_TASKS_INVOKER",
                "mt-tasks-invoker@p.iam.gserviceaccount.com",
            ),
        ];
        move |name: &str| {
            if Some(name) == missing {
                return None;
            }
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// A missing variable is an error that names the variable and nothing else.
    #[test]
    fn smoke_config_missing_variable_is_an_error() {
        let result = SmokeConfig::from_lookup(lookup(Some("MT_SMOKE_APP_URL")));
        match result {
            Err(SmokeError::Missing(name)) => assert_eq!(name, "MT_SMOKE_APP_URL"),
            other => panic!("expected Missing(MT_SMOKE_APP_URL), got {other:?}"),
        }

        let blank = SmokeConfig::from_lookup(|name| {
            if name == "MT_SMOKE_PROJECT" {
                Some("   ".to_owned())
            } else {
                lookup(None)(name)
            }
        });
        match blank {
            Err(SmokeError::Missing(name)) => assert_eq!(name, "MT_SMOKE_PROJECT"),
            other => panic!("expected Missing(MT_SMOKE_PROJECT), got {other:?}"),
        }
    }

    /// A malformed URL is `Invalid`, named by its variable.
    #[test]
    fn smoke_config_rejects_a_malformed_url() {
        let result = SmokeConfig::from_lookup(|name| {
            if name == "MT_SMOKE_UNSUB_URL" {
                Some("not a url".to_owned())
            } else {
                lookup(None)(name)
            }
        });
        match result {
            Err(SmokeError::Invalid(name)) => assert_eq!(name, "MT_SMOKE_UNSUB_URL"),
            other => panic!("expected Invalid(MT_SMOKE_UNSUB_URL), got {other:?}"),
        }
    }

    /// Every variable lands in its field, and the URLs are parsed.
    #[test]
    fn smoke_config_reads_every_variable() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = SmokeConfig::from_lookup(lookup(None))?;
        assert_eq!(cfg.project, "staging-project");
        assert_eq!(cfg.region, "us-central1");
        assert_eq!(
            cfg.tasks_invoker,
            "mt-tasks-invoker@p.iam.gserviceaccount.com"
        );
        assert_eq!(cfg.app_url.as_str(), "https://staging.web.app/");
        Ok(())
    }

    /// The header set `firebase.json` deploys is accepted.
    #[test]
    fn hosting_headers_accept_the_deployed_set() -> Result<(), Box<dyn std::error::Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; require-trusted-types-for 'script'; trusted-types flutter-js",
            ),
        );
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        check_hosting_headers(&headers)?;
        Ok(())
    }

    /// A CSP without `require-trusted-types-for` is refused, as is a missing
    /// `nosniff`.
    #[test]
    fn hosting_headers_refuse_a_weakened_set() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'"),
        );
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        assert!(matches!(
            check_hosting_headers(&headers),
            Err(SmokeError::Failed(_))
        ));

        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("require-trusted-types-for 'script'"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("sniff"),
        );
        assert!(matches!(
            check_hosting_headers(&headers),
            Err(SmokeError::Failed(_))
        ));
    }

    /// The session response the API sends is accepted.
    #[test]
    fn session_response_accepts_the_deployed_set() -> Result<(), Box<dyn std::error::Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static("__session=abc; Path=/; Secure; HttpOnly; SameSite=Lax"),
        );
        check_session_response(200, &headers)?;
        Ok(())
    }

    /// A cacheable body, a non-200, and a `Domain` attribute are each refused.
    #[test]
    fn session_response_refuses_a_weakened_set() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        );
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static("__session=abc; Path=/; Secure; HttpOnly; SameSite=Lax"),
        );
        assert!(check_session_response(200, &headers).is_err());
        assert!(check_session_response(500, &headers).is_err());

        let mut headers = HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "__session=abc; Domain=example.com; Path=/; Secure; HttpOnly; SameSite=Lax",
            ),
        );
        assert!(check_session_response(200, &headers).is_err());

        let mut headers = HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static("__session=abc; Path=/; HttpOnly; SameSite=Lax"),
        );
        assert!(check_session_response(200, &headers).is_err());
    }

    /// The unused port really refuses, so a check that reached it fails.
    #[tokio::test]
    async fn unused_ports_refuse() -> Result<(), Box<dyn std::error::Error>> {
        let inert = Unused;
        assert!(ports::Secrets::get(&inert, ports::SecretName::JevApiKey)
            .await
            .is_err());
        let target = Url::parse("https://example.invalid/")?;
        assert!(ports::HttpEgress::one_click_post(&inert, &target)
            .await
            .is_err());
        Ok(())
    }

    // ---- the staging checks themselves, driven against the fakes ------------
    //
    // The checks above are the ones that need a deployed platform. The checks
    // below are the same functions on `testkit`'s in-memory fakes: they prove
    // the wiring, the failure paths and the cleanup run as written, and they
    // are what keeps this crate above the coverage floor (S10 10.2). A floor is
    // never lowered and a crate is never excluded to get green (T-004, S13), so
    // the crate is covered here instead. `testkit` is a dev-dependency only: the
    // shipped binary links no test code, and the proof of UN-02 AC4 stays the
    // staging run.

    /// A URL the tests can rely on being valid.
    fn test_url(raw: &str) -> Url {
        Url::parse(raw).unwrap_or_else(|_| panic!("test url is valid"))
    }

    /// A `SmokeConfig` for the tests, with the queue inside its own region.
    fn smoke_config() -> SmokeConfig {
        SmokeConfig {
            project: "staging-project".to_owned(),
            region: "us-central1".to_owned(),
            kms_key: "projects/p/locations/us-central1/keyRings/k/cryptoKeys/data-key-kek"
                .to_owned(),
            queue: "projects/p/locations/us-central1/queues/unsubscribe".to_owned(),
            unsub_url: test_url("https://unsub-x.run.app/"),
            worker_url: test_url("https://worker-x.run.app/"),
            app_url: test_url("https://staging.web.app/"),
            tasks_invoker: "mt-tasks-invoker@p.iam.gserviceaccount.com".to_owned(),
        }
    }

    /// The hand-written `Debug` redacts the key resource name and the URLs, so
    /// a stray `{:?}` cannot leak one (the privacy rules refuse a derived
    /// `Debug` on a struct with `…_key` and `…_url` fields).
    #[test]
    fn smoke_config_debug_redacts_the_key_and_the_urls() {
        let rendered = format!("{:?}", smoke_config());
        assert!(rendered.contains("staging-project"));
        assert!(rendered.contains("us-central1"));
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("run.app"));
        assert!(!rendered.contains("data-key-kek"));
    }

    /// A queue outside the configured region is refused, and one inside it
    /// builds the real adapters (construction makes no network call).
    #[test]
    fn build_ports_requires_the_queue_in_the_configured_region() {
        assert!(build_ports(&smoke_config()).is_ok());

        let mut elsewhere = smoke_config();
        elsewhere.queue = "projects/p/locations/us-east1/queues/unsubscribe".to_owned();
        assert!(matches!(
            build_ports(&elsewhere),
            Err(SmokeError::Invalid("MT_SMOKE_QUEUE"))
        ));
    }

    /// KMS wraps a fresh user key and opens what it sealed.
    #[tokio::test]
    async fn kms_round_trip_opens_what_it_sealed() -> Result<(), Box<dyn std::error::Error>> {
        let (ports, _) = testkit::fake_ports();
        kms_round_trip(&ports).await?;
        Ok(())
    }

    /// Firestore's document comes back and is deleted again.
    #[tokio::test]
    async fn firestore_round_trip_leaves_no_document_behind(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (ports, fakes) = testkit::fake_ports();
        firestore_round_trip(&ports).await?;
        assert!(
            fakes.store.export_json().is_empty(),
            "the check left a document behind"
        );
        Ok(())
    }

    /// Cloud Tasks' task is scheduled and cancelled, and a blank queue is an
    /// error rather than a silent pass.
    #[tokio::test]
    async fn tasks_schedule_and_cancel_cancels_the_task() -> Result<(), Box<dyn std::error::Error>>
    {
        let (ports, fakes) = testkit::fake_ports();
        tasks_schedule_and_cancel(&ports, &smoke_config()).await?;
        assert_eq!(
            fakes.scheduler.events().len(),
            2,
            "one schedule and one cancel"
        );

        let mut blank = smoke_config();
        blank.queue = String::new();
        assert!(matches!(
            tasks_schedule_and_cancel(&ports, &blank).await,
            Err(SmokeError::Invalid("MT_SMOKE_QUEUE"))
        ));
        Ok(())
    }

    /// A scheduler double that delivers a job the way the deployed `unsub` does
    /// when it refuses a metadata target: the job flips to `NeedsAttention` and
    /// one refusal item appears. Without it the check below would poll for two
    /// minutes and never turn.
    struct RefusingScheduler {
        store: Arc<dyn ports::ServerStore>,
    }

    #[async_trait::async_trait]
    impl ports::JobScheduler for RefusingScheduler {
        async fn schedule(
            &self,
            job: &JobId,
            due_at: time::OffsetDateTime,
        ) -> Result<ports::TaskName, ports::SchedError> {
            let mut record = self
                .store
                .jobs()
                .get(job)
                .await
                .map_err(|_| ports::SchedError::Unavailable)?
                .ok_or(ports::SchedError::Unavailable)?
                .record;
            record.status = JobStatus::NeedsAttention;
            self.store
                .jobs()
                .put(&record, Precondition::None)
                .await
                .map_err(|_| ports::SchedError::Unavailable)?;
            let item = ports::NeedsAttentionRecord {
                item_id: ports::NeedsAttentionId(job.0),
                user_id: record.user_id,
                mailbox_id: record.mailbox_id,
                sender_display: Ciphertext(Vec::new()),
                link: None,
                reason_code: NeedsAttentionReason::OneClickAddressRefused,
                created_at: due_at,
                expires_at: due_at + Duration::hours(24),
            };
            self.store
                .needs_attention()
                .put(&item, Precondition::MustNotExist)
                .await
                .map_err(|_| ports::SchedError::Unavailable)?;
            Ok(ports::TaskName::for_job(job))
        }

        async fn cancel(
            &self,
            _task: &ports::TaskName,
        ) -> Result<CancelOutcome, ports::SchedError> {
            Ok(CancelOutcome::NotFound)
        }
    }

    /// UN-02 AC4's staging check, driven end to end: both metadata targets end
    /// `NeedsAttention` with a refusal item, and every document the check
    /// created is deleted again.
    #[tokio::test]
    async fn un_02_ac4_staging_unsub_refuses_metadata_target_against_fakes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (mut ports, fakes) = testkit::fake_ports();
        let store = Arc::clone(&fakes.store) as Arc<dyn ports::ServerStore>;
        ports.scheduler = Arc::new(RefusingScheduler { store });

        un_02_ac4_staging_unsub_refuses_metadata_target(&ports, &smoke_config()).await?;

        assert!(
            fakes.store.export_json().is_empty(),
            "the check left documents behind"
        );
        Ok(())
    }
}
