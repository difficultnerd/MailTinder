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

/// How long `type_into` lets a just-focused text field settle before sending
/// its keys. Flutter starts its text editing a framework round-trip after
/// focus, so keys sent immediately can reach a field that is not yet listening.
const FOCUS_SETTLE: Duration = Duration::from_millis(300);

/// The input-source id of [`Ui::pull_to_refresh`]'s touch sequence. A separate
/// id keeps it from colliding with a real device's state in the session.
const PULL_POINTER_ID: &str = "e2e-pull";

/// How many pointer moves [`Ui::pull_to_refresh`] sends. Flutter's scroll
/// physics accumulate overscroll per drag update, and a single large jump can
/// be coalesced away, so the pull arrives as several small moves.
const PULL_STEPS: u32 = 8;

/// The duration of each [`PULL_STEPS`] move, in milliseconds.
const PULL_STEP_MS: u64 = 16;

/// How far [`Ui::pull_to_refresh`] travels, in CSS pixels. The
/// `RefreshIndicator` arms once the drag passes about a sixth of the scroll
/// view's height (`_kDragContainerExtentPercentage` times the colour tween's
/// end), so a third of the 1024-pixel e2e window is comfortably past it.
const PULL_DISTANCE: f64 = 320.0;

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

/// Append one diagnostic line to the run's `diagnostics.log` (S10 3.3), so a
/// step that had to take a fallback path is visible in the uploaded log folder.
/// It carries only which path a journey took - never an address, identifier or
/// message content.
pub fn note_step(message: &str) {
    note(&log_dir(), message);
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

    /// Run a lookup script until it reports `true` or `FIND_TIMEOUT` passes. The scripts return `false` without side effects when
    /// the control does not exist yet, so repeating them is safe.
    async fn poll_script(&self, js: &str, args: Vec<Value>) -> Result<bool, E2eError> {
        let deadline = Instant::now() + FIND_TIMEOUT;
        loop {
            let found = self.client.execute(js, args.clone()).await.map_err(wd)?;
            if found.as_bool() == Some(true) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
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
        self.tap_marked(MARK_TAPPABLE_JS, label).await
    }

    /// Click the control whose merged Semantics label carries `label` as one of
    /// its parts (or as its whole text).
    ///
    /// A bottom-navigation destination merges its icon's label and its text
    /// label into one semantics node, and Flutter joins merged labels with a
    /// newline (`_concatAttributedString`), so the Settings tab's `aria-label`
    /// is `"Settings\nSettings"`: [`Self::tap`]'s exact match cannot find it.
    /// Every other control matches exactly, so this is the exception, not the
    /// rule (T-1101e).
    pub async fn tap_merged(&self, label: &str) -> Result<(), E2eError> {
        self.tap_marked(MARK_MERGED_JS, label).await
    }

    /// Tag the labelled control with `mark_js`, then click it (see
    /// [`Self::tap`] for why the click is retried).
    async fn tap_marked(&self, mark_js: &str, label: &str) -> Result<(), E2eError> {
        // Flutter web rebuilds its semantics tree while the page settles (slower on a cold CI runner): a node can be found and tagged,
        // then replaced before the click lands, and a miss at any step used to be fatal. Retry the whole find -> tag -> click sequence
        // until it succeeds or FIND_TIMEOUT passes; every step is a no-op on a miss, so repeating it is safe.
        let deadline = Instant::now() + FIND_TIMEOUT;
        loop {
            let marked = self
                .client
                .execute(mark_js, vec![json!(label)])
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

    /// Type `text` into the labelled text control.
    ///
    /// Flutter web renders a text field as an `<input>`/`<textarea>` inside its
    /// `flt-semantics` host, carrying a field's label (the filing sheet's hint,
    /// SW-04 AC3) as `aria-label` on that editable element. The control is
    /// tagged by [`MARK_TYPE_JS`], then typed with WebDriver's native element
    /// keys: Flutter attaches its text-editing listeners only once the field is
    /// focused, so a scripted DOM value-set is overwritten by the framework's
    /// own state sync, whereas real key events land on the installed listeners.
    pub async fn type_into(&self, label: &str, text: &str) -> Result<(), E2eError> {
        if !self.poll_script(MARK_TYPE_JS, vec![json!(label)]).await? {
            self.capture_failure(label).await;
            return Err(E2eError::NotFound(label.to_owned()));
        }
        let element = self
            .client
            .find(Locator::Css(MARKED_TYPE_SELECTOR))
            .await
            .map_err(wd)?;
        // Focus the field first: the framework round-trips through its own
        // event loop before editing is live, so let the focus settle before the
        // keys arrive, or the first one is lost.
        let _ = element.click().await;
        tokio::time::sleep(FOCUS_SETTLE).await;
        element.send_keys(text).await.map_err(wd)?;
        // Confirm the framework took the text; a field that never received it
        // is a miss, reported like any other control the harness cannot drive.
        if self.poll_script(TYPED_VALUE_JS, vec![json!(text)]).await? {
            Ok(())
        } else {
            self.capture_failure(label).await;
            Err(E2eError::NotFound(label.to_owned()))
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
        let session = self.session_id().await?;
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

    /// Pull the Feed down with a real touch drag to refresh it (S9 section 3,
    /// FD-03 AC5). The Feed load this starts is the one that collects a queued
    /// unsubscribe's outcome into History (S10 6.3, option B; T-1101e).
    ///
    /// Flutter web scrolls a scroll view for touch pointers only - its default
    /// scroll behaviour ignores a mouse drag and a wheel never overscrolls -
    /// and a downward drag *on* the card is the Skip gesture, so the drag must
    /// start outside the card. The Feed's header is outside the scroll view, so
    /// the strip just below it is inside the scroll view and above the card;
    /// the idle Blitz button marks that boundary (it is the header's last row),
    /// and the drag then travels well past the sixth of the viewport the
    /// indicator arms at, with room to spare inside the window.
    pub async fn pull_to_refresh(&self) -> Result<(), E2eError> {
        let start = self.js(PULL_START_JS).await?;
        let (Some(x), Some(y)) = (
            start.get("x").and_then(Value::as_f64),
            start.get("y").and_then(Value::as_f64),
        ) else {
            self.capture_failure("pull to refresh").await;
            return Err(E2eError::NotFound(
                "the top of the Feed's scroll view to pull down from".to_owned(),
            ));
        };
        let mut steps = vec![
            json!({ "type": "pointerMove", "duration": 0, "x": x, "y": y, "origin": "viewport" }),
            json!({ "type": "pointerDown" }),
        ];
        for step in 1..=PULL_STEPS {
            let travelled = PULL_DISTANCE * f64::from(step) / f64::from(PULL_STEPS);
            steps.push(json!({
                "type": "pointerMove",
                "duration": PULL_STEP_MS,
                "x": x,
                "y": y + travelled,
                "origin": "viewport",
            }));
        }
        steps.push(json!({ "type": "pointerUp" }));
        self.webdriver_post(
            "/actions",
            json!({ "actions": [{
                "type": "pointer",
                "id": PULL_POINTER_ID,
                "parameters": { "pointerType": "touch" },
                "actions": steps,
            }] }),
        )
        .await?;
        // Release the input state, so a later journey's gestures start clean;
        // a release that fails is not worth failing the journey for.
        let _ = self.webdriver_delete("/actions").await;
        Ok(())
    }

    /// Reload the page (WebDriver's refresh). The app boots again on the Feed,
    /// which is the same `FeedModel.open()` call pull to refresh makes, so a
    /// journey has a second way to produce a Feed load when a runner cannot
    /// drive a touch drag (T-1101e).
    pub async fn reload(&self) -> Result<(), E2eError> {
        self.client.refresh().await.map_err(wd)
    }

    /// This browser session's WebDriver id.
    async fn session_id(&self) -> Result<String, E2eError> {
        self.client
            .session_id()
            .await
            .map_err(wd)?
            .ok_or_else(|| E2eError::State("no webdriver session id".to_owned()))
    }

    /// POST a raw WebDriver command to this session. The actions API (the one
    /// way to send a real touch drag) has no fantoccini wrapper at 0.21.
    async fn webdriver_post(&self, command: &str, body: Value) -> Result<(), E2eError> {
        let base = webdriver_url()?;
        let url = format!(
            "{}/session/{}{command}",
            base.trim_end_matches('/'),
            self.session_id().await?
        );
        let response = reqwest::Client::new()
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response.text().await.unwrap_or_default();
            Err(E2eError::WebDriver(format!(
                "POST {command}: {status} {text}"
            )))
        }
    }

    /// DELETE a raw WebDriver command for this session (releasing the input
    /// state the actions API keeps).
    async fn webdriver_delete(&self, command: &str) -> Result<(), E2eError> {
        let base = webdriver_url()?;
        let url = format!(
            "{}/session/{}{command}",
            base.trim_end_matches('/'),
            self.session_id().await?
        );
        let response = reqwest::Client::new()
            .delete(&url)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = response.text().await.unwrap_or_default();
            Err(E2eError::WebDriver(format!(
                "DELETE {command}: {status} {text}"
            )))
        }
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

/// The Gmail scope the fake's list route requires (S8); the label read-back
/// mints a token carrying it.
const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

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

    /// Seed one corpus one-click message into `email`'s mailbox with its
    /// `List-Unsubscribe` target rewritten to this run's testbed (S10 6.2).
    ///
    /// Only the target URL changes: every other header, including the DKIM
    /// coverage of both unsubscribe headers, stays the fixture's, so the
    /// message still classifies as a covered one-click list message. The target
    /// must be the testbed's literal-loopback TLS base
    /// (`MT_E2E_TESTBED_HTTPS_URL`); `unsub`'s e2e egress refuses a hostname.
    pub async fn seed_one_click_message(
        &self,
        email: &str,
        fixture: &str,
        fixture_target: &str,
        route: &str,
        testbed_https: &str,
        internal_date: &str,
    ) -> Result<(), E2eError> {
        let corpus = testkit::corpus::load().map_err(E2eError::State)?;
        let case = corpus
            .case(fixture)
            .ok_or_else(|| E2eError::State(format!("no corpus case {fixture}")))?;
        let text = String::from_utf8(case.eml.clone())
            .map_err(|e| E2eError::State(format!("corpus case {fixture} is not UTF-8: {e}")))?;
        let target = format!("{}{route}", testbed_https.trim_end_matches('/'));
        let eml = text.replace(fixture_target, &target);
        self.post(
            "/__fake/gmail/messages",
            json!({
                "email": email,
                "eml_base64": base64::engine::general_purpose::STANDARD.encode(eml),
                "labels": ["INBOX"],
                "internal_date": internal_date,
            }),
        )
        .await
        .map(|_| ())
    }

    /// The label *names* on every message in `email`'s mailbox, newest first,
    /// one inner list per message (journey 6's provider read-back, S10 3.3).
    ///
    /// The fake stores label IDs on a message, so the names come from the Gmail
    /// labels list; an ID with no name is returned as itself, which keeps an
    /// unexpected label visible to the caller's assertion.
    pub async fn message_labels(&self, email: &str) -> Result<Vec<Vec<String>>, E2eError> {
        let token = self.issue_read_token(email).await?;
        let names = self.label_names(&token).await?;
        let listed = self
            .get_json("/gmail/v1/users/me/messages?maxResults=500", &token)
            .await?;
        let ids: Vec<String> = listed
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let message = self
                .get_json(&format!("/gmail/v1/users/me/messages/{id}"), &token)
                .await?;
            let label_ids: Vec<String> = message
                .get("labelIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            out.push(
                label_ids
                    .iter()
                    .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone()))
                    .collect(),
            );
        }
        Ok(out)
    }

    /// The mailbox's label ID -> name map, from the Gmail labels list.
    async fn label_names(&self, token: &str) -> Result<HashMap<String, String>, E2eError> {
        let labels = self.get_json("/gmail/v1/users/me/labels", token).await?;
        Ok(labels
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| {
                        Some((
                            label.get("id")?.as_str()?.to_owned(),
                            label.get("name")?.as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Mint an access token for `email` with the scope the Gmail list route
    /// requires.
    async fn issue_read_token(&self, email: &str) -> Result<String, E2eError> {
        let value = self
            .post(
                "/__fake/tokens",
                json!({ "email": email, "scopes": [GMAIL_MODIFY_SCOPE], "ttl_s": 3600 }),
            )
            .await?;
        value
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| E2eError::State("token response had no access_token".to_owned()))
    }

    /// GET a fake-Google route with a bearer token and parse its JSON body.
    async fn get_json(&self, path: &str, token: &str) -> Result<Value, E2eError> {
        let response = self
            .client
            .get(endpoint(&self.base, path))
            .bearer_auth(token)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(http)?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(E2eError::Http(format!("GET {path}: {status} {value}")))
        }
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

// ---------------------------------------------------------------------------
// Journey helpers (T-1101b).
// ---------------------------------------------------------------------------

/// A request the one-click testbed recorded (S10 6.2): the method, the headers
/// (lower-cased names) and the raw body.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RecordedRequest {
    /// The HTTP method, upper case.
    pub method: String,
    /// Lower-cased header names and their values.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
}

/// The testbed's wire record: [`RecordedRequest`] plus the path we filter on.
#[derive(Deserialize)]
struct RecordedWire {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// The one-click unsubscribe testbed's read-back control API (`/__testbed`,
/// S10 6.2). Journeys assert exactly what a route received.
pub struct Testbed {
    base: Url,
    client: reqwest::Client,
}

impl Testbed {
    /// Bind the control client to the stack's unsub-testbed.
    pub fn connect(stack: &Stack) -> Result<Self, E2eError> {
        Ok(Self {
            base: stack.testbed.clone(),
            client: reqwest::Client::new(),
        })
    }

    /// Everything the testbed recorded for `route` (a path, without the query),
    /// in arrival order.
    pub async fn received(&self, route: &str) -> Result<Vec<RecordedRequest>, E2eError> {
        let response = self
            .client
            .get(endpoint(&self.base, "/__testbed/requests"))
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(E2eError::Http(format!(
                "GET /__testbed/requests: {status} {body}"
            )));
        }
        let records: Vec<RecordedWire> = response.json().await.map_err(http)?;
        Ok(records
            .into_iter()
            .filter(|record| record.path == route)
            .map(|record| RecordedRequest {
                method: record.method,
                headers: record.headers,
                body: record.body,
            })
            .collect())
    }

    /// Clear every recorded request, so a journey never sees another journey's
    /// traffic (S10 6.2). Journeys run one at a time, but the testbed is one
    /// process for the whole run.
    pub async fn reset(&self) -> Result<(), E2eError> {
        let response = self
            .client
            .post(endpoint(&self.base, "/__testbed/reset"))
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(E2eError::Http(format!(
                "POST /__testbed/reset: {status} {body}"
            )))
        }
    }

    /// Poll `route` until the testbed has recorded a request or `timeout`
    /// passes, then return what it recorded. A journey waits out a job's due
    /// time this way instead of sleeping for minutes: an empty result after the
    /// timeout is the assertion that nothing was sent (T-1101e).
    pub async fn wait_for_request(
        &self,
        route: &str,
        timeout: Duration,
    ) -> Result<Vec<RecordedRequest>, E2eError> {
        let deadline = Instant::now() + timeout;
        loop {
            let records = self.received(route).await?;
            if !records.is_empty() || Instant::now() >= deadline {
                return Ok(records);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// A byte offset per log file captured by [`EventLog::mark`], so each journey
/// reads only the lines written after it started.
#[derive(Clone, Debug, Default)]
pub struct LogMark {
    offsets: HashMap<PathBuf, u64>,
}

/// One metric event (S10 8) as T-307's schema writes it: the event type (the
/// wire key `action`) and the outcome code, with no content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricEvent {
    /// The metric type: `swipe`, `undo`, `unsub_outcome`, ...
    pub event_type: String,
    /// The outcome code, when the metric carries one.
    pub outcome: Option<String>,
}

/// One structured log line, read only for the fields the harness needs.
#[derive(Deserialize)]
struct LogLine {
    event: Option<String>,
    action: Option<String>,
    outcome: Option<String>,
    route: Option<String>,
}

/// Reads the services' structured log lines (`<service>.jsonl`, one JSON object
/// per line as T-307 writes them) from the e2e log directory.
pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    /// Read the run's log directory (`MT_E2E_LOG_DIR`, default
    /// `target/e2e-logs`, S10 3.3).
    pub fn new() -> Self {
        Self { dir: log_dir() }
    }

    /// Read a specific directory (unit tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Record the current byte length of every `*.jsonl` file, so a later
    /// [`Self::since_mark`] returns only the lines written after this point.
    pub fn mark(&self) -> LogMark {
        let offsets = self
            .jsonl_files()
            .into_iter()
            .map(|path| {
                let len = std::fs::metadata(&path).map_or(0, |meta| meta.len());
                (path, len)
            })
            .collect();
        LogMark { offsets }
    }

    /// Every metric event written since `mark`, across every log file. Earlier
    /// journeys' lines are outside the mark and never count.
    pub fn since_mark(&self, mark: LogMark) -> Result<Vec<MetricEvent>, E2eError> {
        Ok(self
            .lines_since(&mark)
            .into_iter()
            .filter_map(|line| metric_event(&line))
            .collect())
    }

    /// The route templates of every `request` line written since `mark`
    /// (T-307's request event; the api logs one per request with its template).
    /// A journey uses it to prove a Feed load happened - `/api/v1/feed/next` -
    /// which is how a pull to refresh is told from a gesture that did nothing
    /// (T-1101e).
    pub fn request_routes_since_mark(&self, mark: &LogMark) -> Result<Vec<String>, E2eError> {
        Ok(self
            .lines_since(mark)
            .into_iter()
            .filter_map(|line| request_route(&line))
            .collect())
    }

    /// The non-empty log lines written since `mark`, across every log file.
    fn lines_since(&self, mark: &LogMark) -> Vec<String> {
        let mut lines = Vec::new();
        for path in self.jsonl_files() {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            // A file created after the mark, or one that shrank, is read whole.
            let start = mark
                .offsets
                .get(&path)
                .copied()
                .unwrap_or(0)
                .min(bytes.len() as u64) as usize;
            let text = String::from_utf8_lossy(&bytes[start..]);
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                lines.push(line.to_owned());
            }
        }
        lines
    }

    /// The `*.jsonl` files in the directory, sorted for a stable order.
    fn jsonl_files(&self) -> Vec<PathBuf> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        files.sort();
        files
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse one log line into a [`MetricEvent`], or `None` for any other event.
fn metric_event(line: &str) -> Option<MetricEvent> {
    let parsed: LogLine = serde_json::from_str(line).ok()?;
    if parsed.event.as_deref() != Some("metric") {
        return None;
    }
    Some(MetricEvent {
        event_type: parsed.action?,
        outcome: parsed.outcome,
    })
}

/// The route template of one `request` log line, or `None` for any other event.
fn request_route(line: &str) -> Option<String> {
    let parsed: LogLine = serde_json::from_str(line).ok()?;
    if parsed.event.as_deref() != Some("request") {
        return None;
    }
    parsed.route
}

/// The invited copy on the Sign-in screen (S2 AU-03 AC1; `Copy.invited`), shown
/// before the OAuth round-trip.
pub const INVITED_COPY: &str =
    "You've been invited. Continue with the Google account the invite was sent to.";
/// The Google button's Semantics label (XC-03).
pub const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
/// The Reject button's Semantics label: proves the Feed rendered a card.
pub const REJECT_BUTTON: &str = "Reject";
/// The api's route template for a Feed load, exactly as T-307's `request`
/// event logs it. A journey reads it back from `api.jsonl` to tell a Feed load
/// that happened from one that did not (T-1101e).
pub const FEED_ROUTE: &str = "/api/v1/feed/next";
/// A cold CI runner takes a long time to paint the app before the semantics
/// tree exposes a control, so the first wait is generous.
pub const APP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// The Feed after the OAuth round-trip, once the api has fetched and classified.
pub const FEED_TIMEOUT: Duration = Duration::from_secs(120);
/// The OAuth round-trip from the invite screen back into the app.
pub const REDIRECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for a tapped button to react before re-tapping it.
pub const TAP_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// The sender display name of the newest fixture (the last id in `fixtures`,
/// which the harness seeds last, so it is the first card). `None` when
/// `fixtures` is empty.
fn newest_sender(fixtures: &[&str]) -> Result<Option<String>, E2eError> {
    let Some(id) = fixtures.last() else {
        return Ok(None);
    };
    let corpus = testkit::corpus::load().map_err(E2eError::State)?;
    let case = corpus
        .cases
        .iter()
        .find(|case| case.spec.id == *id)
        .ok_or_else(|| E2eError::State(format!("no corpus case {id}")))?;
    Ok(Some(case.spec.from_display.clone()))
}

/// Sign a seeded account in through the UI and return a ready [`Ui`] on the
/// Feed (T-1101b). The caller resets fake-google and registers the client once;
/// this seeds `sub`/`email`, seeds `fixtures` mail when given, then drives the
/// invite and OAuth round-trip exactly as the sign-in journey does (AU-03 AC1).
pub async fn signed_in_user(
    stack: &Stack,
    sub: &str,
    email: &str,
    fixtures: &[&str],
) -> Result<Ui, E2eError> {
    let google = FakeGoogle::connect(stack)?;
    let control = TestControl::connect(stack)?;
    google.seed_account(sub, email, true).await?;
    if !fixtures.is_empty() {
        google.seed_messages(sub, fixtures).await?;
    }
    google.select_account_for_next_authorize(sub).await?;
    let token = control.create_invite(email).await?;

    let ui = Ui::open(stack, &format!("/#/invite?t={token}")).await?;
    ui.wait_for_text(INVITED_COPY, APP_LOAD_TIMEOUT).await?;
    // Tap, then confirm the control reacted: the button swaps to a spinner, so
    // its label disappears as soon as the handler runs. A lost click is
    // re-tapped once before giving up.
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
    let app_origin = stack.app_url.as_str().trim_end_matches('/').to_owned();
    ui.wait_for_url(&app_origin, "/invite", REDIRECT_TIMEOUT)
        .await?;
    match newest_sender(fixtures)? {
        Some(sender) => ui.wait_for_text(&sender, FEED_TIMEOUT).await?,
        None => ui.wait_for_text(REJECT_BUTTON, FEED_TIMEOUT).await?,
    }
    Ok(ui)
}

/// The prefix of a [`finish_journey`] marker file; T-1101c's leak scan reads it.
const FINISH_MARKER_PREFIX: &str = "JOURNEYS_FINISHED-";

/// End a journey: `save_browser_dump` then `assert_no_csp_violation`. Journeys
/// call it explicitly as their last statement (async work cannot run reliably in
/// `Drop`, so there is no `Drop`-based variant).
///
/// T-1101b defines this with a stub body: it records the call by writing one
/// `JOURNEYS_FINISHED-<test name>` marker file under the log directory and
/// releases the browser. T-1101c replaces the body with the real
/// `save_browser_dump` and `assert_no_csp_violation` and adds a `zz_leak_scan.rs`
/// check that every journey test wrote a marker.
pub async fn finish_journey(ui: Ui) -> Result<(), E2eError> {
    let dir = log_dir();
    let name = std::thread::current()
        .name()
        .unwrap_or("unknown")
        .to_owned();
    std::fs::create_dir_all(&dir)
        .map_err(|e| E2eError::State(format!("finish_journey log dir: {e}")))?;
    std::fs::write(dir.join(format!("{FINISH_MARKER_PREFIX}{name}")), &name)
        .map_err(|e| E2eError::State(format!("finish_journey marker: {e}")))?;
    // Best-effort: release the browser session; a cleanup hiccup must not hide a
    // real failure.
    let _ = ui.close().await;
    Ok(())
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
  .filter(el => el.getAttribute('aria-label') === label || (el.textContent || '').trim() === label);
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

/// Tag the tappable node whose merged `aria-label` has `arguments[0]` as one
/// of its parts (Flutter joins merged labels with a newline), or whose text is
/// exactly `arguments[0]`. See [`Ui::tap_merged`]. Returns false when nothing
/// matches.
const MARK_MERGED_JS: &str = r#"
const label = arguments[0];
const hasPart = el => (el.getAttribute('aria-label') || '').split('\n')
  .some(part => part.trim() === label);
const matches = Array.from(document.querySelectorAll('flt-semantics, [role], [aria-label], button'))
  .filter(el => hasPart(el) || (el.textContent || '').trim() === label);
if (matches.length === 0) return false;
document.querySelectorAll('[data-e2e-tap]').forEach(el => el.removeAttribute('data-e2e-tap'));
matches.sort((a, b) => (b.hasAttribute('flt-tappable') ? 1 : 0) - (a.hasAttribute('flt-tappable') ? 1 : 0));
matches[0].setAttribute('data-e2e-tap', '');
return true;
"#;

/// The point a pull to refresh starts from: inside the Feed's scroll view,
/// above the card and below the header. The header's last row is the idle
/// Blitz button (its Semantics label is `Copy.blitzSemantics`), so its bottom
/// edge is the top of the scroll view. That node carries its label as text
/// content (a `Semantics`-wrapped `TextButton`), while other controls carry it
/// as `aria-label`, so both forms are matched. Returns null when the Feed is
/// not showing, so the caller reports a miss instead of dragging blind.
const PULL_START_JS: &str = r#"
const label = 'Start a 60-second Blitz round';
const blitz = Array.from(document.querySelectorAll('flt-semantics, [aria-label]'))
  .find(el => (el.getAttribute('aria-label') || '').startsWith(label))
  || Array.from(document.querySelectorAll('flt-semantics[role="button"]'))
    .find(el => (el.textContent || '').trim().startsWith(label));
if (!blitz) return null;
const rect = blitz.getBoundingClientRect();
return { x: rect.left + rect.width / 2, y: rect.bottom + 8 };
"#;

/// DOM attribute [`MARK_TYPE_JS`] sets on the editable control and the CSS
/// selector WebDriver finds it by.
const MARKED_TYPE_SELECTOR: &str = "[data-e2e-type]";

/// Tag the editable `input`/`textarea` carrying the label `arguments[0]`. A
/// text field's label sits on the editable element (its `<input>`/`<textarea>`
/// inside the `flt-semantics` host) as `aria-label`, or as `aria-description`
/// for a hint or error - not on the host node, which is why the host's own
/// `aria-label` is the wrong place to look. Returns false when nothing matches.
const MARK_TYPE_JS: &str = r#"
const label = arguments[0];
const matches = Array.from(document.querySelectorAll('input, textarea'))
  .filter(el => el.getAttribute('aria-label') === label || el.getAttribute('aria-description') === label);
if (matches.length === 0) return false;
document.querySelectorAll('[data-e2e-type]').forEach(el => el.removeAttribute('data-e2e-type'));
matches[0].setAttribute('data-e2e-type', '');
return true;
"#;

/// True when the tagged editable control holds `arguments[0]`: proves the
/// framework took the keys, not just that they reached the DOM element.
const TYPED_VALUE_JS: &str = r#"
const el = document.querySelector('[data-e2e-type]');
return !!el && el.value === arguments[0];
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh, unique directory under this crate's `target` tree. It is
    /// deliberately not the system temporary directory: a fixed, predictable
    /// name there could be pre-created by another process, and the security
    /// scan forbids it for exactly that reason.
    fn temp_log_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("e2e-log-tests")
            .join(format!("case-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create log dir");
        dir
    }

    #[test]
    fn metric_event_reads_the_t307_schema_fields() {
        let event = metric_event(
            "{\"event\":\"metric\",\"action\":\"unsub_outcome\",\"outcome\":\"sent\",\"user_pseudo\":\"00\"}",
        )
        .expect("a metric line parses");
        assert_eq!(event.event_type, "unsub_outcome");
        assert_eq!(event.outcome.as_deref(), Some("sent"));

        // A metric without an outcome keeps the type and no outcome.
        let bare = metric_event("{\"event\":\"metric\",\"action\":\"swipe\"}")
            .expect("a metric line without outcome parses");
        assert_eq!(bare.event_type, "swipe");
        assert_eq!(bare.outcome, None);

        // Non-metric events and missing `action` are not metric events.
        assert_eq!(
            metric_event("{\"event\":\"request\",\"action\":\"unsub_outcome\"}"),
            None
        );
        assert_eq!(metric_event("{\"event\":\"metric\"}"), None);
        assert_eq!(metric_event("not json"), None);
    }

    #[test]
    fn request_routes_since_mark_reads_only_the_request_event() -> Result<(), E2eError> {
        let dir = temp_log_dir();
        let api = dir.join("api.jsonl");
        std::fs::write(
            &api,
            "{\"event\":\"request\",\"route\":\"/api/v1/session\",\"status\":200}\n",
        )
        .expect("seed an earlier request line");
        let log = EventLog::at(dir.clone());
        let mark = log.mark();

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&api)
            .expect("open the api log");
        writeln!(
            file,
            "{{\"event\":\"request\",\"route\":\"{FEED_ROUTE}\",\"status\":200}}"
        )
        .expect("append a feed request");
        writeln!(
            file,
            "{{\"event\":\"metric\",\"action\":\"unsub_outcome\",\"outcome\":\"sent\"}}"
        )
        .expect("append a metric line");

        let routes = log.request_routes_since_mark(&mark)?;
        assert_eq!(routes, vec![FEED_ROUTE.to_owned()]);

        // A request line without a route, a non-request line and junk are all
        // skipped rather than counted as a Feed load.
        assert_eq!(request_route("{\"event\":\"request\"}"), None);
        assert_eq!(
            request_route("{\"event\":\"metric\",\"route\":\"/x\"}"),
            None
        );
        assert_eq!(request_route("not json"), None);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn since_mark_reads_only_lines_written_after_the_mark() -> Result<(), E2eError> {
        let dir = temp_log_dir();
        let api = dir.join("api.jsonl");
        std::fs::write(&api, "{\"event\":\"metric\",\"action\":\"swipe\"}\n")
            .expect("seed an earlier line");
        let log = EventLog::at(dir.clone());
        let mark = log.mark();

        // After the mark: another line in an existing file and a new file.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&api)
            .expect("open the api log");
        writeln!(
            file,
            "{{\"event\":\"metric\",\"action\":\"unsub_outcome\",\"outcome\":\"sent\"}}"
        )
        .expect("append a metric line");
        std::fs::write(
            dir.join("unsub.jsonl"),
            "{\"event\":\"metric\",\"action\":\"unsub_outcome\",\"outcome\":\"cancelled\"}\n",
        )
        .expect("write a new log file");

        let events = log.since_mark(mark)?;
        assert_eq!(events.len(), 2, "only the two post-mark lines count");
        assert!(events.iter().all(|e| e.event_type == "unsub_outcome"));
        assert!(events
            .iter()
            .any(|e| e.outcome.as_deref() == Some("cancelled")));

        // A mark taken now sees nothing new.
        let quiet = log.mark();
        assert!(log.since_mark(quiet)?.is_empty());

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
