//! e2e-mode wiring (T-500b), compiled only with the `testkit` feature.
//!
//! Switched on at run time by `MT_E2E=1`. Storage is the Firestore emulator;
//! keys, system keys, secrets, the scheduler and the caller verifier stay the
//! `testkit` fakes; Google (OAuth, Gmail, Drive) is the local `fake-google`
//! server, reached through [`LoopbackEgress`], which allows exactly that one
//! socket. The production egress policy is never weakened (T-306); e2e simply
//! does not use it.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use adapters_gcp::{FirestoreConfig, FirestoreStore, GcpHttp, StaticTokenSource, TokenSource};
use adapters_gmail::identity::{GoogleIdentity, GoogleIdentityConfig};
use adapters_gmail::{DriveAppFolder, GmailHttp, GmailProvider};
use async_trait::async_trait;
use domain::Tunables;
use obs::Sensitive;
use ports::{
    AppFolderStore, Clock, EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod,
    IdentityProvider, InviteMailer, JobScheduler, MailProvider, OneClickOutcome, Ports, Rng,
    SecretName, ServerStore,
};
use url::Url;

use crate::config::{ApiConfig, Mode};
use crate::local_runner::{LocalJobRunner, LocalJobRunnerConfig};
use crate::startup::SetupError;

/// The literal client secret for e2e. Never a real secret (S10 3.3).
const CLIENT_SECRET: &str = "test-only-not-a-secret";
/// The Firestore emulator project id.
const E2E_PROJECT: &str = "demo-mailtinder";
/// The maximum response body [`LoopbackEgress`] will accept, 1 MiB.
const MAX_BODY: usize = 1024 * 1024;

/// Build e2e ports from the process environment.
///
/// # Errors
///
/// Returns [`SetupError`] when a required variable is missing or malformed.
pub fn build_e2e_from_env() -> Result<(Ports, ApiConfig), SetupError> {
    let store = firestore_emulator_store(|name| std::env::var(name).ok())?;
    build_e2e_ports(store, |name| std::env::var(name).ok())
}

/// Build the e2e Firestore store over the emulator, with a fixed `owner`
/// token (the emulator ignores it).
///
/// # Errors
///
/// Returns [`SetupError`] when `FIRESTORE_EMULATOR_HOST` is missing, is not a
/// `host:port`, or the HTTP client cannot be built.
pub fn firestore_emulator_store(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Arc<FirestoreStore>, SetupError> {
    let host = required(&lookup, "FIRESTORE_EMULATOR_HOST")?;
    let addr: SocketAddr = host
        .parse()
        .map_err(|_| SetupError::Invalid("FIRESTORE_EMULATOR_HOST"))?;
    let tokens: Arc<dyn TokenSource> =
        Arc::new(StaticTokenSource(Sensitive::new("owner".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).map_err(|_| SetupError::Adapter)?);
    Ok(Arc::new(FirestoreStore::new(
        http,
        FirestoreConfig::new(E2E_PROJECT),
    )))
}

/// Build the e2e `Ports` and `ApiConfig`. Generic over the store so tests can
/// pass an `InMemoryServerStore`; production e2e passes the emulator store.
///
/// Required variables: `FAKE_GOOGLE_URL`, `APP_ORIGIN` (loopback `http` is
/// allowed here and nowhere else) and `GOOGLE_OAUTH_CLIENT_ID`.
///
/// # Errors
///
/// Returns [`SetupError`] for a missing/invalid variable or adapter.
pub fn build_e2e_ports<S>(
    store: Arc<S>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(Ports, ApiConfig), SetupError>
where
    S: ServerStore + 'static,
{
    let base = ApiConfig::from_lookup(&lookup, Mode::E2e)?;
    let fake_google_raw = required(&lookup, "FAKE_GOOGLE_URL")?;
    let fake_google =
        Url::parse(&fake_google_raw).map_err(|_| SetupError::Invalid("FAKE_GOOGLE_URL"))?;
    // The browser-facing origin, set only in phone mode (Behaviour 0c). Unset,
    // the authorisation URL is the loopback fake-google exactly as before.
    let public_base = public_base_url(&lookup)?;

    let (mut ports, fakes) = testkit::fake_ports();
    // Standalone services share real time: fake-google checks provider-token
    // expiry against it, and OAuth claims must agree with the API clock.
    ports.clock = adapters_gcp::production_clock();
    let clock = Arc::clone(&ports.clock);
    let egress: Arc<dyn HttpEgress> = Arc::new(LoopbackEgress::new(&fake_google)?);

    // Random 32-byte HMAC keys from the injected `Rng`; the client secret is
    // the literal e2e value.
    let rate_key = random_key(&ports.rng);
    let email_key = random_key(&ports.rng);
    fakes.secrets.set(
        SecretName::GoogleOAuthClientSecret,
        CLIENT_SECRET.as_bytes(),
    );
    fakes
        .secrets
        .set(SecretName::LogPseudonymHmacKey, &rate_key);
    fakes
        .secrets
        .set(SecretName::EmailLookupHmacKey, &email_key);

    let identity: Arc<dyn IdentityProvider> = Arc::new(GoogleIdentity::new(
        GoogleIdentityConfig {
            client_id: base.google_client_id.clone(),
            client_secret: Sensitive::new(CLIENT_SECRET.to_owned()),
            auth_endpoint: public_auth_endpoint(&fake_google, public_base.as_ref())?,
            token_endpoint: join(&fake_google, "token")?,
            revoke_endpoint: join(&fake_google, "revoke")?,
            jwks_uri: join(&fake_google, "oauth2/v3/certs")?,
        },
        Arc::clone(&egress),
        Arc::clone(&clock),
    ));

    let gmail_client = GmailHttp::new(
        Arc::clone(&egress),
        join(&fake_google, "gmail/v1/users/me")?,
        Arc::clone(&clock),
    );
    let drive_client = GmailHttp::new(Arc::clone(&egress), fake_google.clone(), Arc::clone(&clock));
    let gmail = Arc::new(GmailProvider::new(gmail_client));
    let app_folder: Arc<dyn AppFolderStore> = Arc::new(DriveAppFolder::new(drive_client));

    let store: Arc<dyn ServerStore> = store;
    ports.store = store;
    ports.egress = egress;
    ports.identity = identity;
    ports.gmail = Arc::clone(&gmail) as Arc<dyn MailProvider>;
    ports.invite_mailer = Arc::clone(&gmail) as Arc<dyn InviteMailer>;
    ports.app_folder = app_folder;

    // T-1112c: when e2e is pointed at a loopback `unsub` (both `UNSUB_BASE_URL`
    // and `UNSUB_E2E_CALLER_TOKEN` set), deliver due jobs with the local runner
    // instead of the fake scheduler; otherwise behaviour is exactly as before.
    if let Some(runner) = e2e_local_runner(&lookup, Arc::clone(&ports.clock))? {
        let running = Arc::clone(&runner);
        tokio::spawn(async move { running.run().await });
        ports.scheduler = runner as Arc<dyn JobScheduler>;
    }

    Ok((
        ports,
        base.with_keys(Sensitive::new(rate_key), Sensitive::new(email_key)),
    ))
}

/// The local job runner to use in e2e (T-1112c), or `None` to keep the testkit
/// fake scheduler.
///
/// The runner is built only when both `UNSUB_BASE_URL` and
/// `UNSUB_E2E_CALLER_TOKEN` are set to non-blank values, so with either unset
/// behaviour is exactly today's (Behaviour 1). `clock` is the api's own clock,
/// so the runner shares the api's notion of time; `MT_E2E_TIME_SCALE` scales
/// the waits it owns (default `1.0`).
///
/// # Errors
///
/// Returns [`SetupError::Invalid`] when `UNSUB_BASE_URL` is not a URL, or its
/// host is not a loopback IP literal (the runner refuses anything else).
pub fn e2e_local_runner(
    lookup: &impl Fn(&str) -> Option<String>,
    clock: Arc<dyn Clock>,
) -> Result<Option<Arc<LocalJobRunner>>, SetupError> {
    let (Some(base), Some(caller_token)) = (
        nonblank(lookup, "UNSUB_BASE_URL"),
        nonblank(lookup, "UNSUB_E2E_CALLER_TOKEN"),
    ) else {
        return Ok(None);
    };
    let unsub_base = Url::parse(&base).map_err(|_| SetupError::Invalid("UNSUB_BASE_URL"))?;
    let time_scale = lookup("MT_E2E_TIME_SCALE")
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .unwrap_or(1.0);
    let runner = LocalJobRunner::new(
        LocalJobRunnerConfig {
            unsub_base,
            caller_token,
            time_scale,
        },
        clock,
    )
    .map_err(|_| SetupError::Invalid("UNSUB_BASE_URL"))?;
    Ok(Some(Arc::new(runner)))
}

/// A non-blank variable from `lookup`, if present.
fn nonblank(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name).filter(|value| !value.trim().is_empty())
}

/// The e2e tunables (T-1112b). The production defaults, except that when
/// `MT_E2E_UNSUB_DELAY_S` is set to a whole number of seconds it replaces
/// `unsub_delay`, so a demo's undo window is seconds rather than the production
/// five minutes. Unset, blank or unparsable keeps the production value.
#[must_use]
pub fn e2e_tunables(lookup: impl Fn(&str) -> Option<String>) -> Tunables {
    let mut tunables = Tunables::default();
    if let Some(secs) =
        lookup("MT_E2E_UNSUB_DELAY_S").and_then(|raw| raw.trim().parse::<u64>().ok())
    {
        tunables.unsub_delay = Duration::from_secs(secs);
    }
    tunables
}

/// A loopback-only [`HttpEgress`]: it allows requests to exactly one socket,
/// the configured `fake-google` address, and refuses everything else before
/// any connection is opened.
pub struct LoopbackEgress {
    socket: SocketAddr,
    client: reqwest::Client,
}

impl LoopbackEgress {
    /// Build an egress pinned to the socket of `base`. The host must be a
    /// loopback literal: IPv4 `127.0.0.0/8` or IPv6 `::1` (the bracketed
    /// `[::1]` URL form included).
    ///
    /// # Errors
    ///
    /// Returns [`SetupError::Invalid`] when `base` has no literal loopback IP
    /// host or port, and [`SetupError::Adapter`] if the client cannot be built.
    pub fn new(base: &Url) -> Result<Self, SetupError> {
        let host = base
            .host_str()
            .ok_or(SetupError::Invalid("FAKE_GOOGLE_URL"))?;
        let ip = host_ip(host).ok_or(SetupError::Invalid("FAKE_GOOGLE_URL"))?;
        if !ip.is_loopback() {
            return Err(SetupError::Invalid("FAKE_GOOGLE_URL"));
        }
        let port = base
            .port_or_known_default()
            .ok_or(SetupError::Invalid("FAKE_GOOGLE_URL"))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| SetupError::Adapter)?;
        Ok(Self {
            socket: SocketAddr::new(ip, port),
            client,
        })
    }

    /// True only when the request targets exactly the allowed socket.
    fn allowed(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let Some(ip) = host_ip(host) else {
            return false;
        };
        let Some(port) = url.port_or_known_default() else {
            return false;
        };
        SocketAddr::new(ip, port) == self.socket
    }
}

#[async_trait]
impl HttpEgress for LoopbackEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        // The api service never sends one-click POSTs (`Service::Api`).
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        if !self.allowed(&req.url) {
            return Err(EgressError::HostNotAllowed);
        }
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut builder = self
            .client
            .request(method, req.url.clone())
            .timeout(req.timeout);
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.expose());
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let response = builder.send().await.map_err(|_| EgressError::Connect)?;
        let status = response.status().as_u16();
        let mut headers = Vec::new();
        for (name, value) in response.headers() {
            if let Ok(text) = value.to_str() {
                headers.push((name.as_str().to_owned(), text.to_owned()));
            }
        }
        if let Some(len) = response.content_length() {
            if len > MAX_BODY as u64 {
                return Err(EgressError::ResponseTooLarge);
            }
        }
        let body = response
            .bytes()
            .await
            .map_err(|_| EgressError::Connect)?
            .to_vec();
        if body.len() > MAX_BODY {
            return Err(EgressError::ResponseTooLarge);
        }
        Ok(EgressResponse {
            status,
            headers,
            body,
        })
    }
}

/// Parse a URL host as an IP literal, unwrapping the bracketed IPv6 form
/// (`[::1]` → `::1`).
fn host_ip(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

/// A required, non-empty e2e variable. Only the name is ever surfaced.
fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, SetupError> {
    lookup(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or(SetupError::Missing(name))
}

/// Join a `fake-google` path onto its base URL.
fn join(base: &Url, path: &str) -> Result<Url, SetupError> {
    base.join(path)
        .map_err(|_| SetupError::Invalid("FAKE_GOOGLE_URL"))
}

/// The optional browser-facing base URL (`MT_PUBLIC_BASE_URL`).
///
/// In `demo.sh --phone` mode the laptop's loopback is unreachable from the
/// phone, so the authorisation page the *browser* is sent to lives behind the
/// tunnel; this is that public origin. Unset, blank or absent keeps today's
/// behaviour. Server-to-server calls never use it: they keep `FAKE_GOOGLE_URL`.
///
/// # Errors
///
/// Returns [`SetupError::Invalid`] when the variable is set but is not a URL.
pub fn public_base_url(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Url>, SetupError> {
    match lookup("MT_PUBLIC_BASE_URL").filter(|value| !value.trim().is_empty()) {
        Some(raw) => Url::parse(raw.trim())
            .map(Some)
            .map_err(|_| SetupError::Invalid("MT_PUBLIC_BASE_URL")),
        None => Ok(None),
    }
}

/// The browser-facing authorisation endpoint (Behaviour 0c).
///
/// With a public base URL set, the browser is redirected to the tunnel's
/// `/fake-google/o/oauth2/v2/auth` (the one path `e2e_host.py` proxies to the
/// loopback fake-google); without one it is the loopback `fake-google`,
/// exactly as before.
///
/// # Errors
///
/// Returns [`SetupError::Invalid`] when the join fails.
pub fn public_auth_endpoint(
    fake_google: &Url,
    public_base: Option<&Url>,
) -> Result<Url, SetupError> {
    match public_base {
        Some(base) => base
            .join("fake-google/o/oauth2/v2/auth")
            .map_err(|_| SetupError::Invalid("MT_PUBLIC_BASE_URL")),
        None => join(fake_google, "o/oauth2/v2/auth"),
    }
}

/// A random 32-byte HMAC key from the injected `Rng`.
fn random_key(rng: &Arc<dyn Rng>) -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(rng.uuid_v4().as_bytes());
    key.extend_from_slice(rng.uuid_v4().as_bytes());
    key
}
