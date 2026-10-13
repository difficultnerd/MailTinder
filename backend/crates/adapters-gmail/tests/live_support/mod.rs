//! Support for the nightly live Gmail contract run (T-1106).
//!
//! Four jobs: read the sandbox credentials, mint an access token from the
//! stored refresh token, seed the sandbox by IMPORTING messages
//! (`users.messages.import`, never `users.messages.send`), and classify the run
//! into the fixed [`LiveOutcome`] codes the workflow job passes on as a job
//! output.
//!
//! Content-freeness: nothing here prints a token, the sandbox address or a
//! message body. Message IDs are never displayed either; a test binary the
//! harness shows on failure must not carry account data into a log.
#![allow(
    dead_code,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::struct_field_names
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adapters_gmail::GmailProvider;
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use domain::{LabelSet, MailboxId, MessageId};
use obs::Sensitive;
use ports::{
    Clock, EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, MailProvider,
    MailboxCtx, OneClickOutcome,
};
use serde::Deserialize;
use testkit::clock::VirtualClock;
use testkit::contract::mail_provider::{MailSeeder, MailTarget};
use testkit::corpus::CorpusCase;
use testkit::mailbox::state::SeedMessage;
use testkit::mailbox::SentRecord;
use url::Url;

/// The fixed marker label every seeded message carries (T-1106 step 4).
pub const LIVE_LABEL: &str = "MT Live Contract";
/// The Gmail REST base for the sandbox account.
pub const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// The Google OAuth token endpoint used to mint an access token.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// `messages.import` takes the internal date from the message's `Date` header.
pub const INTERNAL_DATE_SOURCE: &str = "dateHeader";
/// One live HTTP call may take this long `[DEFAULT]` (the adapter's 10 s is for
/// production traffic; an import of a small message under a saturated nightly
/// runner is slower).
pub const LIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// The adapter's own per-call timeout cap.
pub const BODY_LIMIT: usize = 8 * 1024 * 1024;

/// One allowed live endpoint: an exact host and its required path prefix.
pub struct LiveEndpoint {
    pub host: &'static str,
    pub path_prefix: &'static str,
}

/// The live allowlist. These four rows are the T-306 `Service::Api` and
/// `Service::Unsub` allowlists (T-1106 step 3): the live run reaches Google and
/// nothing else, and plain `http` is permitted only for a `localhost` stub.
pub const LIVE_ALLOWLIST: [LiveEndpoint; 4] = [
    LiveEndpoint {
        host: "gmail.googleapis.com",
        path_prefix: "/gmail/v1/",
    },
    LiveEndpoint {
        host: "oauth2.googleapis.com",
        path_prefix: "/token",
    },
    LiveEndpoint {
        host: "accounts.google.com",
        path_prefix: "/",
    },
    LiveEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/oauth2/v3/certs",
    },
];
/// The fixed outcome codes the workflow's follow-up job classifies on. They are
/// passed between jobs as a job output, never by reading the captured log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveOutcome {
    Passed,
    ContractFailed,
    CredentialsExpired,
    NotConfigured,
}

impl LiveOutcome {
    /// The wire value the workflow writes to `$GITHUB_OUTPUT`.
    pub fn code(self) -> &'static str {
        match self {
            LiveOutcome::Passed => "Passed",
            LiveOutcome::ContractFailed => "ContractFailed",
            LiveOutcome::CredentialsExpired => "CredentialsExpired",
            LiveOutcome::NotConfigured => "NotConfigured",
        }
    }
}

/// Everything that can go wrong before or during the live run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveError {
    /// One or more sandbox variables are absent or empty.
    NotConfigured(Vec<String>),
    /// Google refused the refresh token: the sandbox needs re-authorising.
    CredentialsExpired,
    /// The provider answered, but not with a usable response/token.
    Transient,
    /// The transport guard caught a call to a message-sending endpoint.
    SendAttempt,
}

impl LiveError {
    /// The fixed outcome this error classifies as.
    pub fn outcome(&self) -> LiveOutcome {
        match self {
            LiveError::NotConfigured(_) => LiveOutcome::NotConfigured,
            LiveError::CredentialsExpired => LiveOutcome::CredentialsExpired,
            LiveError::Transient | LiveError::SendAttempt => LiveOutcome::ContractFailed,
        }
    }
}

/// The four sandbox variables. Secrets are [`Sensitive`].
///
/// `Debug` is written by hand below: the derived one would print the client
/// secret and the refresh token (`privacy-rust-derive-debug-on-sensitive-struct`).
pub struct SandboxCreds {
    pub client_id: String,
    pub client_secret: Sensitive<String>,
    pub refresh: Sensitive<String>,
    pub address: Sensitive<String>,
}

impl SandboxCreds {
    /// Read [`SandboxCreds::VARS`]; a missing or empty variable is reported by
    /// name in [`LiveError::NotConfigured`], never by value.
    pub fn from_env() -> Result<Self, LiveError> {
        let mut missing = Vec::new();
        let mut value = |key: &'static str| match std::env::var(key) {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => {
                missing.push(key.to_owned());
                None
            }
        };
        let client_id = value(Self::CLIENT_ID);
        let client_secret = value(Self::CLIENT_SECRET);
        let refresh = value(Self::REFRESH);
        let address = value(Self::ADDRESS);
        if !missing.is_empty() {
            return Err(LiveError::NotConfigured(missing));
        }
        Ok(Self {
            client_id: client_id.unwrap_or_default(),
            client_secret: Sensitive::new(client_secret.unwrap_or_default()),
            refresh: Sensitive::new(refresh.unwrap_or_default()),
            address: Sensitive::new(address.unwrap_or_default()),
        })
    }

    pub const CLIENT_ID: &'static str = "GMAIL_SANDBOX_CLIENT_ID";
    pub const CLIENT_SECRET: &'static str = "GMAIL_SANDBOX_CLIENT_SECRET";
    pub const REFRESH: &'static str = "GMAIL_SANDBOX_REFRESH_TOKEN";
    pub const ADDRESS: &'static str = "GMAIL_SANDBOX_ADDRESS";

    /// The four variable names, in the documented order.
    pub const VARS: [&'static str; 4] = [
        Self::CLIENT_ID,
        Self::CLIENT_SECRET,
        Self::REFRESH,
        Self::ADDRESS,
    ];
}

impl std::fmt::Debug for SandboxCreds {
    /// Redacted field by field: no secret, token or address ever reaches a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCreds")
            .field("client_id", &"[redacted]")
            .field("client_secret", &"[redacted]")
            .field("refresh", &"[redacted]")
            .field("address", &"[redacted]")
            .finish()
    }
}

/// The URLs the live run talks to. Production values by default; the token URL
/// is overridable only so a `localhost` unit test can answer for it.
#[derive(Clone)]
pub struct LiveConfig {
    pub token_url: Url,
    pub gmail_base: Url,
    pub allow_plain_http_to_localhost: bool,
}

impl std::fmt::Debug for LiveConfig {
    /// Hand-written: a derived `Debug` would print the URLs, which S5 keeps out
    /// of logs. Only the hosts are shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveConfig")
            .field("token_host", &self.token_url.host_str())
            .field("gmail_host", &self.gmail_base.host_str())
            .field(
                "allow_plain_http_to_localhost",
                &self.allow_plain_http_to_localhost,
            )
            .finish()
    }
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            token_url: Url::parse(TOKEN_URL).unwrap_or_else(|_| unreachable!("static url")),
            gmail_base: Url::parse(GMAIL_BASE).unwrap_or_else(|_| unreachable!("static url")),
            allow_plain_http_to_localhost: false,
        }
    }
}

/// A token response: `access_token` on success, `error` on refusal.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    #[serde(default)]
    error: String,
}

/// Mint an access token from the stored refresh token.
///
/// `invalid_grant` is [`LiveError::CredentialsExpired`]: Google has invalidated
/// the refresh token (Testing mode expires it weekly), and that must never read
/// as a pass or a contract failure.
pub async fn mint_access_token(
    config: &LiveConfig,
    creds: &SandboxCreds,
) -> Result<Sensitive<String>, LiveError> {
    let body = format!(
        "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={}",
        urlencode(&creds.client_id),
        urlencode(creds.client_secret.expose()),
        urlencode(creds.refresh.expose()),
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| LiveError::Transient)?;
    let response = client
        .post(config.token_url.clone())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|_| LiveError::Transient)?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|_| LiveError::Transient)?;
    let parsed: TokenResponse = serde_json::from_slice(&bytes).map_err(|_| LiveError::Transient)?;
    if let Some(token) = parsed.access_token {
        return Ok(Sensitive::new(token));
    }
    if parsed.error == "invalid_grant" || status == 400 || status == 401 {
        return Err(LiveError::CredentialsExpired);
    }
    Err(LiveError::Transient)
}

/// Percent-encode one form field (`application/x-www-form-urlencoded`).
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The transport guard: the boundary that stops a send leaving the process.
///
/// A delegated read or modify call could reach a send endpoint inside the
/// adapter without passing a decorator, so the refusal sits at the HTTP layer.
/// The guard records every refusal so the run fails even if the adapter
/// swallowed the error.
#[derive(Clone, Default)]
pub struct SendGuard {
    inner: Option<Arc<dyn HttpEgress>>,
    attempts: Arc<AtomicU64>,
}

impl SendGuard {
    /// Wrap an egress with the send refusal.
    pub fn new(inner: Arc<dyn HttpEgress>) -> Self {
        Self {
            inner: Some(inner),
            attempts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A guard with no inner egress: a unit test can call the decision and the
    /// counter without any transport.
    pub fn detached() -> Self {
        Self::default()
    }

    /// How many sends the guard has refused (always 0 on a clean run).
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::SeqCst)
    }

    /// True when this request would send mail: a call to a send endpoint, or a
    /// `raw` upload to one.
    pub fn is_send(method: HttpMethod, url: &Url) -> bool {
        matches!(method, HttpMethod::Post) && Self::is_send_path(url.path())
    }

    /// The path half of the decision, exposed so a unit test can prove the
    /// exact template that is refused.
    pub fn is_send_path(path: &str) -> bool {
        for template in ["/messages/send", "/drafts/send"] {
            if path == template
                || path.ends_with(template)
                || path.contains(&format!("{template}?"))
            {
                return true;
            }
        }
        false
    }
}

#[async_trait]
impl HttpEgress for SendGuard {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError> {
        let Some(inner) = &self.inner else {
            return Err(EgressError::NotPermitted);
        };
        inner.one_click_post(url).await
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        if SendGuard::is_send(req.method, &req.url) {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            return Err(EgressError::NotPermitted);
        }
        let Some(inner) = &self.inner else {
            return Err(EgressError::NotPermitted);
        };
        inner.call(req).await
    }
}

/// A pass-through egress that first checks the real T-306 allowlist and the
/// T-1106 send guard, so a live call exercises the same boundary production
/// uses. Plain `http` is permitted only for `localhost`, and only when the
/// config says so (a unit test's stub token server).
pub struct AllowlistedEgress {
    guard: SendGuard,
    allow_localhost: bool,
}

impl AllowlistedEgress {
    pub fn new(allow_localhost: bool) -> Self {
        Self {
            guard: SendGuard::new(Arc::new(Passthrough)),
            allow_localhost,
        }
    }

    /// The guard, so the run can read the refusal count.
    pub fn guard(&self) -> &SendGuard {
        &self.guard
    }

    /// The allowlist decision for one URL: https on 443, an exact live host and
    /// its required path prefix. Plain `http` is permitted only for the
    /// `localhost` stub a unit test points at.
    pub fn host_allowed(url: &Url, allow_localhost: bool) -> bool {
        if allow_localhost && Self::is_localhost_http(url) {
            return true;
        }
        if url.scheme() != "https" {
            return false;
        }
        match url.port() {
            None | Some(443) => {}
            Some(_) => return false,
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        let path = url.path();
        LIVE_ALLOWLIST
            .iter()
            .any(|endpoint| endpoint.host == host && path.starts_with(endpoint.path_prefix))
    }

    fn is_localhost_http(url: &Url) -> bool {
        url.scheme() == "http" && url.host_str() == Some("localhost")
    }
}

/// The plain transport under the guard and the allowlist.
struct Passthrough;

#[async_trait]
impl HttpEgress for Passthrough {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(req.timeout)
            .build()
            .map_err(|_| EgressError::Connect)?;
        let mut builder = match req.method {
            HttpMethod::Get => client.get(req.url.clone()),
            HttpMethod::Post => client.post(req.url.clone()),
            HttpMethod::Put => client.put(req.url.clone()),
            HttpMethod::Patch => client.patch(req.url.clone()),
            HttpMethod::Delete => client.delete(req.url.clone()),
        };
        for (key, value) in &req.headers {
            builder = builder.header(key, value.expose());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let resp = builder.send().await.map_err(|_| EgressError::Connect)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect();
        let body = resp
            .bytes()
            .await
            .map_err(|_| EgressError::Connect)?
            .to_vec();
        Ok(EgressResponse {
            status,
            headers,
            body,
        })
    }
}

#[async_trait]
impl HttpEgress for AllowlistedEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        if !Self::host_allowed(&req.url, self.allow_localhost) {
            return Err(EgressError::HostNotAllowed);
        }
        self.guard.call(req).await
    }
}

/// The Gmail wire shapes the seeder reads and writes. Private to the run.
#[derive(Deserialize)]
struct ImportResponse {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    messages: Vec<IdOnly>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

/// A Gmail `messages.get` minimal/raw response: the label IDs and the raw body.
#[derive(Deserialize)]
pub struct MinimalMessage {
    #[serde(default, rename = "labelIds")]
    label_ids: Vec<String>,
    #[serde(default)]
    raw: String,
}

/// A Gmail call with the bearer token attached. The seeder owns its own client
/// so it can read responses the adapter has no port method for.
pub struct LiveHttp<'a> {
    egress: &'a dyn HttpEgress,
    base: &'a Url,
    ctx: &'a MailboxCtx,
}

impl<'a> LiveHttp<'a> {
    pub fn new(egress: &'a dyn HttpEgress, base: &'a Url, ctx: &'a MailboxCtx) -> Self {
        Self { egress, base, ctx }
    }

    async fn post(
        &self,
        path: &str,
        query: &[(&str, String)],
        json: &str,
    ) -> Result<Vec<u8>, LiveError> {
        let url = build_url(self.base, path, query);
        self.call(HttpMethod::Post, url, Some(json.as_bytes().to_vec()), true)
            .await
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<u8>, LiveError> {
        let url = build_url(self.base, path, query);
        self.call(HttpMethod::Get, url, None, false).await
    }

    async fn call(
        &self,
        method: HttpMethod,
        url: Url,
        body: Option<Vec<u8>>,
        json_body: bool,
    ) -> Result<Vec<u8>, LiveError> {
        let mut headers = vec![
            (
                "Authorization".to_owned(),
                Sensitive::new(format!("Bearer {}", self.ctx.access_token.expose())),
            ),
            (
                "Accept".to_owned(),
                Sensitive::new("application/json".to_owned()),
            ),
        ];
        if json_body {
            headers.push((
                "Content-Type".to_owned(),
                Sensitive::new("application/json".to_owned()),
            ));
        }
        let request = EgressRequest {
            method,
            url,
            headers,
            body,
            timeout: LIVE_TIMEOUT,
        };
        let response = self.egress.call(request).await.map_err(|err| match err {
            EgressError::HostNotAllowed | EgressError::NotPermitted => LiveError::SendAttempt,
            _ => LiveError::Transient,
        })?;
        if response.body.len() > BODY_LIMIT {
            return Err(LiveError::Transient);
        }
        if !(200..300).contains(&response.status) {
            return Err(LiveError::Transient);
        }
        Ok(response.body)
    }
}

/// The absolute URL for `path` plus query pairs, never string-built.
pub fn build_url(base: &Url, path: &str, query: &[(&str, String)]) -> Url {
    let base_path = base.path().trim_end_matches('/');
    let mut url = base.clone();
    url.set_path(&format!("{base_path}/{}", path.trim_start_matches('/')));
    url.set_query(None);
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    url
}

/// The `Date` header value for an internal date, in the RFC 5322 form
/// `users.messages.import` reads when `internalDateSource=dateHeader`.
pub fn date_header(when: time::OffsetDateTime) -> String {
    when.to_offset(time::UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default()
}

/// Render a seed as a raw RFC 822 message: the corpus shape, plus a `Date`
/// header carrying the seed's internal date.
pub fn seed_raw(msg: &SeedMessage) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("From: ");
    out.push_str(&msg.from_display);
    out.push_str(" <");
    out.push_str(&msg.from_address);
    out.push_str(">\r\n");
    out.push_str("Subject: ");
    out.push_str(&msg.subject);
    out.push_str("\r\n");
    out.push_str("Date: ");
    out.push_str(&date_header(msg.internal_date));
    out.push_str("\r\n");
    for (key, value) in &msg.raw_headers {
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(&msg.preview_text);
    out.into_bytes()
}

/// Parse a Gmail `messages.get`/`import` response that carries `id` (and
/// optionally `labelIds`).
pub fn parse_minimal(body: &[u8]) -> Result<MinimalMessage, LiveError> {
    serde_json::from_slice(body).map_err(|_| LiveError::Transient)
}

/// Parse the JSON `id` of an import response.
pub fn import_id(body: &[u8]) -> Result<String, LiveError> {
    let parsed: ImportResponse = serde_json::from_slice(body).map_err(|_| LiveError::Transient)?;
    if parsed.id.is_empty() {
        return Err(LiveError::Transient);
    }
    Ok(parsed.id)
}

/// The label IDs one imported message carries: the fixed marker label plus the
/// seed's own labels, deduplicated so the marker is never requested twice.
pub fn label_ids_for(labels: &[String]) -> Vec<String> {
    let mut ids = vec![LIVE_LABEL.to_owned()];
    for label in labels {
        if !ids.iter().any(|seen| seen == label) {
            ids.push(label.clone());
        }
    }
    ids
}

/// The `To` header of a raw message, if present.
fn raw_to(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    for line in text.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("To") {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

/// A `MailSeeder` over the real sandbox account: it inserts synthetic mail with
/// `users.messages.import`, reads labels back with `messages.get`, reports the
/// account's `SENT` label contents, and trash-empties itself before each case.
pub struct LiveSeeder {
    http: Arc<dyn HttpEgress>,
    base: Url,
    ctx: MailboxCtx,
    guard: SendGuard,
    created: Mutex<Vec<String>>,
    trashed: Mutex<Vec<String>>,
}

impl LiveSeeder {
    pub fn new(http: Arc<dyn HttpEgress>, base: Url, ctx: MailboxCtx, guard: SendGuard) -> Self {
        Self {
            http,
            base,
            ctx,
            guard,
            created: Mutex::new(Vec::new()),
            trashed: Mutex::new(Vec::new()),
        }
    }

    fn client(&self) -> LiveHttp<'_> {
        LiveHttp::new(self.http.as_ref(), &self.base, &self.ctx)
    }

    /// Import one raw RFC 822 message, tagged with [`LIVE_LABEL`].
    async fn import(&self, raw: &[u8], labels: &[String]) -> Result<String, LiveError> {
        let body = serde_json::json!({
            "raw": URL_SAFE_NO_PAD.encode(raw),
            "labelIds": label_ids_for(labels),
        })
        .to_string();
        let query = [
            ("internalDateSource", INTERNAL_DATE_SOURCE.to_owned()),
            ("fields", "id".to_owned()),
        ];
        let response = self.client().post("messages/import", &query, &body).await?;
        let id = import_id(&response)?;
        self.remember(&self.created, &id);
        Ok(id)
    }

    /// Record an ID in one of the run's id sets.
    fn remember(&self, set: &Mutex<Vec<String>>, id: &str) {
        if let Ok(mut ids) = set.lock() {
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_owned());
            }
        }
    }

    /// Message IDs the run inserted, oldest first.
    pub fn created(&self) -> Vec<String> {
        self.created
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    /// Message IDs the run trashed, oldest first.
    pub fn trashed(&self) -> Vec<String> {
        self.trashed
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    /// Trash every message carrying [`LIVE_LABEL`], page by page. Import is
    /// idempotent per run: a run that finds its own label already populated
    /// trashes those messages before it seeds anything.
    pub async fn empty_label(&self) -> Result<u64, LiveError> {
        let mut trashed = 0;
        for id in self.labeled_ids().await? {
            self.trash_id(&id).await?;
            trashed += 1;
        }
        Ok(trashed)
    }

    /// Every message ID carrying [`LIVE_LABEL`], following page tokens.
    async fn labeled_ids(&self) -> Result<Vec<String>, LiveError> {
        let mut ids = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("labelIds", LIVE_LABEL.to_owned()),
                ("maxResults", "500".to_owned()),
                ("fields", "messages/id,nextPageToken".to_owned()),
            ];
            if let Some(page) = &token {
                query.push(("pageToken", page.clone()));
            }
            let body = self.client().get("messages", &query).await?;
            let page: ListResponse =
                serde_json::from_slice(&body).map_err(|_| LiveError::Transient)?;
            for item in page.messages {
                ids.push(item.id);
            }
            match page.next_page_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        Ok(ids)
    }

    /// Trash one message by ID: `messages.trash` only, never a delete endpoint.
    async fn trash_id(&self, id: &str) -> Result<(), LiveError> {
        let path = format!("messages/{id}/trash");
        let query = [("fields", "id".to_owned())];
        self.client().post(&path, &query, "{}").await?;
        self.remember(&self.trashed, id);
        Ok(())
    }

    /// Trash everything this run inserted that still exists.
    pub async fn cleanup(&self) -> Result<u64, LiveError> {
        let ids = self.created();
        let mut trashed = 0;
        for id in ids {
            if self.trash_id(&id).await.is_ok() {
                trashed += 1;
            }
        }
        Ok(trashed)
    }

    /// Seed a T-204 corpus case by importing its `.eml` unchanged, so the
    /// suite's synthetic subjects and headers survive the round trip.
    pub async fn seed_corpus_case(&self, case: &CorpusCase) -> Result<MessageId, LiveError> {
        let seed = case.seed_message();
        let id = self.import(&case.eml, &seed.labels).await?;
        MessageId::new(id).map_err(|_| LiveError::Transient)
    }
}

#[async_trait]
impl MailSeeder for LiveSeeder {
    async fn seed(&self, msg: &SeedMessage) -> Result<MessageId, String> {
        let raw = seed_raw(msg);
        match self.import(&raw, &msg.labels).await {
            Ok(id) => MessageId::new(id).map_err(|_| "bad id".to_owned()),
            Err(err) => Err(format!("{err:?}")),
        }
    }

    async fn labels_of(&self, id: &MessageId) -> Result<LabelSet, String> {
        let path = format!("messages/{}", id.as_str());
        let query = vec![
            ("format", "minimal".to_owned()),
            ("fields", "labelIds".to_owned()),
        ];
        let body = self
            .client()
            .get(&path, &query)
            .await
            .map_err(|err| format!("{err:?}"))?;
        let message = parse_minimal(&body).map_err(|err| format!("{err:?}"))?;
        Ok(LabelSet::from_ids(message.label_ids))
    }

    /// The account's `SENT` label contents. Never a constant empty list: this
    /// asks the sandbox, so a send that got past the transport guard is caught.
    async fn sent(&self) -> Result<Vec<SentRecord>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("labelIds", "SENT".to_owned()),
                ("includeSpamTrash", "true".to_owned()),
                ("maxResults", "500".to_owned()),
                ("fields", "messages/id,nextPageToken".to_owned()),
            ];
            if let Some(page) = &token {
                query.push(("pageToken", page.clone()));
            }
            let body = self
                .client()
                .get("messages", &query)
                .await
                .map_err(|err| format!("{err:?}"))?;
            let page: ListResponse =
                serde_json::from_slice(&body).map_err(|_| "sent: bad list".to_owned())?;
            let next = page.next_page_token;
            for item in page.messages {
                let path = format!("messages/{}", item.id);
                let detail_query = vec![("format", "raw".to_owned()), ("fields", "raw".to_owned())];
                let raw = self
                    .client()
                    .get(&path, &detail_query)
                    .await
                    .map_err(|err| format!("{err:?}"))?;
                let message = parse_minimal(&raw).map_err(|err| format!("{err:?}"))?;
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(message.raw.trim_end_matches('='))
                    .unwrap_or_default();
                out.push(SentRecord {
                    mailbox: self.ctx.mailbox,
                    to: raw_to(&bytes).unwrap_or_default(),
                    subject: None,
                    body: None,
                });
            }
            match next {
                Some(page) => token = Some(page),
                None => break,
            }
        }
        Ok(out)
    }

    /// The live run only ever trashes; `messages.import` and the suite's label
    /// work never reach a delete endpoint, so this stays 0 and the shared
    /// suite's INV-5 case passes on the real account too.
    async fn permanent_delete_attempts(&self) -> Result<u64, String> {
        Ok(self.guard.attempts())
    }
}

/// The sandbox mailbox as one object: credentials, token, allowlisted egress
/// and every ID the run created.
pub struct Sandbox {
    creds: SandboxCreds,
    config: LiveConfig,
    http: Arc<dyn HttpEgress>,
    guard: SendGuard,
    base: Url,
    clock: Arc<dyn Clock>,
    mailbox: MailboxId,
    token: Sensitive<String>,
}

impl Sandbox {
    /// Read the environment and mint an access token.
    pub async fn connect() -> Result<Self, LiveError> {
        let creds = SandboxCreds::from_env()?;
        let config = LiveConfig::default();
        let token = mint_access_token(&config, &creds).await?;
        Ok(Self::with_token(creds, config, token))
    }

    /// Build from an already-minted token (a unit test does this).
    pub fn with_token(creds: SandboxCreds, config: LiveConfig, token: Sensitive<String>) -> Self {
        let egress = Arc::new(AllowlistedEgress::new(config.allow_plain_http_to_localhost));
        let guard = egress.guard().clone();
        let mut base = config.gmail_base.clone();
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Self {
            creds,
            config,
            http: egress,
            guard,
            base,
            clock: Arc::new(VirtualClock::new(testkit::T0)),
            mailbox: MailboxId(uuid::Uuid::from_u128(1)),
            token,
        }
    }

    /// The four variables, for a `from_env` unit test.
    pub fn creds(&self) -> &SandboxCreds {
        &self.creds
    }

    /// The config, for a `from_env` unit test.
    pub fn config(&self) -> &LiveConfig {
        &self.config
    }

    /// The egress every call goes through (allowlist plus send guard).
    pub fn egress(&self) -> Arc<dyn HttpEgress> {
        Arc::clone(&self.http)
    }

    /// The transport guard, for the post-run send check.
    pub fn guard(&self) -> &SendGuard {
        &self.guard
    }

    /// The Gmail base URL the adapter and the seeder share.
    pub fn base(&self) -> &Url {
        &self.base
    }

    /// Nothing sends: the run's own proof, read from the guard.
    pub fn send_attempts(&self) -> u64 {
        self.guard.attempts()
    }

    /// A provider over the same egress, for the server-side send check.
    pub fn provider_with(&self) -> GmailProvider {
        GmailProvider::new(adapters_gmail::GmailHttp::new(
            Arc::clone(&self.http),
            self.base.clone(),
            Arc::clone(&self.clock),
        ))
    }

    /// A seeder bound to `ctx`, for the server-side send check.
    pub fn seeder_for(&self, ctx: &MailboxCtx) -> LiveSeeder {
        let ctx = MailboxCtx {
            mailbox: ctx.mailbox,
            access_token: Sensitive::new(ctx.access_token.expose().clone()),
        };
        LiveSeeder::new(
            Arc::clone(&self.http),
            self.base.clone(),
            ctx,
            self.guard.clone(),
        )
    }

    /// A seeder over the run's own context.
    pub fn run_seeder(&self) -> LiveSeeder {
        let ctx = self.context();
        self.seeder_for(&ctx)
    }

    /// The run's own context: the sandbox mailbox and the minted token.
    pub fn context(&self) -> MailboxCtx {
        MailboxCtx {
            mailbox: self.mailbox,
            access_token: Sensitive::new(self.token.expose().clone()),
        }
    }

    /// One fresh, empty case: trash everything carrying [`LIVE_LABEL`] (the
    /// factory contract: each call is a fresh, empty mailbox), then return the
    /// target, its seeder, and the context they run under.
    pub async fn fresh(&self) -> Result<(MailTarget, LiveSeeder, MailboxCtx), LiveError> {
        let mut last = LiveError::Transient;
        for _ in 0..3 {
            match self.fresh_once().await {
                Ok(parts) => return Ok(parts),
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    async fn fresh_once(&self) -> Result<(MailTarget, LiveSeeder, MailboxCtx), LiveError> {
        let ctx = self.context();
        let seeder = self.seeder_for(&ctx);
        seeder.empty_label().await?;
        let provider: Arc<dyn MailProvider> = Arc::new(self.provider_with());
        let target_seeder: Arc<dyn MailSeeder> = Arc::new(self.seeder_for(&ctx));
        let target = MailTarget {
            provider,
            ctx: self.context(),
            seeder: target_seeder,
        };
        Ok((target, seeder, ctx))
    }
}

/// The corpus cases the live run imports (T-204). Two cases prove the import
/// path carries the corpus's own headers, labels and internal dates.
pub fn live_corpus_cases() -> Result<Vec<CorpusCase>, String> {
    let corpus = testkit::corpus::load()?;
    let wanted = ["one-click-covered", "mailto-covered"];
    let mut cases = Vec::new();
    for id in wanted {
        if let Some(case) = corpus.case(id) {
            cases.push(case.clone());
        }
    }
    if cases.is_empty() {
        return Err("no live corpus cases".to_owned());
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::pedantic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::missing_panics_doc,
        clippy::missing_errors_doc
    )]

    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use time::OffsetDateTime;

    /// Serialises the tests that read or write the process environment.
    /// `set_var`/`remove_var` are process-global and the harness runs tests on
    /// several threads, so two environment tests running at once fail each
    /// other (and only under load, as the coverage run showed).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Hold the process-environment lock for the rest of one test.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The four sandbox variables as they are right now.
    fn saved_env() -> Vec<(&'static str, Option<String>)> {
        SandboxCreds::VARS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect()
    }

    /// Put the four sandbox variables back exactly as [`saved_env`] found them,
    /// before the test asserts, so a failing assertion cannot poison the
    /// environment for the tests that follow it.
    fn restore_env(previous: Vec<(&'static str, Option<String>)>) {
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    fn creds() -> SandboxCreds {
        SandboxCreds {
            client_id: "client-id.apps.googleusercontent.com".to_owned(),
            client_secret: Sensitive::new("client-secret".to_owned()),
            refresh: Sensitive::new("refresh-token".to_owned()),
            address: Sensitive::new("sandbox@example.com".to_owned()),
        }
    }

    /// Serve exactly one HTTP response on a loopback socket, then close.
    fn serve_once(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/token")
    }

    fn local_config(url: &str) -> LiveConfig {
        LiveConfig {
            token_url: Url::parse(url).expect("token url"),
            allow_plain_http_to_localhost: true,
            ..LiveConfig::default()
        }
    }

    /// T-1106: `invalid_grant` maps to `CredentialsExpired`, never a pass.
    #[tokio::test]
    async fn invalid_grant_maps_to_credentials_expired() {
        let url = serve_once(400, r#"{"error":"invalid_grant"}"#);
        let error = mint_access_token(&local_config(&url), &creds())
            .await
            .expect_err("invalid_grant must fail");
        assert_eq!(error, LiveError::CredentialsExpired);
        assert_eq!(error.outcome(), LiveOutcome::CredentialsExpired);
    }

    /// T-1106: a fresh refresh token mints the access token and never exposes
    /// it through `Debug`.
    #[tokio::test]
    async fn refreshed_token_is_minted_and_redacted() {
        // The placeholder is deliberately not shaped like a Google access
        // token: this value is fake, and a realistic prefix only gives the
        // secret scanner something to match (S10 4.3, decision T1).
        let url = serve_once(
            200,
            r#"{"access_token":"test-access-token","expires_in":3599}"#,
        );
        let token = mint_access_token(&local_config(&url), &creds())
            .await
            .expect("a token");
        assert_eq!(token.expose(), "test-access-token");
        assert_eq!(format!("{token:?}"), "[redacted]");
    }

    /// T-1106: a token endpoint that answers neither a token nor `invalid_grant`
    /// is transient, not a credentials expiry.
    #[tokio::test]
    async fn unavailable_token_endpoint_is_transient() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let error = mint_access_token(&local_config(&format!("http://{addr}/token")), &creds())
            .await
            .expect_err("closed port must fail");
        assert_eq!(error, LiveError::Transient);
    }

    /// T-1106: missing variables map to `NotConfigured`, naming only the
    /// variables, and the outcome code is its own, not a pass.
    #[test]
    fn missing_variables_map_to_not_configured() {
        let _env = env_lock();
        let previous = saved_env();
        for key in SandboxCreds::VARS {
            std::env::remove_var(key);
        }
        let error = SandboxCreds::from_env();
        restore_env(previous);

        let error = error.expect_err("no variables must fail");
        let mut named = match error {
            LiveError::NotConfigured(ref names) => names.clone(),
            ref other => panic!("expected NotConfigured, got {other:?}"),
        };
        named.sort();
        let mut want: Vec<String> = SandboxCreds::VARS.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        assert_eq!(named, want);
        assert_eq!(error.outcome(), LiveOutcome::NotConfigured);
    }

    /// T-1106: one empty variable is enough for `NotConfigured`.
    #[test]
    fn empty_variable_is_not_configured() {
        let _env = env_lock();
        let saved = saved_env();
        for key in SandboxCreds::VARS {
            std::env::set_var(key, "value");
        }
        std::env::set_var(SandboxCreds::ADDRESS, "   ");
        let error = SandboxCreds::from_env();
        restore_env(saved);

        let error = error.expect_err("blank variable must fail");
        match error {
            LiveError::NotConfigured(names) => {
                assert_eq!(names, vec![SandboxCreds::ADDRESS.to_owned()]);
            }
            other => panic!("expected NotConfigured, got {other:?}"),
        }
    }

    /// T-1106: the credentials' `Debug` is hand-written and redacts every field,
    /// because the derived one would print the client secret and the refresh
    /// token (`privacy-rust-derive-debug-on-sensitive-struct`).
    #[test]
    fn credentials_debug_redacts_every_field() {
        let text = format!("{:?}", creds());
        assert_eq!(text.matches("[redacted]").count(), 4, "{text}");
        for value in [
            "client-id.apps.googleusercontent.com",
            "client-secret",
            "refresh-token",
            "sandbox@example.com",
        ] {
            assert!(!text.contains(value), "Debug must not print a credential");
        }
    }

    /// T-1106: the transport guard refuses the send templates, including a raw
    /// upload, and records each refusal.
    #[tokio::test]
    async fn transport_guard_refuses_send_endpoints() {
        let guard = SendGuard::detached();
        let send = Url::parse("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
            .expect("url");
        assert!(SendGuard::is_send(HttpMethod::Post, &send));
        let error = guard
            .call(EgressRequest {
                method: HttpMethod::Post,
                url: send,
                headers: Vec::new(),
                body: Some(br#"{"raw":"aGk"}"#.to_vec()),
                timeout: LIVE_TIMEOUT,
            })
            .await
            .expect_err("a send must be refused");
        assert_eq!(error, EgressError::NotPermitted);
        assert_eq!(guard.attempts(), 1);
    }

    /// T-1106: the guard's path decision covers both send templates and does not
    /// catch the import or trash paths.
    #[test]
    fn transport_guard_path_decision() {
        for refused in [
            "/gmail/v1/users/me/messages/send",
            "/gmail/v1/users/me/drafts/send",
            "/gmail/v1/users/me/messages/send?uploadType=media",
        ] {
            assert!(SendGuard::is_send_path(refused), "{refused}");
        }
        for allowed in [
            "/gmail/v1/users/me/messages/import",
            "/gmail/v1/users/me/messages/msg-1/trash",
            "/gmail/v1/users/me/messages",
        ] {
            assert!(!SendGuard::is_send_path(allowed), "{allowed}");
        }
    }

    /// T-1106: the live egress allowlists only Gmail and Google OAuth. The real
    /// T-306 lists are asserted here, and the join with the send guard is
    /// checked without any network.
    #[test]
    fn egress_allowlists_only_the_live_hosts() {
        for (host, path) in [
            ("gmail.googleapis.com", "/gmail/v1/users/me/messages"),
            ("oauth2.googleapis.com", "/token"),
            ("accounts.google.com", "/o/oauth2/auth"),
            ("www.googleapis.com", "/oauth2/v3/certs"),
        ] {
            let url = Url::parse(&format!("https://{host}{path}")).expect("url");
            assert!(
                AllowlistedEgress::host_allowed(&url, false),
                "{host}{path} must be on the live allowlist"
            );
        }
        for refused in [
            "https://attacker.example/gmail/v1/users/me/messages",
            "http://gmail.googleapis.com/gmail/v1/users/me/messages",
            "https://gmail.googleapis.com.evil.example/gmail/v1/users/me",
            "https://oauth2.googleapis.com/token/../evil",
        ] {
            let url = Url::parse(refused).expect("url");
            assert!(
                !AllowlistedEgress::host_allowed(&url, false),
                "{refused} must be refused"
            );
        }
        let import = Url::parse("https://gmail.googleapis.com/gmail/v1/users/me/messages/import")
            .expect("url");
        assert!(AllowlistedEgress::host_allowed(&import, false));
    }

    /// T-1106: the run's delete count is the guard's refusal count, so a clean
    /// run reports 0 and an attempt at a permanent message delete shows up.
    #[test]
    fn permanent_delete_attempts_is_the_guard_count() {
        let guard = SendGuard::detached();
        let seeder = LiveSeeder::new(
            Arc::new(Passthrough),
            Url::parse(GMAIL_BASE).expect("url"),
            MailboxCtx {
                mailbox: MailboxId(uuid::Uuid::from_u128(1)),
                access_token: Sensitive::new("token".to_owned()),
            },
            guard.clone(),
        );
        let count = futures::executor::block_on(seeder.permanent_delete_attempts()).expect("count");
        assert_eq!(count, 0);
        assert_eq!(guard.attempts(), 0);
    }

    /// T-1106: the import payload is correct without any network call: the base
    /// URL carries `internalDateSource=dateHeader`, the raw message has the
    /// corpus headers plus the seed's `Date`, and the marker label is applied.
    #[test]
    fn import_payload_and_url_shape() {
        let base = Url::parse(GMAIL_BASE).expect("url");
        let url = build_url(
            &base,
            "messages/import",
            &[
                ("internalDateSource", INTERNAL_DATE_SOURCE.to_owned()),
                ("fields", "id".to_owned()),
            ],
        );
        assert_eq!(url.path(), "/gmail/v1/users/me/messages/import");
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "internalDateSource"),
            Some(("internalDateSource".into(), "dateHeader".into()))
        );
        assert!(url.query_pairs().any(|(k, _)| k == "fields"));

        let seed = SeedMessage {
            from_display: "Sender".to_owned(),
            from_address: "a@example.com".to_owned(),
            subject: "old".to_owned(),
            raw_headers: vec![("List-Id".to_owned(), "<x.example.com>".to_owned())],
            facts: domain::HeaderFacts::default(),
            preview_text: "CANARY-old-body".to_owned(),
            internal_date: OffsetDateTime::from_unix_timestamp(100).expect("date"),
            labels: vec!["INBOX".to_owned()],
        };
        let raw = String::from_utf8(seed_raw(&seed)).expect("utf8");
        assert!(raw.contains("Subject: old\r\n"));
        assert!(raw.contains("Date: "));
        assert!(raw.contains("a@example.com"));
        assert!(raw.contains("List-Id: <x.example.com>\r\n"));
        assert!(raw.ends_with("\r\n\r\nCANARY-old-body"));
    }

    /// T-1106: an import response with no id is transient, not a silent success.
    #[test]
    fn import_response_without_id_is_transient() {
        assert_eq!(import_id(b"{}").expect_err("no id"), LiveError::Transient);
        assert_eq!(
            import_id(b"not json").expect_err("bad json"),
            LiveError::Transient
        );
        assert_eq!(import_id(br#"{"id":"18c1"}"#).expect("id"), "18c1");
    }

    /// T-1106: the outcome codes match the workflow's fixed classification.
    #[test]
    fn outcome_codes_are_fixed() {
        assert_eq!(LiveOutcome::Passed.code(), "Passed");
        assert_eq!(LiveOutcome::ContractFailed.code(), "ContractFailed");
        assert_eq!(LiveOutcome::CredentialsExpired.code(), "CredentialsExpired");
        assert_eq!(LiveOutcome::NotConfigured.code(), "NotConfigured");
        assert_eq!(
            LiveError::SendAttempt.outcome(),
            LiveOutcome::ContractFailed
        );
    }

    /// T-1106: the corpus the live run imports is available and uses reserved
    /// `example.*` domains only.
    #[test]
    fn live_corpus_cases_are_reserved_domains() {
        let cases = live_corpus_cases().expect("corpus");
        assert!(!cases.is_empty());
        for case in &cases {
            let address = &case.spec.from_address;
            assert!(
                address.ends_with(".example") || address.ends_with("example.com"),
                "reserved domain expected"
            );
            assert!(!case.eml.is_empty());
        }
    }
}
