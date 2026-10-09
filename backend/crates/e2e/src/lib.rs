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
    /// The api service, including its `testkit`-only `/internal/test` routes.
    pub api_internal: Url,
}

impl Stack {
    /// Read the stack URLs from the `MT_E2E_*` environment.
    pub fn from_env() -> Result<Self, E2eError> {
        Ok(Self {
            app_url: env_url("MT_E2E_APP_URL")?,
            fake_google: env_url("MT_E2E_FAKE_GOOGLE_URL")?,
            testbed: env_url("MT_E2E_TESTBED_URL")?,
            api_internal: env_url("MT_E2E_API_URL")?,
        })
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
        // reqwest and fantoccini can enable different providers through feature
        // unification; select one explicitly rather than panic at construction.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut capabilities = serde_json::Map::new();
        capabilities.insert(
            "goog:chromeOptions".to_owned(),
            json!({ "args": ["--headless=new", "--disable-dev-shm-usage"] }),
        );
        let client = fantoccini::ClientBuilder::rustls()
            .map_err(|e| E2eError::WebDriver(e.to_string()))?
            .capabilities(capabilities)
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

    /// Register the harness OAuth client with its exact callback URI.
    pub async fn register_client(&self, stack: &Stack) -> Result<(), E2eError> {
        self.post(
            "/__fake/identity/clients",
            json!({
                "client_id": "fake-e2e-client",
                "client_secret": "test-only-not-a-secret",
                "redirect_uris": [endpoint(&stack.app_url, "/api/v1/auth/google/callback")],
            }),
        )
        .await
        .map(|_| ())
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
        let email = self
            .accounts
            .lock()
            .map_err(|_| E2eError::State("accounts poisoned".to_owned()))?
            .get(sub)
            .map(|(email, _)| email.clone())
            .ok_or_else(|| E2eError::State(format!("no seeded account for {sub}")))?;
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        // A fixed base, not wall-clock: the harness must not read the clock (S10
        // 1 rule 2), and deterministic dates keep the seeded order stable.
        let mut base = time::OffsetDateTime::from_unix_timestamp(BASE_UNIX_SECONDS)
            .map_err(|e| E2eError::State(format!("base timestamp: {e}")))?;
        for id in fixture_ids {
            let case = corpus
                .cases
                .iter()
                .find(|c| c.spec.id == *id)
                .ok_or_else(|| E2eError::State(format!("no corpus case {id}")))?;
            base += time::Duration::minutes(1);
            let received = base
                .format(&Rfc3339)
                .map_err(|e| E2eError::State(format!("rfc3339: {e}")))?;
            self.post(
                "/__fake/gmail/messages",
                json!({
                    "email": email,
                    "eml_base64": base64::engine::general_purpose::STANDARD.encode(&case.eml),
                    "labels": ["INBOX"],
                    "internal_date": received,
                }),
            )
            .await?;
        }
        Ok(())
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

/// Click the semantics node whose `aria-label` is `arguments[0]`.
const TAP_JS: &str = r#"
const label = arguments[0];
const node = Array.from(document.querySelectorAll('flt-semantics, [role="button"], button'))
  .find(el => el.getAttribute('aria-label') === label || el.textContent.trim() === label);
if (!node) return false;
node.click();
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
