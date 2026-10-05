//! `ProdEgress`: the production `HttpEgress` implementation.
//!
//! Every request is https-only, resolves the host once, checks every answer
//! against `classify_ip`, pins the checked address for the connection (so a
//! second DNS lookup cannot redirect it), and never follows a redirect.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use obs::{op_log, security_event, OpLog, SecurityEvent};
use ports::{EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, OneClickOutcome};
use url::Url;

use crate::allowlist::{allows, one_click_permitted, Service};
use crate::ranges::classify_ip;
use crate::resolver::Resolver;
use crate::target::ONE_CLICK_PORT;
#[cfg(feature = "test-policy")]
use crate::test_policy::TestOverride;

/// The fixed RFC 8058 one-click body.
pub const ONE_CLICK_BODY: &[u8] = b"List-Unsubscribe=One-Click";
/// The one-click request timeout (S6 sec. 6).
pub const ONE_CLICK_TIMEOUT: Duration = Duration::from_secs(10);
/// The fixed user-agent on outbound requests.
pub const USER_AGENT: &str = "MailTinder-Unsubscribe/1";
/// One-click response bodies are read up to this many bytes, then dropped.
pub const ONE_CLICK_MAX_BODY: usize = 64 * 1024;
/// The maximum `call` response body.
pub const CALL_MAX_BODY: usize = 8 * 1024 * 1024;
/// The maximum `call` request timeout.
pub const CALL_MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// The production egress client for one service.
pub struct ProdEgress {
    service: Service,
    resolver: Arc<dyn Resolver>,
    // Retained for signature parity; per-request clients are built fresh so the
    // checked address can be pinned for one host without pinning a shared client.
    #[allow(dead_code)]
    base_client: reqwest::Client,
    #[cfg(feature = "test-policy")]
    test: Option<TestOverride>,
}

impl ProdEgress {
    /// Build a production egress client for `service`.
    ///
    /// # Errors
    ///
    /// Returns `EgressError::Connect` if a base client cannot be built.
    pub fn new(service: Service, resolver: Arc<dyn Resolver>) -> Result<Self, EgressError> {
        let base_client = Self::base_builder()
            .https_only(true)
            .timeout(CALL_MAX_TIMEOUT)
            .build()
            .map_err(|_| EgressError::Connect)?;
        Ok(Self {
            service,
            resolver,
            base_client,
            #[cfg(feature = "test-policy")]
            test: None,
        })
    }

    /// Build an egress client with a test override (test-policy only).
    ///
    /// # Errors
    ///
    /// Returns `EgressError::Connect` if a base client cannot be built.
    #[cfg(feature = "test-policy")]
    pub fn with_test_override(
        service: Service,
        resolver: Arc<dyn Resolver>,
        t: TestOverride,
    ) -> Result<Self, EgressError> {
        let base_client = Self::base_builder()
            .https_only(!t.allow_plain_http_to_socket)
            .timeout(t.one_click_timeout)
            .build()
            .map_err(|_| EgressError::Connect)?;
        Ok(Self {
            service,
            resolver,
            base_client,
            test: Some(t),
        })
    }

    /// The shared policy for every outbound client: no redirects, no proxy, no
    /// referer, HTTP/1.1, the fixed user-agent, rustls, no cookie store.
    fn base_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .referer(false)
            .http1_only()
            .user_agent(USER_AGENT)
            .use_rustls_tls()
    }

    /// Build a per-request client, pinned to `pin` so the checked address is
    /// used for `host` and no second DNS lookup or proxy can redirect it.
    fn request_client(
        &self,
        host: &str,
        pin: SocketAddr,
        https_only: bool,
        timeout: Duration,
    ) -> Result<reqwest::Client, EgressError> {
        let builder = Self::base_builder()
            .https_only(https_only)
            .timeout(timeout)
            .connect_timeout(timeout)
            .resolve(host, pin);
        let builder = self.attach_test_ca(builder)?;
        builder.build().map_err(|_| EgressError::Connect)
    }

    /// Attach an extra trusted root CA from a test override, if any.
    #[cfg(feature = "test-policy")]
    fn attach_test_ca(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder, EgressError> {
        let Some(ca_pem) = self.test.as_ref().and_then(|t| t.extra_root_ca_pem.clone()) else {
            return Ok(builder);
        };
        let cert = reqwest::Certificate::from_pem(&ca_pem).map_err(|_| EgressError::Connect)?;
        Ok(builder.add_root_certificate(cert))
    }

    /// The one-click timeout, overridden in tests (T-702).
    #[cfg(feature = "test-policy")]
    fn one_click_timeout(&self) -> Duration {
        self.test
            .as_ref()
            .map_or(ONE_CLICK_TIMEOUT, |t| t.one_click_timeout)
    }

    #[cfg(not(feature = "test-policy"))]
    fn one_click_timeout(&self) -> Duration {
        ONE_CLICK_TIMEOUT
    }

    /// Extract the host, whether https is required and the pin port for a
    /// one-click URL. Production is strict (`check_one_click_url`); a test
    /// override may relax scheme and port for a loopback socket.
    fn one_click_target(&self, url: &Url) -> Result<(String, bool, u16), EgressError> {
        #[cfg(feature = "test-policy")]
        {
            if let Some(t) = &self.test {
                let https = !t.allow_plain_http_to_socket;
                if https && url.scheme() != "https" {
                    return Err(EgressError::SchemeNotAllowed);
                }
                if url.as_str().len() > crate::target::MAX_URL_LEN {
                    return Err(EgressError::HostNotAllowed);
                }
                let host = self.relaxed_host(url)?;
                let port = match t.allow_socket {
                    Some(socket) => socket.port(),
                    None => url.port().unwrap_or(ONE_CLICK_PORT),
                };
                return Ok((host, https, port));
            }
        }
        let host = crate::target::check_one_click_url(url)?;
        Ok((host, true, ONE_CLICK_PORT))
    }

    /// Host extraction for a one-click URL under a test override: IP literals
    /// and reserved names are still refused, but the scheme/port rules are
    /// handled by the override.
    #[cfg(feature = "test-policy")]
    fn relaxed_host(&self, url: &Url) -> Result<String, EgressError> {
        match url.host() {
            Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => Err(EgressError::IpLiteralHost),
            Some(url::Host::Domain(d)) => {
                let lower = d.to_ascii_lowercase();
                let stripped = lower.strip_suffix('.').unwrap_or(&lower);
                crate::target::check_host_name(stripped)?;
                Ok(stripped.to_owned())
            }
            None => Err(EgressError::HostNotAllowed),
        }
    }

    /// Resolve and check the addresses for a one-click target. An address
    /// matching the test socket is allowed only under a test override.
    async fn one_click_resolve(&self, host: &str, url: &Url) -> Result<Vec<IpAddr>, EgressError> {
        let addrs = self
            .resolver
            .lookup(host)
            .await
            .map_err(|_| EgressError::DnsFailed)?;
        if addrs.is_empty() {
            return Err(EgressError::DnsFailed);
        }
        for addr in &addrs {
            let refused = classify_ip(*addr);
            match refused {
                Ok(()) => {}
                Err(range) => {
                    if self.test_socket_allows(*addr, url) {
                        continue;
                    }
                    return Err(EgressError::AddressRefused(range));
                }
            }
        }
        Ok(addrs)
    }

    /// True when `addr` is the test socket and the URL targets its port.
    fn test_socket_allows(&self, addr: IpAddr, url: &Url) -> bool {
        #[cfg(feature = "test-policy")]
        {
            let Some(t) = &self.test else {
                return false;
            };
            let Some(socket) = t.allow_socket else {
                return false;
            };
            addr == socket.ip() && (url.port().is_none() || url.port() == Some(socket.port()))
        }
        #[cfg(not(feature = "test-policy"))]
        {
            let _ = (addr, url);
            false
        }
    }

    /// Log a one-click outcome through T-307 (route `egress.one_click`).
    fn log_one_click(&self, outcome: OneClickOutcome, latency: Duration) {
        let (code, status) = match outcome {
            OneClickOutcome::Accepted { status } => ("success", Some(status)),
            OneClickOutcome::Redirected { status } => ("redirected", Some(status)),
            OneClickOutcome::Rejected { status } => ("rejected", Some(status)),
            OneClickOutcome::TimedOut => ("timed_out", None),
        };
        op_log(&OpLog {
            op: "egress.one_click",
            outcome: code,
            status,
            latency_ms: Some(latency.as_millis() as u64),
        });
    }

    /// Log a refused `call` (T-307, route `egress.call`, outcome `host_refused`).
    fn log_call_refused(&self) {
        op_log(&OpLog {
            op: "egress.call",
            outcome: "host_refused",
            status: None,
            latency_ms: None,
        });
    }

    /// Log a completed `call` (T-307).
    fn log_call(&self, status: u16, latency: Duration) {
        let code = if (200..300).contains(&status) {
            "success"
        } else {
            "failure"
        };
        op_log(&OpLog {
            op: "egress.call",
            outcome: code,
            status: Some(status),
            latency_ms: Some(latency.as_millis() as u64),
        });
    }

    /// TLS failures are security events (ASVS V16.3.4).
    fn log_tls_failure(&self) {
        security_event(&SecurityEvent {
            action: "egress_tls_failure",
            outcome: "tls_failure",
            user: None,
            request_id: None,
            amr: None,
            provider: None,
            method: None,
        });
    }
}

/// Classify a reqwest send error into an egress error, walking the source chain
/// so a TLS/rustls certificate failure is reported distinctly from a plain
/// connect error. Timeout detection is done by the caller before this.
fn classify_send_error(err: &reqwest::Error) -> EgressError {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        let message = e.to_string().to_ascii_lowercase();
        if message.contains("certificate")
            || message.contains("tls")
            || message.contains("handshake")
            || message.contains("rustls")
        {
            return EgressError::Tls;
        }
        current = e.source();
    }
    EgressError::Connect
}

/// A reqwest request builder for a method. Exhaustive: no `_` arm.
fn method_builder(
    client: &reqwest::Client,
    method: HttpMethod,
    url: Url,
) -> reqwest::RequestBuilder {
    match method {
        HttpMethod::Get => client.get(url),
        HttpMethod::Post => client.post(url),
        HttpMethod::Put => client.put(url),
        HttpMethod::Patch => client.patch(url),
        HttpMethod::Delete => client.delete(url),
    }
}

/// Read and discard up to `cap` bytes of a one-click response body.
async fn drain_capped(resp: reqwest::Response, cap: usize) -> Result<(), EgressError> {
    if let Some(len) = resp.content_length() {
        if len > cap as u64 {
            return Ok(());
        }
    }
    let _ = resp.bytes().await.map_err(|_| EgressError::Connect)?;
    Ok(())
}

/// Read a `call` response body, refusing bodies larger than `cap`.
async fn read_capped(resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, EgressError> {
    if let Some(len) = resp.content_length() {
        if len > cap as u64 {
            return Err(EgressError::ResponseTooLarge);
        }
    }
    let body = resp
        .bytes()
        .await
        .map_err(|_| EgressError::Connect)?
        .to_vec();
    if body.len() > cap {
        return Err(EgressError::ResponseTooLarge);
    }
    Ok(body)
}

#[async_trait]
impl HttpEgress for ProdEgress {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError> {
        let started = Instant::now();
        if !one_click_permitted(self.service) {
            return Err(EgressError::NotPermitted);
        }
        let (host, https_only, pin_port) = self.one_click_target(url)?;
        let addrs = self.one_click_resolve(&host, url).await?;
        let pinned = SocketAddr::from((addrs[0], pin_port));
        let timeout = self.one_click_timeout();
        let client = self.request_client(&host, pinned, https_only, timeout)?;

        // Normalise the request URL's host so the pin covers it exactly (this
        // also drops any trailing dot, so SNI and the hostname check use `host`).
        let mut request_url = url.clone();
        request_url
            .set_host(Some(&host))
            .map_err(|_| EgressError::HostNotAllowed)?;

        let response = client
            .post(request_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(ONE_CLICK_BODY)
            .send()
            .await;

        let outcome = match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                drain_capped(resp, ONE_CLICK_MAX_BODY).await?;
                match status {
                    200..=299 => OneClickOutcome::Accepted { status },
                    300..=399 => OneClickOutcome::Redirected { status },
                    _ => OneClickOutcome::Rejected { status },
                }
            }
            Err(err) => {
                if err.is_timeout() {
                    OneClickOutcome::TimedOut
                } else {
                    let classified = classify_send_error(&err);
                    if classified == EgressError::Tls {
                        self.log_tls_failure();
                    }
                    return Err(classified);
                }
            }
        };
        self.log_one_click(outcome, started.elapsed());
        Ok(outcome)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        let started = Instant::now();
        if req.url.scheme() != "https" {
            return Err(EgressError::SchemeNotAllowed);
        }
        if !allows(self.service, &req.url) {
            self.log_call_refused();
            return Err(EgressError::HostNotAllowed);
        }
        let path = req.url.path();
        let host_matches_email = req.url.host_str() == Some("gmail.googleapis.com");
        let is_gmail_delete = host_matches_email && req.method == HttpMethod::Delete;
        let is_batch_delete = path.ends_with("/batchDelete");
        if is_gmail_delete || is_batch_delete {
            return Err(EgressError::PermanentDeleteRefused);
        }
        let Some(host) = req.url.host_str() else {
            return Err(EgressError::HostNotAllowed);
        };
        let host = host.to_ascii_lowercase();

        let timeout = req.timeout.min(CALL_MAX_TIMEOUT);
        let (pinned, https_only) = match self.host_route(&host) {
            Some(socket) => (socket, false),
            None => {
                let addrs = self
                    .resolver
                    .lookup(&host)
                    .await
                    .map_err(|_| EgressError::DnsFailed)?;
                if addrs.is_empty() {
                    return Err(EgressError::DnsFailed);
                }
                for addr in &addrs {
                    if let Err(range) = classify_ip(*addr) {
                        return Err(EgressError::AddressRefused(range));
                    }
                }
                (SocketAddr::from((addrs[0], ONE_CLICK_PORT)), true)
            }
        };

        let client = self.request_client(&host, pinned, https_only, timeout)?;
        let mut request_url = req.url.clone();
        if !https_only {
            // A host-routes override sends the allowlisted host to a local
            // socket over plain http.
            request_url
                .set_scheme("http")
                .map_err(|()| EgressError::Connect)?;
        }
        request_url
            .set_host(Some(&host))
            .map_err(|_| EgressError::HostNotAllowed)?;
        let mut builder = method_builder(&client, req.method, request_url);
        for (key, value) in &req.headers {
            builder = builder.header(key, value.expose());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }

        let response = builder.send().await;
        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let headers = resp
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
                    .collect();
                let body = read_capped(resp, CALL_MAX_BODY).await?;
                self.log_call(status, started.elapsed());
                Ok(EgressResponse {
                    status,
                    headers,
                    body,
                })
            }
            Err(err) => {
                let classified = if err.is_timeout() {
                    EgressError::Timeout
                } else {
                    classify_send_error(&err)
                };
                if classified == EgressError::Tls {
                    self.log_tls_failure();
                }
                Err(classified)
            }
        }
    }
}

/// A host-routes override for `call`, when the `test-policy` feature is on.
#[cfg(feature = "test-policy")]
impl ProdEgress {
    fn host_route(&self, host: &str) -> Option<SocketAddr> {
        self.test.as_ref().and_then(|t| {
            t.host_routes
                .iter()
                .find(|(h, _)| *h == host)
                .map(|(_, s)| *s)
        })
    }
}

#[cfg(not(feature = "test-policy"))]
impl ProdEgress {
    fn host_route(&self, _host: &str) -> Option<SocketAddr> {
        None
    }

    fn attach_test_ca(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder, EgressError> {
        Ok(builder)
    }
}
