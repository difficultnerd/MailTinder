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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use fantoccini::Locator;
use serde::{Deserialize, Serialize};
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

/// Cap for each captured text artifact (200 KB), so a runaway DOM never fills
/// the uploaded log folder (S10 3.3).
const TEXT_CAP: usize = 200 * 1024;

/// How long `tap` and `type_into` keep looking for a control before giving up. Flutter web builds its labelled semantics nodes
/// shortly AFTER the page text is visible (slow on a cold CI runner), so a single lookup right after the text appears can miss
/// a control that is a few hundred milliseconds from existing.
const FIND_TIMEOUT: Duration = Duration::from_secs(20);

/// Directory the harness writes failure artifacts to. `scripts/e2e.sh` points
/// `MT_E2E_LOG_DIR` at `target/e2e-logs`, the folder the `e2e` CI job uploads.
fn log_dir() -> PathBuf {
    std::env::var_os("MT_E2E_LOG_DIR")
        .map_or_else(|| PathBuf::from("target/e2e-logs"), PathBuf::from)
}

/// The ChromeDriver log the harness tails on failure
/// (`MT_E2E_CHROMEDRIVER_LOG`, default `<log dir>/chromedriver.log`).
fn chromedriver_log_path() -> PathBuf {
    std::env::var_os("MT_E2E_CHROMEDRIVER_LOG")
        .map_or_else(|| log_dir().join("chromedriver.log"), PathBuf::from)
}

/// A filesystem-safe artifact-name stem for `label`.
fn slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "step".to_owned()
    } else {
        trimmed.chars().take(60).collect()
    }
}

/// `body`, truncated to at most `cap` bytes on a char boundary.
fn cap_str(body: &str, cap: usize) -> String {
    if body.len() <= cap {
        return body.to_owned();
    }
    let mut end = cap;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n...[truncated, {} of {} bytes shown]",
        body.get(..end).unwrap_or_default(),
        end,
        body.len()
    )
}

/// Write one text artifact, ignoring failure: a diagnostic must never mask the
/// real error.
fn write_artifact(dir: &Path, name: &str, body: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), body.as_bytes());
}

/// Write one binary artifact (a screenshot) to `dir`, ignoring failure.
fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), bytes);
}

/// Append a line to `<dir>/diagnostics.log` for the parts that have no file of
/// their own (for example a screenshot the dead session could not take).
fn note(dir: &Path, message: &str) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("diagnostics.log"))
    {
        let _ = writeln!(file, "{message}");
    }
}

/// The last `lines` lines of `path`, or `None` when it cannot be read.
fn tail_lines(path: &Path, lines: usize) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    Some(all.get(start..)?.join("\n"))
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
        // `--no-sandbox` and `--disable-dev-shm-usage` are what headless Chrome
        // needs on a containerised/CI Linux runner and are harmless elsewhere.
        // The window size guarantees a non-zero viewport so Flutter paints a
        // real layout; `--enable-unsafe-swiftshader` keeps CanvasKit's WebGL
        // working when the GPU is unavailable (as headless CI has no GPU).
        capabilities.insert(
            "goog:chromeOptions".to_owned(),
            json!({ "args": [
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--enable-unsafe-swiftshader",
                "--window-size=1280,1024",
                "--hide-scrollbars",
                "--mute-audio",
            ] }),
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
    ///
    /// The labelled node is first tagged in the DOM, then clicked through
    /// WebDriver's element click, which dispatches *trusted* pointer events -
    /// the same input a real user produces and what Flutter web's gesture
    /// recognizers and semantics click-debouncer both expect. A scripted DOM
    /// `click()` remains the fallback for a node WebDriver cannot interact
    /// with (for example an element its hit test finds covered).
    pub async fn tap(&self, label: &str) -> Result<(), E2eError> {
        // Flutter web rebuilds its semantics tree while the page settles (slower on a cold CI runner): a node can be found and tagged,
        // then replaced before the click lands, and a miss at any step used to be fatal. Retry the whole find -> tag -> click sequence
        // until it succeeds or FIND_TIMEOUT passes; every step is a no-op on a miss, so repeating it is safe.
        let deadline = Instant::now() + FIND_TIMEOUT;
        loop {
            let marked = self
                .client
                .execute(MARK_TAPPABLE_JS, vec![json!(label)])
                .await
                .map_err(wd)?;
            if marked.as_bool() == Some(true) {
                let clicked_natively = match self.client.find(Locator::Css(MARKED_SELECTOR)).await {
                    Ok(element) => element.click().await.is_ok(),
                    Err(_) => false,
                };
                if clicked_natively {
                    return Ok(());
                }
                let clicked = self
                    .client
                    .execute(CLICK_MARKED_JS, vec![])
                    .await
                    .map_err(wd)?;
                if clicked.as_bool() == Some(true) {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                self.capture_failure(label).await;
                return Err(E2eError::NotFound(label.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Type `text` into the labelled control.
    ///
    /// Flutter web's text fields ignore synthetic DOM `input` events, so the
    /// field is focused with a real click and the text is sent as WebDriver key
    /// events — the trusted input a user produces. Flutter exposes the field's
    /// `aria-label` on its semantics `input` (or the editing element).
    pub async fn type_into(&self, label: &str, text: &str) -> Result<(), E2eError> {
        let selector = format!(
            "input[aria-label=\"{label}\"], textarea[aria-label=\"{label}\"], \
             [data-semantics-role=\"text-field\"][aria-label=\"{label}\"]"
        );
        let deadline = Instant::now() + FIND_TIMEOUT;
        loop {
            if let Ok(field) = self.client.find(Locator::Css(&selector)).await {
                // Focus the field; Flutter then routes typed keys to it.
                let _ = field.click().await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let target = match self.client.active_element().await {
                    Ok(active) => active,
                    Err(_) => field,
                };
                if target.send_keys(text).await.is_ok() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                self.capture_failure(label).await;
                return Err(E2eError::NotFound(label.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// The current page URL, when WebDriver can report it.
    pub async fn current_url(&self) -> Result<Url, E2eError> {
        self.client.current_url().await.map_err(wd)
    }

    /// Poll until the URL starts with `prefix` and no longer contains
    /// `not_contains`, or the deadline passes. Watches the OAuth redirect leave
    /// the invite screen and come back to the app.
    pub async fn wait_for_url(
        &self,
        prefix: &str,
        not_contains: &str,
        timeout: Duration,
    ) -> Result<(), E2eError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(url) = self.client.current_url().await {
                let now = url.as_str();
                if now.starts_with(prefix) && !now.contains(not_contains) {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                self.capture_failure("redirect").await;
                return Err(E2eError::Timeout(format!(
                    "url to start with {prefix} and leave {not_contains}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Poll until `text` is no longer on the page or in the semantics tree.
    /// Confirms a tap registered: the tapped control's label disappears as soon
    /// as its handler runs (the button swaps to a spinner).
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
                self.capture_failure(text).await;
                return Err(E2eError::Timeout(text.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Poll until `text` appears in the *semantics tree* (not merely the page
    /// text). The Feed shows the next card behind the focused one, excluded
    /// from semantics (FD-01 AC1), so this distinguishes the card in focus from
    /// the one behind it when a journey reads a card's sender in turn.
    pub async fn wait_for_semantic_text(
        &self,
        text: &str,
        timeout: Duration,
    ) -> Result<(), E2eError> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self
                .client
                .execute(SEMANTIC_TEXT_JS, vec![json!(text)])
                .await
                .map_err(wd)?;
            if found.as_bool() == Some(true) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.capture_failure(text).await;
                return Err(E2eError::Timeout(format!("semantics to show {text}")));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Best-effort capture of everything a failing step needs: a screenshot, the
    /// visible text and page source (each truncated to 200 KB), the semantics
    /// tree, the browser console log, the current URL and the tail of the
    /// ChromeDriver log, all written into `MT_E2E_LOG_DIR` (S10 3.3). Every
    /// artifact is written independently, so one failure never hides the others.
    pub async fn capture_failure(&self, label: &str) {
        let dir = log_dir();
        let stem = slug(label);
        // The screenshot is the single most useful artifact: what the page
        // actually showed. PNG bytes are already base64-decoded by WebDriver.
        match self.client.screenshot().await {
            Ok(png) => write_bytes(&dir, &format!("{stem}-screenshot.png"), &png),
            Err(e) => note(&dir, &format!("screenshot for {label}: {e}")),
        }
        // Current URL, document state and user agent.
        let meta = "return location.href + '\\nreadyState=' + document.readyState \
                    + '\\nuserAgent=' + navigator.userAgent;";
        if let Ok(value) = self.js(meta).await {
            write_artifact(
                &dir,
                &format!("{stem}-url.txt"),
                value.as_str().unwrap_or_default(),
            );
        }
        // Visible text (200 KB cap).
        if let Ok(value) = self
            .js("return document.body ? document.body.innerText : '(no document.body)';")
            .await
        {
            write_artifact(
                &dir,
                &format!("{stem}-page.txt"),
                &cap_str(value.as_str().unwrap_or_default(), TEXT_CAP),
            );
        }
        // Full page source (200 KB cap).
        if let Ok(source) = self.client.source().await {
            write_artifact(
                &dir,
                &format!("{stem}-source.html"),
                &cap_str(&source, TEXT_CAP),
            );
        }
        // Semantics tree: proves whether the labelled control existed at all.
        if let Ok(value) = self.js(SEMANTICS_JS).await {
            let nodes = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            write_artifact(
                &dir,
                &format!("{stem}-semantics.json"),
                &cap_str(&nodes, TEXT_CAP),
            );
        }
        // Browser console log, every level.
        match self.console_log().await {
            Ok(entries) => write_artifact(&dir, &format!("{stem}-console.json"), &entries),
            Err(e) => note(&dir, &format!("console log for {label}: {e}")),
        }
        // ChromeDriver's tail: the last commands it saw and any crash.
        match tail_lines(&chromedriver_log_path(), 100) {
            Some(tail) => write_artifact(&dir, &format!("{stem}-chromedriver.log"), &tail),
            None => note(&dir, &format!("chromedriver log for {label}: not readable")),
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(
            out,
            "[e2e] failure diagnostics for \"{label}\" written to {}",
            dir.display()
        );
        let _ = out.flush();
    }

    /// Run a JS expression body and return its value.
    async fn js(&self, script: &str) -> Result<Value, E2eError> {
        self.client.execute(script, vec![]).await.map_err(wd)
    }

    /// The whole browser log (all levels) as pretty JSON.
    async fn console_log(&self) -> Result<String, E2eError> {
        let entries = self.browser_log().await?;
        serde_json::to_string_pretty(&entries)
            .map_err(|e| E2eError::State(format!("console log json: {e}")))
    }

    /// Fetch the WebDriver browser log for this session.
    async fn browser_log(&self) -> Result<Vec<LogEntry>, E2eError> {
        let base = webdriver_url()?;
        let session = self
            .client
            .session_id()
            .await
            .map_err(wd)?
            .ok_or_else(|| E2eError::State("no webdriver session id".to_owned()))?;
        let url = format!("{}/session/{}/log", base.trim_end_matches('/'), session);
        reqwest::Client::new()
            .post(&url)
            .json(&json!({ "type": "browser" }))
            .send()
            .await
            .map_err(http)?
            .json()
            .await
            .map_err(http)
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

    /// Switch back to the first (main) window after a popup.
    pub async fn switch_to_main(&self) -> Result<(), E2eError> {
        let handles = self.client.windows().await.map_err(wd)?;
        let first = handles
            .into_iter()
            .next()
            .ok_or_else(|| E2eError::State("no browser window".to_owned()))?;
        self.client.switch_to_window(first).await.map_err(wd)
    }

    /// Reload the app: a fresh Feed load, which is how the api collects stored
    /// unsubscribe outcomes into History (UN-01 AC3).
    pub async fn reload(&self) -> Result<(), E2eError> {
        self.client
            .execute("location.reload(); return true;", vec![])
            .await
            .map(|_| ())
            .map_err(wd)
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
        Ok(self
            .browser_log()
            .await?
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
#[derive(Deserialize, Serialize)]
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

    /// The email address recorded for a seeded `sub`, or an error.
    fn account_email(&self, sub: &str) -> Result<String, E2eError> {
        self.accounts
            .lock()
            .map_err(|_| E2eError::State("accounts poisoned".to_owned()))?
            .get(sub)
            .map(|(email, _)| email.clone())
            .ok_or_else(|| E2eError::State(format!("no seeded account for {sub}")))
    }

    /// Seed the named corpus cases into `sub`'s mailbox at explicit offsets, in
    /// minutes after the harness base time. Lets a journey interleave two
    /// mailboxes by received time (FD-02 AC1) without reading the wall clock.
    pub async fn seed_messages_at(
        &self,
        sub: &str,
        fixtures: &[(&str, i64)],
    ) -> Result<(), E2eError> {
        let email = self.account_email(sub)?;
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        for (id, minutes) in fixtures {
            let case = corpus
                .cases
                .iter()
                .find(|c| c.spec.id == *id)
                .ok_or_else(|| E2eError::State(format!("no corpus case {id}")))?;
            self.seed_case(&email, &case.eml, *minutes, &["INBOX"])
                .await?;
        }
        Ok(())
    }

    /// Seed one one-click corpus message whose `List-Unsubscribe` URL is
    /// rewritten to `one_click`, and return the fake message id. The fake does
    /// not verify the DKIM signature body, so only the URL changes.
    pub async fn seed_one_click_message(
        &self,
        sub: &str,
        fixture_id: &str,
        one_click: &Url,
        labels: &[&str],
    ) -> Result<String, E2eError> {
        let email = self.account_email(sub)?;
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        let case = corpus
            .cases
            .iter()
            .find(|c| c.spec.id == fixture_id)
            .ok_or_else(|| E2eError::State(format!("no corpus case {fixture_id}")))?;
        let eml = rewrite_list_unsubscribe(&case.eml, one_click)?;
        self.seed_case(&email, &eml, 1, labels).await
    }

    /// Seed one corpus case into `sub`'s mailbox with its `INBOX` label and
    /// return the fake message id.
    pub async fn seed_message(&self, sub: &str, fixture_id: &str) -> Result<String, E2eError> {
        let email = self.account_email(sub)?;
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        let case = corpus
            .cases
            .iter()
            .find(|c| c.spec.id == fixture_id)
            .ok_or_else(|| E2eError::State(format!("no corpus case {fixture_id}")))?;
        self.seed_case(&email, &case.eml, 1, &["INBOX"]).await
    }

    /// POST one message to the fake Gmail mailbox and return its id.
    async fn seed_case(
        &self,
        email: &str,
        eml: &[u8],
        minutes: i64,
        labels: &[&str],
    ) -> Result<String, E2eError> {
        let received = time::OffsetDateTime::from_unix_timestamp(BASE_UNIX_SECONDS)
            .map_err(|e| E2eError::State(format!("base timestamp: {e}")))?
            + time::Duration::minutes(minutes);
        let received = received
            .format(&Rfc3339)
            .map_err(|e| E2eError::State(format!("rfc3339: {e}")))?;
        let value = self
            .post(
                "/__fake/gmail/messages",
                json!({
                    "email": email,
                    "eml_base64": base64::engine::general_purpose::STANDARD.encode(eml),
                    "labels": labels,
                    "internal_date": received,
                }),
            )
            .await?;
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| E2eError::State("seed message returned no id".to_owned()))
    }

    /// The label set currently on `sub`'s message `id`, from the fake Gmail
    /// control route, with each label ID resolved to its display name (exact
    /// set, for undo-restores-labels and filed-label assertions). A user label
    /// is stored as `Label_<n>` on the message, so it must be mapped back to
    /// the name the journey typed (SW-04 AC2).
    pub async fn message_labels(&self, sub: &str, id: &str) -> Result<Vec<String>, E2eError> {
        let email = self.account_email(sub)?;
        let url = endpoint(&self.base, &format!("/__fake/gmail/messages/{email}/{id}"));
        let value: Value = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(http)?
            .json()
            .await
            .map_err(http)?;
        let label_ids = value
            .get("labelIds")
            .and_then(Value::as_array)
            .ok_or_else(|| E2eError::State("label response had no labelIds".to_owned()))?;
        let names = self.label_names(&email).await?;
        Ok(label_ids
            .iter()
            .filter_map(Value::as_str)
            .map(|label_id| {
                names
                    .get(label_id)
                    .cloned()
                    .unwrap_or_else(|| label_id.to_owned())
            })
            .collect())
    }

    /// Every label ID of `email`'s mailbox mapped to its name, read through the
    /// fake Gmail `labels.list` API with a control-issued token (the app's own
    /// label names live there; message label IDs are opaque).
    async fn label_names(&self, email: &str) -> Result<HashMap<String, String>, E2eError> {
        let token = self
            .post(
                "/__fake/tokens",
                json!({
                    "email": email,
                    "scopes": ["https://www.googleapis.com/auth/gmail.modify"],
                    "ttl_s": 3600,
                }),
            )
            .await?
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| E2eError::State("token response had no access_token".to_owned()))?;
        let url = endpoint(&self.base, "/gmail/v1/users/me/labels");
        let value: Value = self
            .client
            .get(&url)
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(http)?
            .json()
            .await
            .map_err(http)?;
        let mut names = HashMap::new();
        for label in value
            .get("labels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(label_id), Some(name)) = (
                label.get("id").and_then(Value::as_str),
                label.get("name").and_then(Value::as_str),
            ) {
                names.insert(label_id.to_owned(), name.to_owned());
            }
        }
        Ok(names)
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

    /// Advance the api's virtual job clock by `by` (testkit only). Journeys use
    /// this instead of sleeping for minutes (S10 6.3).
    pub async fn advance_clock(&self, by: Duration) -> Result<(), E2eError> {
        self.post(
            "/internal/test/advance-clock",
            json!({ "seconds": by.as_secs() }),
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

/// DOM attribute [`MARK_TAPPABLE_JS`] sets on the node and
/// [`CLICK_MARKED_JS`] clicks; the CSS selector WebDriver finds it by.
const MARKED_SELECTOR: &str = "[data-e2e-tap]";

/// Tag the tappable semantics node whose `aria-label` (or text) is
/// `arguments[0]`, preferring a node Flutter marked `flt-tappable`. Returns
/// false when nothing matches.
const MARK_TAPPABLE_JS: &str = r#"
const label = arguments[0];
const matches = Array.from(document.querySelectorAll('flt-semantics, [role="button"], button'))
  .filter(el => {
    const aria = el.getAttribute('aria-label') || '';
    // Flutter repeats a merged control's label ("Settings\nSettings") in its
    // aria-label, so a whole-line match counts as well as an exact one.
    return aria === label
      || (el.textContent || '').trim() === label
      || aria.split('\n').some(line => line === label);
  });
if (matches.length === 0) return false;
document.querySelectorAll('[data-e2e-tap]').forEach(el => el.removeAttribute('data-e2e-tap'));
matches.sort((a, b) => (b.hasAttribute('flt-tappable') ? 1 : 0) - (a.hasAttribute('flt-tappable') ? 1 : 0));
matches[0].setAttribute('data-e2e-tap', '');
return true;
"#;

/// Scripted fallback: dispatch a DOM `click` on the tagged node. Flutter's
/// semantics click handler (Tappable) turns it into a tap.
const CLICK_MARKED_JS: &str = r#"
const node = document.querySelector('[data-e2e-tap]');
if (!node) return false;
node.click();
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

/// True when any Flutter semantics node carries `arguments[0]` in its text or
/// its `aria-label`. Flutter web surfaces a merged card's content through the
/// node's `aria-label` (its `textContent` stays empty), so both are checked.
/// The card behind the focused one is wrapped in `ExcludeSemantics`, so it has
/// no `flt-semantics` node and this sees only the card in focus.
const SEMANTIC_TEXT_JS: &str = r#"
const needle = arguments[0];
return Array.from(document.querySelectorAll('flt-semantics'))
  .some(el => (el.textContent || '').includes(needle)
           || (el.getAttribute('aria-label') || '').includes(needle));
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

/// Every semantics/actionable node's role, label, text and box, as JSON. Proves
/// whether the control a wait looked for existed, and where it sat on the page.
const SEMANTICS_JS: &str = r#"
const nodes = Array.from(document.querySelectorAll('flt-semantics, [role], [aria-label]'));
const out = nodes.slice(0, 400).map(el => {
  const r = el.getBoundingClientRect();
  return {
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute('role'),
    label: el.getAttribute('aria-label'),
    text: (el.textContent || '').trim().slice(0, 120),
    box: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)],
  };
});
return JSON.stringify({ count: nodes.length, nodes: out });
"#;

// ---------------------------------------------------------------------------
// T-1101b helpers: the unsubscribe testbed, the metric-event log, and the
// sign-in preamble shared by the triage and unsubscribe journeys.
// ---------------------------------------------------------------------------

/// Copy shown on the Sign-in screen while an invite is being redeemed
/// (S2 AU-03 AC1; `Copy.invited`).
pub const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";
/// The Google button's Semantics label (XC-03).
pub const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The Feed action buttons' Semantics labels (S9 section 3, XC-03).
pub const KEEP_BUTTON: &str = "Keep";
pub const REJECT_BUTTON: &str = "Reject";
pub const FILE_BUTTON: &str = "File";
pub const UNDO_BUTTON: &str = "Undo";
pub const NEW_CATEGORY: &str = "New category";
/// Settings entry titles and the History filter (S9 7, 7.2).
pub const SETTINGS_TAB: &str = "Settings";
pub const CONNECTED_ACCOUNTS: &str = "Connected accounts";
pub const HISTORY: &str = "History";
pub const UNSUBSCRIBES_FILTER: &str = "Unsubscribes";

/// How long a wait may take. A cold runner paints slowly, so the first wait is
/// generous; after a tap a shorter ack timeout lets a lost click be retried.
pub const APP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
pub const FEED_TIMEOUT: Duration = Duration::from_secs(120);
pub const REDIRECT_TIMEOUT: Duration = Duration::from_secs(60);
pub const TAP_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// The app origin (`http://localhost:<port>`), a prefix for URL waits.
#[must_use]
pub fn app_origin(stack: &Stack) -> String {
    stack.app_url.as_str().trim_end_matches('/').to_owned()
}

/// Signs a seeded account in through the UI and returns a ready [`Ui`] on the
/// Feed (S10 3.3).
///
/// The caller resets fake-google and registers the harness OAuth client first;
/// this seeds the account (and its corpus fixtures, when any), scripts the next
/// authorisation to approve it, mints an invite through the api's test route,
/// redeems it, and waits for the Feed's action buttons.
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
    ui.wait_for_text(INVITED_COPY, APP_LOAD_TIMEOUT).await?;
    tap_continue_with_google(&ui).await?;
    ui.wait_for_url(&app_origin(stack), "/invite", REDIRECT_TIMEOUT)
        .await?;
    ui.wait_for_text(KEEP_BUTTON, FEED_TIMEOUT).await?;
    Ok(ui)
}

/// Tap "Continue with Google" and confirm the tap registered: the button swaps
/// to a spinner, so its label disappears once the handler runs. A lost click on
/// a cold runner is retried once.
pub async fn tap_continue_with_google(ui: &Ui) -> Result<(), E2eError> {
    ui.tap(CONTINUE_WITH_GOOGLE).await?;
    if ui
        .wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
        .await
        .is_err()
    {
        ui.tap(CONTINUE_WITH_GOOGLE).await?;
        ui.wait_for_text_absent(CONTINUE_WITH_GOOGLE, TAP_ACK_TIMEOUT)
            .await?;
    }
    Ok(())
}

/// The `unsub-testbed` control client (`/__testbed/...`, S10 6.2).
pub struct Testbed {
    base: Url,
    client: reqwest::Client,
}

/// Exactly what the testbed recorded for one request.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    /// The HTTP method, upper case.
    pub method: String,
    /// Lower-cased header names with their values.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
}

#[derive(Deserialize)]
struct TestbedRecord {
    #[serde(default)]
    method: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    body: Vec<u8>,
}

impl Testbed {
    /// Bind the control client to the stack's unsubscribe testbed.
    pub fn connect(stack: &Stack) -> Result<Self, E2eError> {
        Ok(Self {
            base: stack.testbed.clone(),
            client: reqwest::Client::new(),
        })
    }

    /// Everything the testbed recorded for `route`, in order.
    pub async fn received(&self, route: &str) -> Result<Vec<RecordedRequest>, E2eError> {
        let url = endpoint(&self.base, "/__testbed/requests");
        let records: Vec<TestbedRecord> = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(http)?
            .json()
            .await
            .map_err(http)?;
        Ok(records
            .into_iter()
            .filter(|r| r.path == route)
            .map(|r| RecordedRequest {
                method: r.method,
                headers: r.headers,
                body: r.body,
            })
            .collect())
    }

    /// Clear everything the testbed has recorded.
    pub async fn reset(&self) -> Result<(), E2eError> {
        let url = endpoint(&self.base, "/__testbed/reset");
        let response = self.client.post(&url).send().await.map_err(http)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(E2eError::Http(format!(
                "POST /__testbed/reset: {}",
                response.status()
            )))
        }
    }

    /// The https one-click URL on the testbed for `path`; a one-click POST must
    /// be https (UN-02 AC1). The https listener's port is printed by the testbed
    /// binary into its log, which `scripts/e2e.sh` writes under the harness log
    /// directory.
    pub fn one_click_url(&self, path: &str) -> Result<Url, E2eError> {
        testbed_https_base()?
            .join(path.trim_start_matches('/'))
            .map_err(|_| E2eError::InvalidUrl("MT_E2E_TESTBED_HTTPS"))
    }
}

/// The testbed's https base, parsed from the `TESTBED_HTTPS_ADDR=` line its
/// binary prints into `<log dir>/unsub-testbed.jsonl` at start-up.
fn testbed_https_base() -> Result<Url, E2eError> {
    let path = log_dir().join("unsub-testbed.jsonl");
    let text =
        std::fs::read_to_string(&path).map_err(|_| E2eError::MissingEnv("MT_E2E_TESTBED_HTTPS"))?;
    let marker = "TESTBED_HTTPS_ADDR=";
    let rest = text
        .find(marker)
        .map(|at| &text[at + marker.len()..])
        .ok_or(E2eError::MissingEnv("MT_E2E_TESTBED_HTTPS"))?;
    let addr: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    Url::parse(&format!("https://{addr}")).map_err(|_| E2eError::InvalidUrl("MT_E2E_TESTBED_HTTPS"))
}

/// A metric event (S10 8). Field names follow T-307's schema: the JSON line's
/// `action` key carries the event type, and `outcome` the outcome code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricEvent {
    /// The metric event type (`swipe`, `undo`, `unsub_outcome`, ...).
    pub event_type: String,
    /// The outcome code, when the event has one.
    pub outcome: Option<String>,
}

/// A byte offset per log file, so a journey reads only the lines written after
/// its mark (S10 8).
pub struct LogMark {
    files: Vec<(PathBuf, u64)>,
}

/// Reads the metric events a journey's services emitted, from the per-service
/// JSONL logs under the harness log directory (T-1101a).
pub struct EventLog {
    dir: PathBuf,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLog {
    /// An event log over `MT_E2E_LOG_DIR` (default `target/e2e-logs`).
    #[must_use]
    pub fn new() -> Self {
        Self { dir: log_dir() }
    }

    /// Record the current end of every `.jsonl` file.
    #[must_use]
    pub fn mark(&self) -> LogMark {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(std::ffi::OsStr::to_str) == Some("jsonl") {
                    let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    files.push((path, len));
                }
            }
        }
        LogMark { files }
    }

    /// Every metric event written to the marked files since `mark`, in file
    /// order.
    pub fn since_mark(&self, mark: LogMark) -> Result<Vec<MetricEvent>, E2eError> {
        let mut events = Vec::new();
        for (path, offset) in mark.files {
            let bytes = std::fs::read(&path)
                .map_err(|e| E2eError::State(format!("read {}: {e}", path.display())))?;
            let start = usize::try_from(offset).unwrap_or(0);
            if start >= bytes.len() {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes[start..]);
            for line in text.lines() {
                let Ok(value) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if value.get("event").and_then(Value::as_str) != Some("metric") {
                    continue;
                }
                let Some(event_type) = value.get("action").and_then(Value::as_str) else {
                    continue;
                };
                events.push(MetricEvent {
                    event_type: event_type.to_owned(),
                    outcome: value
                        .get("outcome")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
        Ok(events)
    }
}

/// Replace the first angle-bracketed URL of the `List-Unsubscribe` header in
/// `eml` with `url`, so a seeded one-click message points at the testbed. The
/// fake trusts the synthetic `Authentication-Results`/`DKIM-Signature`, so only
/// the URL value changes.
fn rewrite_list_unsubscribe(eml: &[u8], url: &Url) -> Result<Vec<u8>, E2eError> {
    let text = std::str::from_utf8(eml)
        .map_err(|_| E2eError::State("corpus eml is not utf-8".to_owned()))?;
    let lower = text.to_ascii_lowercase();
    let header_at = lower
        .find("list-unsubscribe:")
        .ok_or_else(|| E2eError::State("fixture has no List-Unsubscribe header".to_owned()))?;
    let after = &text[header_at..];
    let open_rel = after
        .find('<')
        .ok_or_else(|| E2eError::State("List-Unsubscribe has no angle-bracketed URL".to_owned()))?;
    let close_rel = after[open_rel..]
        .find('>')
        .ok_or_else(|| E2eError::State("List-Unsubscribe URL is not closed".to_owned()))?;
    let open = header_at + open_rel;
    let close = header_at + open_rel + close_rel;
    let mut out = String::with_capacity(text.len() + url.as_str().len());
    out.push_str(&text[..=open]);
    out.push_str(url.as_str());
    out.push_str(&text[close..]);
    Ok(out.into_bytes())
}
