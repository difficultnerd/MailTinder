//! `unsub-testbed`: the local unsubscribe sites (S10 6.2).
//!
//! A small `axum` server, usable both as a library inside tests and as a binary
//! for `scripts/e2e.sh`, that stands in for unsubscribe endpoints. Each route
//! is one scenario and every request is recorded (method, path, headers, body,
//! cookies) *before* the scenario runs, so a client timeout still leaves a
//! record. It also exports the SSRF host table that T-702 uses with a fake
//! resolver.
//!
//! Page-handler routes are v2 and are not built here.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::doc_markdown,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::unreadable_literal,
    clippy::similar_names,
    clippy::unused_async,
    clippy::ignored_unit_patterns,
    clippy::option_if_let_else,
    clippy::map_unwrap_or,
    clippy::cast_possible_truncation,
    clippy::redundant_clone
)]

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::serve::Listener;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use url::Url;

pub mod ssrf_table;

pub use ssrf_table::{SsrfCase, SSRF_CASES};

/// The ceiling on the `/oneclick/hang` route: it protects a forgotten test, not
/// a real scenario (S10 6.2).
pub const HANG_CEILING: Duration = Duration::from_secs(30);

/// Exactly what a request carried, recorded before the scenario is applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    /// The HTTP method, upper case.
    pub method: String,
    /// The request path, without the query.
    pub path: String,
    /// Lower-cased header names and their values.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
    /// True when a `Cookie` header arrived.
    pub cookies_present: bool,
}

#[derive(Default)]
struct Inner {
    records: Vec<Recorded>,
    counters: HashMap<String, u32>,
}

#[derive(Clone)]
struct AppState {
    inner: Arc<Mutex<Inner>>,
    hang: Arc<tokio::sync::Notify>,
}

impl AppState {
    fn note(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &Bytes) {
        let record = Recorded {
            method: method.as_str().to_owned(),
            path: uri.path().to_owned(),
            headers: headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_ascii_lowercase(),
                        value.to_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect(),
            body: body.to_vec(),
            cookies_present: headers.contains_key(header::COOKIE),
        };
        self.lock().records.push(record);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_count(&self, key: &str) -> u32 {
        let mut inner = self.lock();
        let count = inner.counters.entry(key.to_owned()).or_insert(0);
        *count += 1;
        *count
    }

    fn requests(&self) -> Vec<Recorded> {
        self.lock().records.clone()
    }

    fn reset(&self) {
        let mut inner = self.lock();
        inner.records.clear();
        inner.counters.clear();
    }
}

/// One scenario, selected by the request path.
enum Scenario {
    Status(u16),
    StatusThen200 { key: String },
    FailN { n: u32, key: String },
    Hang,
    Redirect(u16),
    Landed,
}

fn scenario_of(path: &str) -> Option<Scenario> {
    if path == "/landed" {
        return Some(Scenario::Landed);
    }
    let rest = path.strip_prefix("/oneclick/")?;
    if rest == "hang" {
        return Some(Scenario::Hang);
    }
    if let Ok(code) = rest.parse::<u16>() {
        return Some(Scenario::Status(code));
    }
    if let Some(key) = rest.strip_prefix("500-then-200/") {
        return Some(Scenario::StatusThen200 {
            key: key.to_owned(),
        });
    }
    if let Some(tail) = rest.strip_prefix("fail-n/") {
        let (n, key) = tail.split_once('/')?;
        return n.parse::<u32>().ok().map(|n| Scenario::FailN {
            n,
            key: key.to_owned(),
        });
    }
    if let Some(code) = rest.strip_prefix("redirect/") {
        return code.parse::<u16>().ok().map(Scenario::Redirect);
    }
    None
}

fn status_response(code: u16) -> Response {
    StatusCode::from_u16(code)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        .into_response()
}

/// The one scenario handler: it records first, then applies the scenario.
/// One-click routes answer `POST` only; another method gets 405 and is still
/// recorded.
async fn handle(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.note(&method, &uri, &headers, &body);

    match scenario_of(uri.path()) {
        Some(Scenario::Landed) => StatusCode::OK.into_response(),
        Some(_) if method != Method::POST => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        Some(Scenario::Status(code)) => status_response(code),
        Some(Scenario::StatusThen200 { key }) => {
            if state.next_count(&format!("500-then-200:{key}")) == 1 {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            } else {
                StatusCode::OK.into_response()
            }
        }
        Some(Scenario::FailN { n, key }) => {
            if state.next_count(&format!("fail-n:{key}")) <= n {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            } else {
                StatusCode::OK.into_response()
            }
        }
        Some(Scenario::Hang) => {
            tokio::select! {
                _ = state.hang.notified() => {}
                _ = tokio::time::sleep(HANG_CEILING) => {}
            }
            StatusCode::OK.into_response()
        }
        Some(Scenario::Redirect(code)) => {
            let mut response = status_response(code);
            response
                .headers_mut()
                .insert(header::LOCATION, HeaderValue::from_static("/landed"));
            response
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn control_requests(State(state): State<AppState>) -> Json<Vec<Recorded>> {
    Json(state.requests())
}

async fn control_reset(State(state): State<AppState>) -> StatusCode {
    state.reset();
    StatusCode::OK
}

/// The router. The `/__testbed/*` control routes exist only in the binary's
/// router (`control = true`); in-process tests read the same data through
/// [`Testbed`] methods.
fn router(state: AppState, control: bool) -> Router {
    let mut app = Router::new()
        .route("/oneclick/200", any(handle))
        .route("/oneclick/202", any(handle))
        .route("/oneclick/204", any(handle))
        .route("/oneclick/400", any(handle))
        .route("/oneclick/500", any(handle))
        .route("/oneclick/500-then-200/{key}", any(handle))
        .route("/oneclick/fail-n/{n}/{key}", any(handle))
        .route("/oneclick/hang", any(handle))
        .route("/oneclick/redirect/{code}", any(handle))
        .route("/landed", any(handle));
    if control {
        app = app
            .route("/__testbed/requests", get(control_requests))
            .route("/__testbed/reset", post(control_reset));
    }
    app.with_state(state)
}

/// A running testbed: plain HTTP and HTTPS listeners, both on loopback.
pub struct Testbed {
    /// The plain HTTP listener address, bound on `127.0.0.1`.
    pub addr: SocketAddr,
    /// The HTTPS listener address, bound on `127.0.0.1`.
    pub https_addr: SocketAddr,
    base_http: Url,
    base_https: Url,
    state: AppState,
    http_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    https_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    http_task: Option<tokio::task::JoinHandle<()>>,
    https_task: Option<tokio::task::JoinHandle<()>>,
}

impl Testbed {
    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> Url {
        with_path(&self.base_http, path)
    }

    /// `https://127.0.0.1:<port><path>` (the TLS listener).
    pub fn https_url(&self, path: &str) -> Url {
        with_path(&self.base_https, path)
    }

    /// The PEM trust root for the egress test client.
    pub fn test_ca_pem(&self) -> &'static [u8] {
        test_ca().pem.as_slice()
    }

    /// Everything recorded, in order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.state.requests()
    }

    /// Everything recorded for one path, without its query.
    pub fn requests_to(&self, path: &str) -> Vec<Recorded> {
        self.state
            .requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }

    /// Let the `/oneclick/hang` requests finish.
    pub fn release_hanging(&self) {
        // `notify_one` stores a permit when no request is waiting yet, so a
        // release that lands before the request arrives is not lost.
        self.state.hang.notify_one();
    }

    /// Stop both listeners and wait for them to finish.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.http_shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(tx) = self.https_shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.http_task.take() {
            let _ = task.await;
        }
        if let Some(task) = self.https_task.take() {
            let _ = task.await;
        }
    }
}

fn with_path(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Binds port 0 on `127.0.0.1` for plain HTTP and a second port for HTTPS;
/// returns once both listen.
pub async fn start() -> io::Result<Testbed> {
    start_inner(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        false,
        CertKind::TrustedCa,
    )
    .await
}

/// Same, but the HTTPS listener uses a certificate the test CA did not sign
/// (for the TLS-failure test).
pub async fn start_with_untrusted_cert() -> io::Result<Testbed> {
    start_inner(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        false,
        CertKind::UntrustedSelfSigned,
    )
    .await
}

/// Bind a specific plain-HTTP address (the binary uses `TESTBED_ADDR`). With
/// `control`, the `/__testbed/*` routes are mounted.
pub async fn start_at(addr: SocketAddr, control: bool) -> io::Result<Testbed> {
    start_inner(addr, control, CertKind::TrustedCa).await
}

#[derive(Clone, Copy)]
enum CertKind {
    TrustedCa,
    UntrustedSelfSigned,
}

async fn start_inner(addr: SocketAddr, control: bool, cert: CertKind) -> io::Result<Testbed> {
    install_crypto_provider();

    let state = AppState {
        inner: Arc::new(Mutex::new(Inner::default())),
        hang: Arc::new(tokio::sync::Notify::new()),
    };
    let app = router(state.clone(), control);

    // Plain HTTP listener.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let http_addr = listener.local_addr()?;
    let (http_shutdown, http_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app.clone()).with_graceful_shutdown(async {
        let _ = http_rx.await;
    });
    let http_task = tokio::spawn(async move {
        let _ = server.await;
    });

    // HTTPS listener.
    let tls_listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let https_addr = tls_listener.local_addr()?;
    let acceptor = tls_acceptor(cert)?;
    let (https_shutdown, https_rx) = tokio::sync::oneshot::channel::<()>();
    let tls = TlsListener {
        inner: tls_listener,
        acceptor,
    };
    let server = axum::serve(tls, app).with_graceful_shutdown(async {
        let _ = https_rx.await;
    });
    let https_task = tokio::spawn(async move {
        let _ = server.await;
    });

    let base_http = Url::parse(&format!("http://127.0.0.1:{}", http_addr.port()))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let base_https = Url::parse(&format!("https://127.0.0.1:{}", https_addr.port()))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

    Ok(Testbed {
        addr: http_addr,
        https_addr,
        base_http,
        base_https,
        state,
        http_shutdown: Some(http_shutdown),
        https_shutdown: Some(https_shutdown),
        http_task: Some(http_task),
        https_task: Some(https_task),
    })
}

/// An `axum` listener that completes the TLS handshake before handing the
/// stream to the router. A failed handshake (an untrusted certificate, or a
/// client that rejects ours) drops that connection and keeps accepting.
struct TlsListener {
    inner: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if let Ok((stream, addr)) = self.inner.accept().await {
                if let Ok(tls) = self.acceptor.accept(stream).await {
                    return (tls, addr);
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

fn install_crypto_provider() {
    // rustls 0.23 needs a process-default crypto provider. Ignore the error:
    // another crate may have installed one already.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn tls_acceptor(kind: CertKind) -> io::Result<tokio_rustls::TlsAcceptor> {
    let (cert, key) = leaf_certificate(kind).map_err(rcgen_error)?;
    let private_key = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
    );
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], private_key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The leaf certificate: either signed by the shared test CA, or a second,
/// unrelated self-signed leaf.
fn leaf_certificate(
    kind: CertKind,
) -> Result<(rustls::pki_types::CertificateDer<'static>, rcgen::KeyPair), rcgen::Error> {
    let key = rcgen::KeyPair::generate()?;
    let mut params =
        rcgen::CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "unsub-testbed");
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

    let cert = match kind {
        CertKind::TrustedCa => params.signed_by(&key, &test_ca().certified)?,
        CertKind::UntrustedSelfSigned => params.self_signed(&key)?,
    };
    Ok((cert.der().clone(), key))
}

fn rcgen_error(e: rcgen::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

struct TestCa {
    certified: rcgen::CertifiedIssuer<'static, rcgen::KeyPair>,
    pem: Vec<u8>,
}

/// The shared test CA, generated once per process with `rcgen`.
fn test_ca() -> &'static TestCa {
    static CA: OnceLock<TestCa> = OnceLock::new();
    CA.get_or_init(|| build_test_ca().unwrap_or_else(|e| panic!("test CA: {e}")))
}

fn build_test_ca() -> Result<TestCa, rcgen::Error> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        "MailTinder unsub-testbed test CA",
    );
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let certified = rcgen::CertifiedIssuer::self_signed(params, key)?;
    let pem = certified.as_ref().pem().into_bytes();
    Ok(TestCa { certified, pem })
}
