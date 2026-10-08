//! End-to-end harness library (dev-only; never a dependency of a service).
//!
//! The stack is brought up by `scripts/e2e.sh`, which exports the `MT_E2E_*`
//! URLs and starts ChromeDriver. Journeys then drive a real browser from
//! outside the page (full-page OAuth redirects end any in-page Dart test), find
//! controls by the `aria-label` on Flutter's `flt-semantics` nodes, and use the
//! fakes' control APIs to seed state (S10 3.2/3.3).
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use url::Url;

/// A harness error.
#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    /// A required environment variable is missing.
    #[error("missing environment variable {0}")]
    MissingEnv(&'static str),
    /// An environment variable is not a URL.
    #[error("invalid URL in {0}")]
    InvalidUrl(&'static str),
    /// The WebDriver command failed.
    #[error("webdriver: {0}")]
    WebDriver(String),
    /// The labelled control was not found.
    #[error("no control labelled or showing {0}")]
    NotFound(String),
    /// An HTTP call to a fake failed.
    #[error("http: {0}")]
    Http(String),
    /// A wait ran past its deadline.
    #[error("timed out waiting for {0}")]
    Timeout(String),
    /// The browser or a fake was in an unexpected state.
    #[error("state: {0}")]
    State(String),
}

fn wd(error: fantoccini::error::CmdError) -> E2eError {
    E2eError::WebDriver(error.to_string())
}

/// 2026-01-01T00:00:00Z: the deterministic base for seeded `internal_date`s.
/// The harness never reads the wall clock (S10 1 rule 2).
const BASE_UNIX_SECONDS: i64 = 1_767_225_600;

fn http(error: reqwest::Error) -> E2eError {
    E2eError::Http(error.to_string())
}

fn env_url(name: &'static str) -> Result<Url, E2eError> {
    let raw = std::env::var(name).map_err(|_| E2eError::MissingEnv(name))?;
    Url::parse(&raw).map_err(|_| E2eError::InvalidUrl(name))
}

/// An optional `MT_E2E_*` URL: absent or unparseable is treated as absent.
fn optional_url(name: &'static str) -> Option<Url> {
    std::env::var(name)
        .ok()
        .and_then(|raw| Url::parse(&raw).ok())
}

fn webdriver_url() -> Result<String, E2eError> {
    std::env::var("MT_E2E_WEBDRIVER_URL").map_err(|_| E2eError::MissingEnv("MT_E2E_WEBDRIVER_URL"))
}

fn endpoint(base: &Url, path: &str) -> String {
    format!("{}{}", base.as_str().trim_end_matches('/'), path)
}

/// The URLs of the running local stack.
pub struct Stack {
    /// The Flutter web build served with the `firebase.json` headers.
    pub app_url: Url,
    /// The fake Google (Gmail, Drive, OAuth, OIDC and `/__fake` control API).
    pub fake_google: Url,
    /// The one-click unsubscribe testbed.
    pub testbed: Url,
    /// The testbed's HTTPS listener, which a `List-Unsubscribe: <https://...>`
    /// header must target (S6 6 drops plain `http` links). Defaults to
    /// [`Stack::testbed`] when the HTTPS URL is not exported.
    pub testbed_https: Url,
    /// The api service, including its `testkit`-only `/internal/test` routes.
    pub api_internal: Url,
}

impl Stack {
    /// Read the stack URLs from the `MT_E2E_*` environment.
    pub fn from_env() -> Result<Self, E2eError> {
        let testbed = env_url("MT_E2E_TESTBED_URL")?;
        let testbed_https =
            optional_url("MT_E2E_TESTBED_HTTPS_URL").unwrap_or_else(|| testbed.clone());
        Ok(Self {
            app_url: env_url("MT_E2E_APP_URL")?,
            fake_google: env_url("MT_E2E_FAKE_GOOGLE_URL")?,
            testbed,
            testbed_https,
            api_internal: env_url("MT_E2E_API_URL")?,
        })
    }

    /// The absolute URL of `route` on the testbed's HTTPS listener; this is
    /// what a seeded `List-Unsubscribe` header points at for a one-click route.
    #[must_use]
    pub fn testbed_https_url(&self, route: &str) -> String {
        format!(
            "{}{route}",
            self.testbed_https.as_str().trim_end_matches('/')
        )
    }
}

/// A WebDriver browser session driving the Flutter web app.
pub struct Ui {
    client: fantoccini::Client,
}

impl Ui {
    /// Start a new Chrome session and open `hash_path` on the app.
    pub async fn open(stack: &Stack, hash_path: &str) -> Result<Self, E2eError> {
        let url = webdriver_url()?;
        let client = fantoccini::ClientBuilder::rustls()
            .map_err(|e| E2eError::WebDriver(e.to_string()))?
            .connect(&url)
            .await
            .map_err(|e| E2eError::WebDriver(e.to_string()))?;
        let target = format!(
            "{}{}",
            stack.app_url.as_str().trim_end_matches('/'),
            hash_path
        );
        client.goto(&target).await.map_err(wd)?;
        Ok(Self { client })
    }

    /// Click the `flt-semantics` control whose `aria-label` is `label`.
    pub async fn tap(&self, label: &str) -> Result<(), E2eError> {
        let clicked = self
            .client
            .execute(TAP_JS, vec![json!(label)])
            .await
            .map_err(wd)?;
        if clicked.as_bool() == Some(true) {
            Ok(())
        } else {
            Err(E2eError::NotFound(label.to_owned()))
        }
    }

    /// Type `text` into the labelled control.
    pub async fn type_into(&self, label: &str, text: &str) -> Result<(), E2eError> {
        let typed = self
            .client
            .execute(TYPE_JS, vec![json!(label), json!(text)])
            .await
            .map_err(wd)?;
        if typed.as_bool() == Some(true) {
            Ok(())
        } else {
            Err(E2eError::NotFound(label.to_owned()))
        }
    }

    /// Click the last `flt-semantics` control whose `aria-label` is `label`:
    /// the topmost one when a sheet or toast repeats a Feed button's label.
    pub async fn tap_last(&self, label: &str) -> Result<(), E2eError> {
        let clicked = self
            .client
            .execute(TAP_LAST_JS, vec![json!(label)])
            .await
            .map_err(wd)?;
        if clicked.as_bool() == Some(true) {
            Ok(())
        } else {
            Err(E2eError::NotFound(label.to_owned()))
        }
    }

    /// Poll until `text` appears in the page or the semantics tree.
    pub async fn wait_for_text(&self, text: &str, timeout: Duration) -> Result<(), E2eError> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self
                .client
                .execute(TEXT_JS, vec![json!(text)])
                .await
                .map_err(wd)?;
            if found.as_bool() == Some(true) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(E2eError::Timeout(text.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Poll until `text` no longer appears anywhere in the page.
    pub async fn wait_for_text_absent(
        &self,
        text: &str,
        timeout: Duration,
    ) -> Result<(), E2eError> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self
                .client
                .execute(TEXT_JS, vec![json!(text)])
                .await
                .map_err(wd)?;
            if found.as_bool() != Some(true) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(E2eError::Timeout(format!("{text} to disappear")));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Switch back to the first window (the app window, after a step-up popup).
    pub async fn switch_to_first_window(&self) -> Result<(), E2eError> {
        let handles = self.client.windows().await.map_err(wd)?;
        let first = handles
            .into_iter()
            .next()
            .ok_or_else(|| E2eError::State("no browser window".to_owned()))?;
        self.client.switch_to_window(first).await.map_err(wd)
    }

    /// Load the app again. A reload is the harness's stand-in for a Feed
    /// pull-to-refresh: the next Feed load collects stored unsubscribe outcomes
    /// (S10 6.3) that a real pointer gesture cannot reliably produce here.
    pub async fn reload(&self) -> Result<(), E2eError> {
        let url = self.client.current_url().await.map_err(wd)?;
        self.client.goto(url.as_str()).await.map_err(wd)
    }

    /// Switch to the most recently opened window (step-up popups; T-1001b).
    pub async fn switch_to_popup(&self) -> Result<(), E2eError> {
        let handles = self.client.windows().await.map_err(wd)?;
        let last = handles
            .into_iter()
            .last()
            .ok_or_else(|| E2eError::State("no browser window".to_owned()))?;
        self.client.switch_to_window(last).await.map_err(wd)
    }

    /// Persisted browser state: `localStorage`, `sessionStorage`, IndexedDB
    /// database names and Cache Storage keys, as JSON (leak scans; T-1101c).
    pub async fn storage_dump(&self) -> Result<String, E2eError> {
        let value = self
            .client
            .execute_async(STORAGE_JS, vec![])
            .await
            .map_err(wd)?;
        Ok(value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned))
    }

    /// Browser console entries at `SEVERE` level (CSP violations; T-1101c).
    pub async fn console_errors(&self) -> Result<Vec<String>, E2eError> {
        let base = webdriver_url()?;
        let session = self
            .client
            .session_id()
            .await
            .map_err(wd)?
            .ok_or_else(|| E2eError::State("no webdriver session id".to_owned()))?;
        let url = format!("{}/session/{}/log", base.trim_end_matches('/'), session);
        let entries: Vec<LogEntry> = reqwest::Client::new()
            .post(&url)
            .json(&json!({ "type": "browser" }))
            .send()
            .await
            .map_err(http)?
            .json()
            .await
            .map_err(http)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.level == "SEVERE")
            .map(|e| e.message)
            .collect())
    }

    /// End the browser session.
    pub async fn close(self) -> Result<(), E2eError> {
        self.client.close().await.map_err(wd)
    }
}

/// One WebDriver browser log entry.
#[derive(Deserialize)]
struct LogEntry {
    level: String,
    message: String,
}

/// The fake Google control client (`/__fake`).
pub struct FakeGoogle {
    base: Url,
    client: reqwest::Client,
    /// `sub` -> (email, email_verified), remembered so a later selection can
    /// fill in the address the scripted login needs.
    accounts: Mutex<HashMap<String, (String, bool)>>,
}

impl FakeGoogle {
    /// Bind the control client to the stack's fake Google.
    pub fn connect(stack: &Stack) -> Result<Self, E2eError> {
        Ok(Self {
            base: stack.fake_google.clone(),
            client: reqwest::Client::new(),
            accounts: Mutex::new(HashMap::new()),
        })
    }

    /// Clear all fake state.
    pub async fn reset(&self) -> Result<(), E2eError> {
        self.post("/__fake/reset", json!({})).await.map(|_| ())
    }

    /// Register a fake Google account and its mailbox.
    pub async fn seed_account(
        &self,
        sub: &str,
        email: &str,
        verified: bool,
    ) -> Result<(), E2eError> {
        self.post("/__fake/gmail/mailboxes", json!({ "email": email }))
            .await?;
        self.accounts
            .lock()
            .map_err(|_| E2eError::State("accounts poisoned".to_owned()))?
            .insert(sub.to_owned(), (email.to_owned(), verified));
        Ok(())
    }

    /// Script the next authorisation to approve `sub`.
    pub async fn select_account_for_next_authorize(&self, sub: &str) -> Result<(), E2eError> {
        let (email, verified) = self
            .accounts
            .lock()
            .map_err(|_| E2eError::State("accounts poisoned".to_owned()))?
            .get(sub)
            .cloned()
            .ok_or_else(|| E2eError::State(format!("no seeded account for {sub}")))?;
        self.post(
            "/__fake/identity/next-login",
            json!({
                "sub": sub,
                "email": email,
                "email_verified": verified,
                "outcome": "approve",
            }),
        )
        .await
        .map(|_| ())
    }

    /// Seed the named corpus cases into `sub`'s mailbox, in order; the last is
    /// the newest.
    pub async fn seed_messages(&self, sub: &str, fixture_ids: &[&str]) -> Result<(), E2eError> {
        for (index, id) in fixture_ids.iter().enumerate() {
            let minutes = i64::try_from(index).unwrap_or(i64::MAX) + 1;
            let received = seeded_received(minutes)?;
            self.seed_message(sub, id, &received, None).await?;
        }
        Ok(())
    }

    /// Seed one corpus case into `sub`'s mailbox at `received` (RFC 3339),
    /// optionally replacing its `List-Unsubscribe` header with `list_unsubscribe`
    /// (the testbed's one-click route), and return the seeded message's ID.
    pub async fn seed_message(
        &self,
        sub: &str,
        fixture_id: &str,
        received: &str,
        list_unsubscribe: Option<&str>,
    ) -> Result<String, E2eError> {
        let email = self.email_of(sub)?;
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        let case = corpus
            .cases
            .iter()
            .find(|c| c.spec.id == fixture_id)
            .ok_or_else(|| E2eError::State(format!("no corpus case {fixture_id}")))?;
        let eml = match list_unsubscribe {
            Some(value) => rewrite_list_unsubscribe(&case.eml, value),
            None => case.eml.clone(),
        };
        let value = self
            .post(
                "/__fake/gmail/messages",
                json!({
                    "email": email,
                    "eml_base64": base64::engine::general_purpose::STANDARD.encode(&eml),
                    "labels": ["INBOX"],
                    "internal_date": received,
                }),
            )
            .await?;
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| E2eError::State("seed response had no message id".to_owned()))
    }

    /// The label IDs on one seeded message (FD-02 AC2, SW-04 AC2, SW-05 AC2).
    pub async fn message_labels(&self, email: &str, id: &str) -> Result<Vec<String>, E2eError> {
        let response = self
            .client
            .get(endpoint(
                &self.base,
                &format!("/__fake/gmail/messages/{email}/{id}"),
            ))
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(http)?;
        if !status.is_success() {
            return Err(E2eError::Http(format!(
                "GET message {id}: {status} {value}"
            )));
        }
        value
            .get("labelIds")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .ok_or_else(|| E2eError::State("labels response had no labelIds".to_owned()))
    }

    /// The mailbox's label names keyed by the label ID a message carries, so a
    /// journey can name a user label the app applied (SW-04 AC2).
    pub async fn label_names(&self, email: &str) -> Result<HashMap<String, String>, E2eError> {
        let response = self
            .client
            .get(endpoint(
                &self.base,
                &format!("/__fake/gmail/labels/{email}"),
            ))
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(http)?;
        if !status.is_success() {
            return Err(E2eError::Http(format!(
                "GET labels {email}: {status} {value}"
            )));
        }
        let labels = value
            .get("labels")
            .and_then(Value::as_array)
            .ok_or_else(|| E2eError::State("labels response had no labels".to_owned()))?;
        Ok(labels
            .iter()
            .filter_map(|label| {
                let id = label.get("id")?.as_str()?.to_owned();
                let name = label.get("name")?.as_str()?.to_owned();
                Some((id, name))
            })
            .collect())
    }

    /// The mailbox address seeded for `sub`.
    fn email_of(&self, sub: &str) -> Result<String, E2eError> {
        self.accounts
            .lock()
            .map_err(|_| E2eError::State("accounts poisoned".to_owned()))?
            .get(sub)
            .map(|(email, _)| email.clone())
            .ok_or_else(|| E2eError::State(format!("no seeded account for {sub}")))
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, E2eError> {
        let response = self
            .client
            .post(endpoint(&self.base, path))
            .json(&body)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(http)?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(E2eError::Http(format!("POST {path}: {status} {value}")))
        }
    }
}

/// The api's test-only control client (`/internal/test`, `testkit` feature).
pub struct TestControl {
    base: Url,
    client: reqwest::Client,
}

impl TestControl {
    /// Bind the control client to the stack's api service.
    pub fn connect(stack: &Stack) -> Result<Self, E2eError> {
        Ok(Self {
            base: stack.api_internal.clone(),
            client: reqwest::Client::new(),
        })
    }

    /// Create an invite for `email` and return the raw token.
    pub async fn create_invite(&self, email: &str) -> Result<String, E2eError> {
        let value = self
            .post("/internal/test/invites", json!({ "email": email }))
            .await?;
        value
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| E2eError::State("invite response had no token".to_owned()))
    }

    /// Advance the fake scheduler's clock by `by`.
    pub async fn advance_clock(&self, by: Duration) -> Result<(), E2eError> {
        let seconds = i64::try_from(by.as_secs()).unwrap_or(i64::MAX);
        self.post(
            "/internal/test/advance-clock",
            json!({ "seconds": seconds }),
        )
        .await
        .map(|_| ())
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, E2eError> {
        let response = self
            .client
            .post(endpoint(&self.base, path))
            .json(&body)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(http)?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(E2eError::Http(format!("POST {path}: {status} {value}")))
        }
    }
}

/// One HTTP request recorded by the `unsub-testbed` (S10 6.2): method, path,
/// headers and raw body. The testbed lower-cases header names.
#[derive(Clone, Debug, Deserialize)]
pub struct RecordedRequest {
    /// The HTTP method, upper case.
    pub method: String,
    /// Lower-cased header names and their values.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
    /// The request path, without the query; used to select a route.
    path: String,
}

impl RecordedRequest {
    /// True when a header named `name` (lower-cased) is present.
    #[must_use]
    pub fn has_header(&self, name: &str) -> bool {
        self.headers.iter().any(|(key, _)| key == name)
    }

    /// The body as text (lossy UTF-8), for exact-body assertions (UN-02 AC1).
    #[must_use]
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The `unsub-testbed` control client (`/__testbed`; S10 6.2).
pub struct Testbed {
    base: Url,
    client: reqwest::Client,
}

impl Testbed {
    /// Bind the control client to the stack's testbed.
    pub fn connect(stack: &Stack) -> Result<Self, E2eError> {
        Ok(Self {
            base: stack.testbed.clone(),
            client: reqwest::Client::new(),
        })
    }

    /// Every request the testbed recorded for `route`, in order.
    pub async fn received(&self, route: &str) -> Result<Vec<RecordedRequest>, E2eError> {
        let response = self
            .client
            .get(endpoint(&self.base, "/__testbed/requests"))
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let all: Vec<RecordedRequest> = response.json().await.map_err(http)?;
        if !status.is_success() {
            return Err(E2eError::Http(format!("GET /__testbed/requests: {status}")));
        }
        Ok(all.into_iter().filter(|r| r.path == route).collect())
    }
}

/// A byte offset per log file, so a journey reads only the lines written after
/// its own start ([`EventLog::since_mark`]).
#[derive(Clone, Debug, Default)]
pub struct LogMark {
    offsets: Vec<(PathBuf, u64)>,
}

/// One metric log line (S10 8): the event type and its outcome code, with the
/// field names T-307 writes (`action` carries the event type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricEvent {
    /// The event type, e.g. `swipe`, `undo`, `unsub_outcome`.
    pub event_type: String,
    /// The outcome code, when the line carries one.
    pub outcome: Option<String>,
}

/// Reads the JSONL logs `scripts/e2e.sh` writes to `target/e2e-logs/*.jsonl`.
pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    /// Bind to the log directory: `MT_E2E_LOG_DIR`, else the repo's
    /// `target/e2e-logs`.
    pub fn open() -> Result<Self, E2eError> {
        let dir = std::env::var_os("MT_E2E_LOG_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/e2e-logs"),
            PathBuf::from,
        );
        Ok(Self { dir })
    }

    /// The byte offset of every log file now; [`Self::since_mark`] then reads
    /// only what is appended after this point.
    pub fn mark(&self) -> LogMark {
        let offsets = self
            .files()
            .into_iter()
            .map(|path| {
                let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
                (path, size)
            })
            .collect();
        LogMark { offsets }
    }

    /// The metric events written since `mark`.
    pub fn since_mark(&self, mark: LogMark) -> Result<Vec<MetricEvent>, E2eError> {
        let mut events = Vec::new();
        for path in self.files() {
            let from = mark
                .offsets
                .iter()
                .find(|(marked, _)| marked == &path)
                .map_or(0, |(_, offset)| *offset);
            let bytes = std::fs::read(&path)
                .map_err(|e| E2eError::State(format!("read {}: {e}", path.display())))?;
            let start = usize::try_from(from).unwrap_or(0).min(bytes.len());
            let text = String::from_utf8_lossy(&bytes[start..]);
            events.extend(text.lines().filter_map(parse_metric));
        }
        Ok(events)
    }

    /// The `.jsonl` files in the log directory, sorted for determinism.
    fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }
}

/// Parse one JSONL line into a metric event, when it is one.
fn parse_metric(line: &str) -> Option<MetricEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("event").and_then(Value::as_str) != Some("metric") {
        return None;
    }
    let event_type = value.get("action").or_else(|| value.get("event_type"))?;
    let event_type = event_type.as_str()?.to_owned();
    let outcome = value
        .get("outcome")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(MetricEvent {
        event_type,
        outcome,
    })
}

/// The RFC 3339 received time `minutes` after the harness's fixed seeded base,
/// so a journey can interleave two mailboxes by time (FD-02 AC1).
pub fn seeded_received(minutes: i64) -> Result<String, E2eError> {
    let base = time::OffsetDateTime::from_unix_timestamp(BASE_UNIX_SECONDS)
        .map_err(|e| E2eError::State(format!("base timestamp: {e}")))?;
    (base + time::Duration::minutes(minutes))
        .format(&Rfc3339)
        .map_err(|e| E2eError::State(format!("rfc3339: {e}")))
}

/// Rewrite the first `List-Unsubscribe:` header line in `eml` to point at
/// `value` (the testbed's one-click route), leaving everything else intact.
fn rewrite_list_unsubscribe(eml: &[u8], value: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(eml);
    let mut out = String::with_capacity(text.len() + value.len());
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        let is_header = !replaced
            && line
                .trim_end_matches(['\r', '\n'])
                .to_ascii_lowercase()
                .starts_with("list-unsubscribe:");
        if is_header {
            let newline = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
            out.push_str(&format!("List-Unsubscribe: <{value}>{newline}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    out.into_bytes()
}

/// The Feed tab's Semantics label (XC-03); its presence means the Feed loaded.
pub const FEED_TAB: &str = "Feed";
/// The Google button's Semantics label (XC-03).
pub const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The invited copy shown on the Sign-in screen (S2 AU-03 AC1).
pub const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";

/// Seed a verified account, create its invite, sign it in through the UI and
/// return a [`Ui`] sitting on the Feed. The named corpus cases are seeded first
/// (the last is the newest).
pub async fn signed_in_user(
    stack: &Stack,
    sub: &str,
    email: &str,
    fixtures: &[&str],
) -> Result<Ui, E2eError> {
    let google = FakeGoogle::connect(stack)?;
    google.seed_account(sub, email, true).await?;
    if !fixtures.is_empty() {
        google.seed_messages(sub, fixtures).await?;
    }
    google.select_account_for_next_authorize(sub).await?;
    let control = TestControl::connect(stack)?;
    let token = control.create_invite(email).await?;
    let ui = Ui::open(stack, &format!("/#/invite?t={token}")).await?;
    ui.wait_for_text(INVITED_COPY, Duration::from_secs(30))
        .await?;
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    ui.wait_for_text(FEED_TAB, Duration::from_secs(60)).await?;
    Ok(ui)
}

/// Click the semantics node whose `aria-label` is `arguments[0]`.
const TAP_JS: &str = r#"
const label = arguments[0];
const node = document.querySelector(`flt-semantics[aria-label="${label}"]`);
if (!node) return false;
node.click();
return true;
"#;

/// Click the last semantics node whose `aria-label` is `arguments[0]`.
const TAP_LAST_JS: &str = r#"
const label = arguments[0];
const nodes = document.querySelectorAll(`flt-semantics[aria-label="${label}"]`);
if (nodes.length === 0) return false;
nodes[nodes.length - 1].click();
return true;
"#;

/// Type `arguments[1]` into the labelled control `arguments[0]`.
const TYPE_JS: &str = r#"
const [label, text] = arguments;
const node = document.querySelector(`flt-semantics[aria-label="${label}"]`);
if (!node) return false;
const input = node.querySelector('input, textarea');
if (!input) {
  node.textContent = text;
  node.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
}
input.focus();
const proto = input.tagName === 'TEXTAREA'
  ? HTMLTextAreaElement.prototype
  : HTMLInputElement.prototype;
const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
setter.call(input, text);
input.dispatchEvent(new Event('input', { bubbles: true }));
return true;
"#;

/// True when the page text or any `aria-label` contains `arguments[0]`.
const TEXT_JS: &str = r#"
const needle = arguments[0];
const body = document.body;
if (!body) return false;
if (body.innerText && body.innerText.includes(needle)) return true;
return Array.from(document.querySelectorAll('[aria-label]'))
  .some(el => (el.getAttribute('aria-label') || '').includes(needle));
"#;

/// Persisted browser state as a JSON string; completes asynchronously.
const STORAGE_JS: &str = r#"
const done = arguments[arguments.length - 1];
(async () => {
  const out = { localStorage: {}, sessionStorage: {}, indexedDB: [], caches: [] };
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      out.localStorage[k] = localStorage.getItem(k);
    }
  } catch (e) {}
  try {
    for (let i = 0; i < sessionStorage.length; i++) {
      const k = sessionStorage.key(i);
      out.sessionStorage[k] = sessionStorage.getItem(k);
    }
  } catch (e) {}
  try {
    if (indexedDB.databases) {
      out.indexedDB = (await indexedDB.databases()).map(d => d.name);
    }
  } catch (e) {}
  try {
    out.caches = await caches.keys();
  } catch (e) {}
  done(JSON.stringify(out));
})();
"#;
