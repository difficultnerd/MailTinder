//! One-click and `call` egress integration tests against local servers with a
//! fake resolver, plus the pure unit tables for URL checks and allowlists.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::needless_pass_by_value,
    clippy::enum_glob_use,
    clippy::single_match_else,
    clippy::map_unwrap_or,
    clippy::doc_markdown,
    clippy::ignored_unit_patterns,
    clippy::assert_is_empty,
    clippy::redundant_closure,
    clippy::unused_async
)]

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use egress::{
    allowlist, allows, check_one_click_url, one_click_permitted, ProdEgress, Service, TestOverride,
    ONE_CLICK_PORT,
};
use ports::{EgressError, EgressRequest, HttpEgress, HttpMethod, OneClickOutcome};
use url::Url;

const UNSUB_HOST: &str = "unsub-host.test";
const LOCALHOST: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

// ---------------------------------------------------------------------------
// Fake resolver
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StubResolver {
    calls: Arc<AtomicU32>,
    scripts: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
}

impl StubResolver {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicU32::new(0)),
            scripts: Mutex::new(HashMap::new()),
        }
    }

    fn calls_counter(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.calls)
    }

    fn map(&self, host: &str, addrs: Vec<IpAddr>) {
        let mut scripts = self
            .scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scripts.insert(host.to_owned(), VecDeque::from([addrs]));
    }

    fn seq(&self, host: &str, answers: Vec<Vec<IpAddr>>) {
        let mut scripts = self
            .scripts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scripts.insert(host.to_owned(), VecDeque::from(answers));
    }
}

#[async_trait]
impl egress::Resolver for StubResolver {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, EgressError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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

/// A resolver that always fails; fine for tests that refuse before resolving.
struct NoopResolver;

#[async_trait]
impl egress::Resolver for NoopResolver {
    async fn lookup(&self, _host: &str) -> Result<Vec<IpAddr>, EgressError> {
        Err(EgressError::DnsFailed)
    }
}

// ---------------------------------------------------------------------------
// Local servers
// ---------------------------------------------------------------------------

/// A recorded request the local server saw.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

type Responder = Arc<dyn Fn(&Recorded) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync>;

/// Spawn a plain-HTTP axum server that records every request and answers with
/// the scripted response. Returns the address and the record list.
async fn spawn_server(responder: Responder) -> (SocketAddr, Arc<Mutex<Vec<Recorded>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let records_for_handler = Arc::clone(&records);
    let app = axum::Router::new().route(
        "/{*path}",
        axum::routing::any(move |request: axum::extract::Request| {
            let records = Arc::clone(&records_for_handler);
            let responder = Arc::clone(&responder);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let recorded = Recorded {
                    method: parts.method.to_string(),
                    headers: parts
                        .headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
                        .collect(),
                    body: bytes.to_vec(),
                };
                let (status, headers, body) = responder(&recorded);
                let mut builder = axum::response::Response::builder().status(status);
                for (k, v) in headers {
                    builder = builder.header(k, v);
                }
                let response = builder.body(axum::body::Body::from(body)).unwrap();
                records.lock().unwrap().push(recorded);
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, records)
}

fn respond(status: u16) -> Responder {
    Arc::new(move |_| (status, Vec::new(), Vec::new()))
}

/// A TCP server that accepts connections but never answers, so a client times
/// out waiting for a response.
async fn spawn_hanging_server() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(stream);
                });
            }
        }
    });
    addr
}

/// A TLS server with a self-signed certificate (so the client's verification
/// fails). The certificate is generated with `rcgen`.
async fn spawn_untrusted_tls_server() -> SocketAddr {
    // rustls 0.23 does not auto-install a provider; install the ring provider
    // so `ServerConfig::builder()` can build the self-signed server.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_der = certified.cert.der().clone();
    let key_der = certified.signing_key.serialize_der();
    let private_key = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_der),
    );
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], private_key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let _ = acceptor.accept(stream).await;
                });
            }
        }
    });
    addr
}

// ---------------------------------------------------------------------------
// Override clients
// ---------------------------------------------------------------------------

fn unsub_with_socket(
    resolver: StubResolver,
    socket: SocketAddr,
    plain_http: bool,
    timeout: Duration,
) -> ProdEgress {
    ProdEgress::with_test_override(
        Service::Unsub,
        Arc::new(resolver),
        TestOverride {
            allow_socket: Some(socket),
            allow_plain_http_to_socket: plain_http,
            extra_root_ca_pem: None,
            host_routes: Vec::new(),
            one_click_timeout: timeout,
        },
    )
    .unwrap()
}

fn hits(records: &Arc<Mutex<Vec<Recorded>>>) -> usize {
    records.lock().unwrap().len()
}

// ---------------------------------------------------------------------------
// Unit tables
// ---------------------------------------------------------------------------

/// Every URL in the V1.3.6 table gives exactly its expected error.
#[test]
fn asvs_v1_3_6_url_checks_table() {
    let cases: &[(&str, EgressError)] = &[
        ("http://host.example/", EgressError::SchemeNotAllowed),
        (
            "https://user:pw@host.example/",
            EgressError::CredentialsInUrl,
        ),
        ("https://host.example:8443/", EgressError::PortNotAllowed),
        ("https://2130706433/", EgressError::IpLiteralHost),
        ("https://0177.0.0.1/", EgressError::IpLiteralHost),
        ("https://0x7f.1/", EgressError::IpLiteralHost),
        ("https://[::1]/", EgressError::IpLiteralHost),
        ("https://localhost/", EgressError::HostNotAllowed),
        (
            "https://metadata.google.internal./",
            EgressError::HostNotAllowed,
        ),
        ("https://metadata/", EgressError::HostNotAllowed),
        ("https://svc.internal/", EgressError::HostNotAllowed),
        ("https://printer.local/", EgressError::HostNotAllowed),
    ];
    for (raw, expected) in cases {
        let url = Url::parse(raw).unwrap_or_else(|_| panic!("parse {raw}"));
        assert_eq!(
            check_one_click_url(&url),
            Err(expected.clone()),
            "for {raw}"
        );
    }
    assert_eq!(ONE_CLICK_PORT, 443);

    // A clean URL passes and returns the host.
    let clean = url_parse("https://unsub-target.test/");
    assert_eq!(
        check_one_click_url(&clean),
        Ok("unsub-target.test".to_owned())
    );

    // An over-long URL is refused for length.
    let long_path = "b".repeat(2100);
    let long_url = url_parse(&format!("https://unsub-target.test/{long_path}"));
    assert!(long_url.as_str().len() > 2048);
    assert_eq!(
        check_one_click_url(&long_url),
        Err(EgressError::HostNotAllowed)
    );
}

/// The allowlist table per service (ASVS V13.2.4).
#[test]
fn asvs_v13_2_4_allowlists_table() {
    // `unsub` must never reach Drive (T-702 regression guard).
    let drive_url = url_parse("https://www.googleapis.com/drive/v3/files");
    assert!(!allows(Service::Unsub, &drive_url));

    let api_allowed = [
        "https://gmail.googleapis.com/gmail/v1/users/me/messages",
        "https://www.googleapis.com/drive/v3/files",
        "https://www.googleapis.com/upload/drive/v2/files",
        "https://oauth2.googleapis.com/token",
        "https://oauth2.googleapis.com/revoke",
        "https://accounts.google.com/.well-known/openid-configuration",
        "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/l",
        "https://api.typesafe.ai/v1/systemone/classify",
        "https://www.googleapis.com/oauth2/v3/certs",
    ];
    for raw in api_allowed {
        assert!(allows(Service::Api, &url_parse(raw)), "api {raw}");
    }
    let api_refused = [
        "https://example.com/",
        "https://gmail.googleapis.com/drive/v3/files",
        "https://gmail.googleapis.com:8443/gmail/v1/users/me/messages",
        "http://gmail.googleapis.com/gmail/v1/users/me/messages",
    ];
    for raw in api_refused {
        assert!(!allows(Service::Api, &url_parse(raw)), "api refused {raw}");
    }

    assert!(allows(
        Service::Unsub,
        &url_parse("https://gmail.googleapis.com/gmail/v1/users/me/messages")
    ));
    assert!(allows(
        Service::Unsub,
        &url_parse("https://oauth2.googleapis.com/token")
    ));
    assert!(allows(
        Service::Unsub,
        &url_parse("https://www.googleapis.com/oauth2/v3/certs")
    ));
    assert!(!allows(Service::Unsub, &drive_url));
    assert!(!allows(
        Service::Unsub,
        &url_parse("https://gmail.googleapis.com/drive/v3/files")
    ));

    assert!(allows(
        Service::Worker,
        &url_parse("https://www.googleapis.com/oauth2/v3/certs")
    ));
    assert!(!allows(
        Service::Worker,
        &url_parse("https://www.googleapis.com/drive/v3/files")
    ));
    assert!(!allows(Service::Worker, &url_parse("https://example.com/")));

    // Every service resolves an exhaustive, non-empty allowlist.
    assert!(!allowlist(Service::Api).is_empty());
    assert!(!allowlist(Service::Unsub).is_empty());
    assert!(!allowlist(Service::Worker).is_empty());
}

/// Production `call` is https-only (ASVS V12.3.1): an `http://` allowlisted
/// URL gives SchemeNotAllowed before any DNS.
#[tokio::test]
async fn asvs_v12_3_1_production_client_is_https_only() {
    let client = ProdEgress::new(Service::Api, Arc::new(NoopResolver)).unwrap();
    let req = EgressRequest {
        method: HttpMethod::Get,
        url: url_parse("http://gmail.googleapis.com/gmail/v1/users/me/messages"),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
    };
    let err = client.call(req).await.unwrap_err();
    assert_eq!(err, EgressError::SchemeNotAllowed);
}

/// One-click is permitted for `unsub` only.
#[tokio::test]
async fn one_click_not_permitted_for_api_or_worker() {
    let url = url_parse(&format!("https://{UNSUB_HOST}/"));
    for service in [Service::Api, Service::Worker] {
        let client = ProdEgress::new(service, Arc::new(NoopResolver)).unwrap();
        assert_eq!(
            client.one_click_post(&url).await.unwrap_err(),
            EgressError::NotPermitted
        );
    }
    assert!(one_click_permitted(Service::Unsub));
}

/// The Gmail permanent-delete guard refuses DELETE and batchDelete (INV-5).
#[tokio::test]
async fn inv_5_egress_refuses_gmail_delete_and_batch_delete() {
    let client = ProdEgress::new(Service::Api, Arc::new(NoopResolver)).unwrap();

    let delete = EgressRequest {
        method: HttpMethod::Delete,
        url: url_parse("https://gmail.googleapis.com/gmail/v1/users/me/messages/123"),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
    };
    assert_eq!(
        client.call(delete).await.unwrap_err(),
        EgressError::PermanentDeleteRefused
    );

    let batch = EgressRequest {
        method: HttpMethod::Post,
        url: url_parse("https://gmail.googleapis.com/gmail/v1/users/me/messages/batchDelete"),
        headers: Vec::new(),
        body: Some(Vec::new()),
        timeout: Duration::from_secs(5),
    };
    assert_eq!(
        client.call(batch).await.unwrap_err(),
        EgressError::PermanentDeleteRefused
    );
}

// ---------------------------------------------------------------------------
// One-click SSRF integration
// ---------------------------------------------------------------------------

/// Refusing a target that resolves to any forbidden address: zero connections,
/// error AddressRefused with that range (ASVS V1.3.6).
#[tokio::test]
async fn un_02_ac4_refuses_each_forbidden_resolution() {
    let forbidden: &[(&str, IpAddr, EgressError)] = &[
        (
            "loopback.test",
            ip("127.0.0.1"),
            EgressError::AddressRefused(ports::RefusedRange::Loopback),
        ),
        (
            "private.test",
            ip("10.0.0.1"),
            EgressError::AddressRefused(ports::RefusedRange::Private),
        ),
        (
            "cgnat.test",
            ip("100.64.0.1"),
            EgressError::AddressRefused(ports::RefusedRange::Cgnat),
        ),
        (
            "metadata.test",
            ip("169.254.169.254"),
            EgressError::AddressRefused(ports::RefusedRange::Metadata),
        ),
        (
            "linklocal.test",
            ip("169.254.1.1"),
            EgressError::AddressRefused(ports::RefusedRange::LinkLocal),
        ),
        (
            "v6loopback.test",
            ip("::1"),
            EgressError::AddressRefused(ports::RefusedRange::Loopback),
        ),
        (
            "v6linklocal.test",
            ip("fe80::1"),
            EgressError::AddressRefused(ports::RefusedRange::LinkLocal),
        ),
        (
            "v6unique.test",
            ip("fd20:ce::254"),
            EgressError::AddressRefused(ports::RefusedRange::UniqueLocal),
        ),
        (
            "mapped.test",
            ip("::ffff:127.0.0.1"),
            EgressError::AddressRefused(ports::RefusedRange::Loopback),
        ),
    ];
    for (host, addr, expected) in forbidden {
        let resolver = StubResolver::new();
        resolver.map(host, vec![*addr]);
        let client = ProdEgress::new(Service::Unsub, Arc::new(resolver)).unwrap();
        let url = url_parse(&format!("https://{host}/"));
        let err = client.one_click_post(&url).await.unwrap_err();
        assert_eq!(&err, expected, "for host {host}");
    }
}

/// If any answer is refused, the whole target is refused (an attacker can
/// choose which answer a client uses).
#[tokio::test]
async fn un_02_ac4_any_refused_answer_refuses_target() {
    let resolver = StubResolver::new();
    resolver.map(UNSUB_HOST, vec![ip("93.184.216.34"), ip("10.0.0.1")]);
    let client = ProdEgress::new(Service::Unsub, Arc::new(resolver)).unwrap();
    let url = url_parse(&format!("https://{UNSUB_HOST}/"));
    assert_eq!(
        client.one_click_post(&url).await.unwrap_err(),
        EgressError::AddressRefused(ports::RefusedRange::Private)
    );
}

/// DNS rebinding is defeated by pinning: the resolver is consulted once and the
/// connection uses the checked address.
#[tokio::test]
async fn un_02_ac4_dns_rebinding_is_pinned() {
    let (addr, records) = spawn_server(respond(200)).await;
    let resolver = StubResolver::new();
    resolver.seq("rebind.test", vec![vec![LOCALHOST], vec![ip("10.0.0.1")]]);
    let resolver_calls = resolver.calls_counter();
    let client = unsub_with_socket(resolver, addr, true, Duration::from_secs(10));
    let url = url_parse(&format!("http://rebind.test:{}/one-click", addr.port()));
    let outcome = client.one_click_post(&url).await.unwrap();
    assert!(matches!(outcome, OneClickOutcome::Accepted { status: 200 }));
    // The resolver answered exactly once: the pinned connection never asks
    // again, so the later private answer can never redirect the connection.
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hits(&records), 1);
}

/// A 302 is reported as Redirected and never followed (ASVS V15.3.2).
#[tokio::test]
async fn un_02_ac4_redirect_not_followed() {
    let (second_addr, second_records) = spawn_server(respond(200)).await;
    let second_url = format!("http://127.0.0.1:{}/landing", second_addr.port());
    let respond_302 = Arc::new(move |_: &Recorded| {
        (
            302,
            vec![("Location".to_owned(), second_url.clone())],
            Vec::new(),
        )
    });
    let (addr, _records) = spawn_server(respond_302).await;

    let resolver = StubResolver::new();
    resolver.map("redir.test", vec![LOCALHOST]);
    let client = unsub_with_socket(resolver, addr, true, Duration::from_secs(10));
    let url = url_parse(&format!("http://redir.test:{}/one-click", addr.port()));
    let outcome = client.one_click_post(&url).await.unwrap();
    assert!(matches!(
        outcome,
        OneClickOutcome::Redirected { status: 302 }
    ));
    assert_eq!(hits(&second_records), 0);
}

/// A server that never answers yields TimedOut, not an error.
#[tokio::test]
async fn un_02_ac4_timeout_is_timed_out() {
    let addr = spawn_hanging_server().await;
    let resolver = StubResolver::new();
    resolver.map("slow.test", vec![LOCALHOST]);
    let client = unsub_with_socket(resolver, addr, true, Duration::from_millis(250));
    let url = url_parse(&format!("http://slow.test:{}/one-click", addr.port()));
    let outcome = client.one_click_post(&url).await.unwrap();
    assert_eq!(outcome, OneClickOutcome::TimedOut);
}

/// The request shape matches RFC 8058 exactly: POST, fixed body, Content-Type,
/// fixed User-Agent, and no Cookie, Authorization, Referer or Origin.
#[tokio::test]
async fn un_02_ac1_request_shape() {
    let (addr, records) = spawn_server(respond(200)).await;
    let resolver = StubResolver::new();
    resolver.map("shape.test", vec![LOCALHOST]);
    let client = unsub_with_socket(resolver, addr, true, Duration::from_secs(10));
    let url = url_parse(&format!("http://shape.test:{}/one-click", addr.port()));
    let outcome = client.one_click_post(&url).await.unwrap();
    assert!(matches!(outcome, OneClickOutcome::Accepted { status: 200 }));

    let list = records.lock().unwrap();
    assert_eq!(list.len(), 1);
    let r = &list[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.body, b"List-Unsubscribe=One-Click");
    assert_eq!(
        r.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(r.header("user-agent"), Some("MailTinder-Unsubscribe/1"));
    for forbidden in ["cookie", "authorization", "referer", "origin"] {
        assert!(!r.has_header(forbidden), "must not send {forbidden} header");
    }
}

/// An unlisted host is refused before any DNS (ASVS V13.2.5).
#[tokio::test]
async fn asvs_v13_2_5_unlisted_host_refused_before_connect() {
    let resolver = StubResolver::new();
    resolver.map("gmail.googleapis.com", vec![ip("142.250.1.1")]);
    let resolver_calls = resolver.calls_counter();
    let client = ProdEgress::new(Service::Unsub, Arc::new(resolver)).unwrap();
    // `api.typesafe.ai` is not on the `unsub` allowlist.
    let req = EgressRequest {
        method: HttpMethod::Get,
        url: url_parse("https://api.typesafe.ai/v1/systemone/classify"),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
    };
    let err = client.call(req).await.unwrap_err();
    assert_eq!(err, EgressError::HostNotAllowed);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

/// `call` never follows a redirect (ASVS V15.3.2).
#[tokio::test]
async fn asvs_v15_3_2_call_follows_no_redirect() {
    let (second_addr, second_records) = spawn_server(respond(200)).await;
    let second_url = format!("http://127.0.0.1:{}/landing", second_addr.port());
    let respond_302 = Arc::new(move |_: &Recorded| {
        (
            302,
            vec![("Location".to_owned(), second_url.clone())],
            Vec::new(),
        )
    });
    let (addr, _records) = spawn_server(respond_302).await;

    let resolver = StubResolver::new();
    let client = ProdEgress::with_test_override(
        Service::Api,
        Arc::new(resolver),
        TestOverride {
            allow_socket: None,
            allow_plain_http_to_socket: false,
            extra_root_ca_pem: None,
            host_routes: vec![("gmail.googleapis.com", addr)],
            one_click_timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    let req = EgressRequest {
        method: HttpMethod::Get,
        url: url_parse("https://gmail.googleapis.com/gmail/v1/users/me/messages"),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
    };
    let resp = client.call(req).await.unwrap();
    assert_eq!(resp.status, 302);
    assert_eq!(hits(&second_records), 0);
}

/// A TLS failure surfaces as Err(Tls) and a captured `egress_tls_failure`
/// security event (ASVS V16.3.4).
#[tokio::test]
async fn asvs_v16_3_4_tls_failure_is_error_and_logged() {
    let (sink, _guard) = obs::capture("unsub", obs::arc(obs::FixedClock::default()));
    let tls_addr = spawn_untrusted_tls_server().await;
    let resolver = StubResolver::new();
    resolver.map("tlssrv.test", vec![LOCALHOST]);
    let client = unsub_with_socket(resolver, tls_addr, false, Duration::from_secs(10));
    let url = url_parse(&format!("https://tlssrv.test:{}/relay", tls_addr.port()));
    let err = client.one_click_post(&url).await.unwrap_err();
    assert_eq!(err, EgressError::Tls);
    assert!(
        sink.text().contains("egress_tls_failure"),
        "expected a `egress_tls_failure` security event, got: {}",
        sink.text()
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn url_parse(s: &str) -> Url {
    Url::parse(s).unwrap_or_else(|_| panic!("parse {s}"))
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}
