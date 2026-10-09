//! e2e-mode wiring (T-1112a), compiled only with the `testkit` feature.
//!
//! Switched on at run time by `MT_E2E=1`. Storage is the Firestore emulator;
//! keys and secrets stay the `testkit` fakes; Gmail is the local `fake-google`
//! server reached through [`LoopbackEgress`], which allows exactly the
//! `fake-google` and unsubscribe-testbed sockets. The caller verifier accepts
//! exactly one bearer value, read from `UNSUB_E2E_CALLER_TOKEN`; a missing,
//! empty or short token refuses the start. The production egress policy is
//! never weakened (T-306); e2e simply does not use it.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use adapters_gcp::{FirestoreConfig, FirestoreStore, GcpHttp, StaticTokenSource, TokenSource};
use adapters_gmail::{GmailHttp, GmailProvider};
use async_trait::async_trait;
use obs::Sensitive;
use ports::{
    CallerAuthError, CallerVerifier, EgressError, EgressRequest, EgressResponse, HttpEgress,
    HttpMethod, InviteMailer, MailProvider, OneClickOutcome, SecretName, ServerStore,
    VerifiedCaller,
};
use svc_common::internal_auth::InternalAuthConfig;
use url::Url;

use crate::config::DEFAULT_PORT;
use crate::mailto::MailtoSender;
use crate::one_click::OneClickSender;
use crate::runner::UnsubState;
use crate::sender::UnsubSender;

/// The fixed e2e audience; never a production value.
pub const E2E_AUDIENCE: &str = "unsub-e2e";
/// The fixed e2e caller identity the verifier reports. Not a real address.
pub const E2E_CALLER_EMAIL: &str = "e2e-caller@mailtinder.invalid";
/// The variable holding the one accepted bearer value.
pub const E2E_CALLER_TOKEN_ENV: &str = "UNSUB_E2E_CALLER_TOKEN";
/// The shortest accepted token, in bytes. A short token is refused at start.
pub const MIN_TOKEN_LEN: usize = 16;
/// The unauthenticated liveness route mounted only in e2e mode.
pub const E2E_HEALTH_PATH: &str = "/healthz";
/// The Firestore emulator project id.
const E2E_PROJECT: &str = "demo-mailtinder";
/// The maximum response body [`LoopbackEgress`] will accept, 1 MiB.
const MAX_BODY: usize = 1024 * 1024;
/// The literal client secret for e2e. Never a real secret (S10 3.3).
const CLIENT_SECRET: &str = "test-only-not-a-secret";

/// An e2e start-up failure. Nothing is printed through the print macros; the
/// process logs one line through `tracing` and exits non-zero.
#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    /// A required environment variable was absent or blank.
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    /// A present but malformed or too-weak variable.
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    /// An adapter could not be built.
    #[error("adapter")]
    Adapter,
    /// Observability could not be initialised.
    #[error("obs")]
    Obs,
}

/// Where the e2e service listens. Loopback only: the value is a constant, so
/// no configuration can widen it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct E2eConfig {
    /// The listen address, always `127.0.0.1:<port>`.
    pub bind: SocketAddr,
}

/// True when the process should start in e2e mode. False unless the crate was
/// built with the `testkit` feature *and* `MT_E2E=1` is set (S10 3.3).
#[cfg(feature = "testkit")]
#[must_use]
pub fn e2e_mode_enabled() -> bool {
    std::env::var("MT_E2E").is_ok_and(|v| v == "1")
}

/// Build e2e state from the process environment.
///
/// # Errors
///
/// Returns [`E2eError`] when a required variable is missing, malformed, a
/// token is too short, or an adapter cannot be built.
pub async fn run_e2e() -> Result<(), E2eError> {
    let (state, config) = build_e2e_from_env()?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|_| E2eError::Adapter)?;
    tracing::info!(event = "op", route = "unsub.startup", outcome = "success");
    axum::serve(listener, e2e_router(state))
        .await
        .map_err(|_| E2eError::Adapter)?;
    Ok(())
}

/// Build the e2e Firestore store over the emulator, with a fixed `owner`
/// token (the emulator ignores it).
///
/// # Errors
///
/// Returns [`E2eError`] when `FIRESTORE_EMULATOR_HOST` is missing, is not a
/// `host:port`, or the HTTP client cannot be built.
pub fn firestore_emulator_store(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Arc<FirestoreStore>, E2eError> {
    let host = required(&lookup, "FIRESTORE_EMULATOR_HOST")?;
    let addr: SocketAddr = host
        .parse()
        .map_err(|_| E2eError::Invalid("FIRESTORE_EMULATOR_HOST"))?;
    let tokens: Arc<dyn TokenSource> =
        Arc::new(StaticTokenSource(Sensitive::new("owner".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).map_err(|_| E2eError::Adapter)?);
    Ok(Arc::new(FirestoreStore::new(
        http,
        FirestoreConfig::new(E2E_PROJECT),
    )))
}

/// Build the e2e [`UnsubState`] and [`E2eConfig`] from a variable lookup.
/// Generic over the store so tests can pass an `InMemoryServerStore`;
/// production e2e passes the emulator store.
///
/// Required variables: `UNSUB_E2E_CALLER_TOKEN` (>= 16 bytes), `FAKE_GOOGLE_URL`
/// and `UNSUB_TESTBED_URL` (both literal loopback URLs).
///
/// # Errors
///
/// Returns [`E2eError`] for a missing/invalid variable or adapter.
pub fn build_e2e_ports<S>(
    store: Arc<S>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(UnsubState, E2eConfig), E2eError>
where
    S: ServerStore + 'static,
{
    let token = accepted_token(&lookup)?;
    let fake_google = url_from(&lookup, "FAKE_GOOGLE_URL")?;
    let testbed = url_from(&lookup, "UNSUB_TESTBED_URL")?;
    let egress: Arc<dyn HttpEgress> = Arc::new(LoopbackEgress::new(&[
        ("FAKE_GOOGLE_URL", fake_google.clone()),
        ("UNSUB_TESTBED_URL", testbed),
    ])?);

    let (mut ports, fakes) = testkit::fake_ports();
    let clock = Arc::clone(&ports.clock);

    // The literal e2e client secret, from the testkit fakes (S10 3.3).
    fakes.secrets.set(
        SecretName::GoogleOAuthClientSecret,
        CLIENT_SECRET.as_bytes(),
    );

    let gmail_client = GmailHttp::new(
        Arc::clone(&egress),
        join(&fake_google, "gmail/v1/users/me")?,
        Arc::clone(&clock),
    );
    let gmail = Arc::new(GmailProvider::new(gmail_client));

    let store: Arc<dyn ServerStore> = store;
    ports.store = store;
    ports.egress = Arc::clone(&egress);
    ports.gmail = Arc::clone(&gmail) as Arc<dyn MailProvider>;
    ports.invite_mailer = Arc::clone(&gmail) as Arc<dyn InviteMailer>;
    ports.caller = Arc::new(StaticTokenVerifier::new(
        token,
        E2E_CALLER_EMAIL.to_owned(),
        E2E_AUDIENCE.to_owned(),
    )) as Arc<dyn CallerVerifier>;

    let state = UnsubState {
        ports,
        senders: vec![
            Arc::new(OneClickSender) as Arc<dyn UnsubSender>,
            Arc::new(MailtoSender) as Arc<dyn UnsubSender>,
        ],
        auth: InternalAuthConfig {
            audience: E2E_AUDIENCE.to_owned(),
            allowed_caller_email: E2E_CALLER_EMAIL.to_owned(),
        },
    };
    let bind = SocketAddr::from(([127, 0, 0, 1], port_from(&lookup)?));
    Ok((state, E2eConfig { bind }))
}

/// Build e2e ports from the process environment, over the emulator store.
///
/// # Errors
///
/// As [`build_e2e_ports`], plus a malformed `FIRESTORE_EMULATOR_HOST`.
pub fn build_e2e_from_env() -> Result<(UnsubState, E2eConfig), E2eError> {
    let store = firestore_emulator_store(|name| std::env::var(name).ok())?;
    build_e2e_ports(store, |name| std::env::var(name).ok())
}

/// The e2e router: the one internal route plus an unauthenticated liveness
/// route. Nothing else is mounted, so production behaviour is untouched.
pub fn e2e_router(state: UnsubState) -> axum::Router {
    crate::router(state).route(E2E_HEALTH_PATH, axum::routing::get(healthz))
}

/// A plain 200 for probes.
async fn healthz() -> axum::http::StatusCode {
    axum::http::StatusCode::OK
}

/// A verifier that accepts exactly one bearer value and reports a fixed,
/// verified caller. Nothing else can authenticate: an empty presented token is
/// `Malformed`, a different value is `BadSignature`, a token presented for a
/// different audience is `WrongAudience`.
pub struct StaticTokenVerifier {
    token: Sensitive<String>,
    caller_email: String,
    audience: String,
}

impl StaticTokenVerifier {
    /// Build a verifier for `token`, reporting `caller_email` for `audience`.
    #[must_use]
    pub fn new(token: Sensitive<String>, caller_email: String, audience: String) -> Self {
        Self {
            token,
            caller_email,
            audience,
        }
    }
}

#[async_trait]
impl CallerVerifier for StaticTokenVerifier {
    async fn verify(
        &self,
        bearer: &Sensitive<String>,
        audience: &str,
    ) -> Result<VerifiedCaller, CallerAuthError> {
        let presented = bearer.expose();
        if presented.is_empty() {
            return Err(CallerAuthError::Malformed);
        }
        if !constant_time_eq(presented.as_bytes(), self.token.expose().as_bytes()) {
            return Err(CallerAuthError::BadSignature);
        }
        if audience != self.audience {
            return Err(CallerAuthError::WrongAudience);
        }
        Ok(VerifiedCaller {
            email: self.caller_email.clone(),
            email_verified: true,
        })
    }
}

/// A loopback-only [`HttpEgress`]: it allows requests to exactly the configured
/// sockets (the `fake-google` and unsubscribe-testbed addresses) and refuses
/// everything else before any connection is opened.
pub struct LoopbackEgress {
    sockets: Vec<SocketAddr>,
    client: reqwest::Client,
}

impl LoopbackEgress {
    /// Build an egress pinned to the sockets of `endpoints`. Each host must be
    /// a loopback literal: IPv4 `127.0.0.0/8` or IPv6 `::1` (the bracketed
    /// `[::1]` URL form included).
    ///
    /// # Errors
    ///
    /// Returns [`E2eError::Invalid`] (named by the offending variable) when an
    /// endpoint has no literal loopback IP host or port, and
    /// [`E2eError::Adapter`] if the client cannot be built.
    pub fn new(endpoints: &[(&'static str, Url)]) -> Result<Self, E2eError> {
        if endpoints.is_empty() {
            return Err(E2eError::Invalid("UNSUB_TESTBED_URL"));
        }
        let mut sockets = Vec::with_capacity(endpoints.len());
        for (name, base) in endpoints {
            let host = base.host_str().ok_or(E2eError::Invalid(name))?;
            let ip = host_ip(host).ok_or(E2eError::Invalid(name))?;
            if !ip.is_loopback() {
                return Err(E2eError::Invalid(name));
            }
            let port = base
                .port_or_known_default()
                .ok_or(E2eError::Invalid(name))?;
            sockets.push(SocketAddr::new(ip, port));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| E2eError::Adapter)?;
        Ok(Self { sockets, client })
    }

    /// True only when the request targets exactly one allowed socket.
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
        self.sockets.contains(&SocketAddr::new(ip, port))
    }
}

#[async_trait]
impl HttpEgress for LoopbackEgress {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError> {
        if !self.allowed(url) {
            return Err(EgressError::HostNotAllowed);
        }
        let response = self
            .client
            .post(url.clone())
            .header("Content-Type", "application/x-www-form-urlencoded")
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

/// The accepted token: present, non-blank and at least [`MIN_TOKEN_LEN`] bytes.
/// A blank or absent value is `Missing`; a short one is `Invalid`.
pub fn accepted_token(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Sensitive<String>, E2eError> {
    let raw = lookup(E2E_CALLER_TOKEN_ENV)
        .filter(|v| !v.trim().is_empty())
        .ok_or(E2eError::Missing(E2E_CALLER_TOKEN_ENV))?;
    if raw.len() < MIN_TOKEN_LEN {
        return Err(E2eError::Invalid(E2E_CALLER_TOKEN_ENV));
    }
    Ok(Sensitive::new(raw))
}

/// The listen port from the lookup, defaulting when unset or blank.
fn port_from(lookup: &impl Fn(&str) -> Option<String>) -> Result<u16, E2eError> {
    match lookup("PORT").filter(|v| !v.trim().is_empty()) {
        None => Ok(DEFAULT_PORT),
        Some(raw) => raw
            .trim()
            .parse::<u16>()
            .map_err(|_| E2eError::Invalid("PORT")),
    }
}

/// A required, non-empty e2e variable. Only the name is ever surfaced.
fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, E2eError> {
    lookup(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or(E2eError::Missing(name))
}

/// A required URL variable.
fn url_from(lookup: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<Url, E2eError> {
    let raw = required(lookup, name)?;
    Url::parse(&raw).map_err(|_| E2eError::Invalid(name))
}

/// Join a `fake-google` path onto its base URL.
fn join(base: &Url, path: &str) -> Result<Url, E2eError> {
    base.join(path)
        .map_err(|_| E2eError::Invalid("FAKE_GOOGLE_URL"))
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

/// Constant-time byte comparison, so a wrong token never leaks its match
/// length through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
