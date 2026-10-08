//! Browser journeys against the synthetic, loopback-only e2e stack.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use fantoccini::{Client, ClientBuilder, Locator};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const WAIT: Duration = Duration::from_secs(30);

/// Failures in stack configuration, browser control or fixture setup.
#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    #[error("missing or invalid environment variable {0}")]
    Environment(&'static str),
    #[error("e2e endpoints must use loopback HTTP without credentials")]
    NonLocalEndpoint,
    #[error("invalid URL")]
    Url(#[from] url::ParseError),
    #[error("HTTP control request failed")]
    Http(#[from] reqwest::Error),
    #[error("browser command failed")]
    Browser(#[source] Box<fantoccini::error::CmdError>),
    #[error("browser session failed")]
    Session(#[source] Box<fantoccini::error::NewSessionError>),
    #[error("browser TLS setup failed")]
    Io(#[from] std::io::Error),
    #[error("invalid fixture or control response: {0}")]
    Control(String),
    #[error("timed out waiting for browser state")]
    Timeout,
    #[error("invalid browser response")]
    BrowserResponse,
    #[error("JSON conversion failed")]
    Json(#[from] serde_json::Error),
}

impl From<fantoccini::error::CmdError> for E2eError {
    fn from(error: fantoccini::error::CmdError) -> Self {
        Self::Browser(Box::new(error))
    }
}

/// Addresses exported by `scripts/e2e.sh`.
#[derive(Debug, Clone)]
pub struct Stack {
    pub app_url: Url,
    pub fake_google: Url,
    pub testbed: Url,
    pub api_internal: Url,
}

impl Stack {
    /// Read and validate the local stack addresses.
    ///
    /// # Errors
    /// Returns an error for missing variables or non-loopback endpoints.
    pub fn from_env() -> Result<Self, E2eError> {
        Ok(Self {
            app_url: env_url("MT_E2E_APP_URL")?,
            fake_google: env_url("MT_E2E_FAKE_GOOGLE_URL")?,
            testbed: env_url("MT_E2E_TESTBED_URL")?,
            api_internal: env_url("MT_E2E_API_URL")?,
        })
    }
}

fn env_url(name: &'static str) -> Result<Url, E2eError> {
    let value = std::env::var(name).map_err(|_| E2eError::Environment(name))?;
    local_url(&value)
}

fn local_url(value: &str) -> Result<Url, E2eError> {
    let url = Url::parse(value)?;
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !local || url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return Err(E2eError::NonLocalEndpoint);
    }
    Ok(url)
}

/// One isolated headless Chrome session, controlled through semantic labels.
pub struct Ui {
    client: Client,
    webdriver: Url,
    main_window: fantoccini::wd::WindowHandle,
}

impl Ui {
    /// Launch Chrome and navigate to a fragment route.
    ///
    /// `MT_E2E_WEBDRIVER_URL` is the local `ChromeDriver` endpoint.
    /// # Errors
    /// Returns configuration, browser session or navigation errors.
    pub async fn open(stack: &Stack, hash_path: &str) -> Result<Self, E2eError> {
        // fantoccini enables aws-lc while reqwest enables ring. Select one
        // explicitly rather than relying on rustls's ambiguous feature inference.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let webdriver = env_url("MT_E2E_WEBDRIVER_URL")?;
        let capabilities = json!({
            "browserName": "chrome",
            "goog:chromeOptions": {"args": ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--window-size=1280,1000"]},
            "goog:loggingPrefs": {"browser": "ALL"}
        });
        let client = ClientBuilder::rustls()?
            .capabilities(serde_json::from_value(capabilities)?)
            .connect(webdriver.as_str())
            .await
            .map_err(|error| E2eError::Session(Box::new(error)))?;
        let mut url = stack.app_url.clone();
        url.set_fragment(Some(hash_path.trim_start_matches('#')));
        let navigation = async {
            let window = client.window().await?;
            client.goto(url.as_str()).await?;
            Ok::<_, E2eError>(window)
        }
        .await;
        match navigation {
            Ok(main_window) => Ok(Self {
                client,
                webdriver,
                main_window,
            }),
            Err(error) => {
                client.close().await?;
                Err(error)
            }
        }
    }

    /// Click a control identified by its exact semantic label.
    /// # Errors
    /// Returns a browser error if the control never appears or cannot be clicked.
    pub async fn tap(&self, label: &str) -> Result<(), E2eError> {
        let selector = semantic_selector(label);
        self.client
            .wait()
            .at_most(WAIT)
            .for_element(Locator::Css(&selector))
            .await?
            .click()
            .await?;
        Ok(())
    }

    /// Clear and type into the native input beneath a semantic node.
    /// # Errors
    /// Returns a browser error if the labelled field is absent or not editable.
    pub async fn type_into(&self, label: &str, text: &str) -> Result<(), E2eError> {
        let semantic = semantic_selector(label);
        let selector = format!("{semantic} input, {semantic} textarea, input[aria-label=\"{}\"], textarea[aria-label=\"{}\"]", css_string(label), css_string(label));
        let field = self
            .client
            .wait()
            .at_most(WAIT)
            .for_element(Locator::Css(&selector))
            .await?;
        field.clear().await?;
        field.send_keys(text).await?;
        Ok(())
    }

    /// Wait for visible Flutter semantics containing the requested copy.
    /// # Errors
    /// Returns a timeout or browser error; hidden DOM text does not satisfy the wait.
    pub async fn wait_for_text(&self, text: &str, timeout: Duration) -> Result<(), E2eError> {
        tokio::time::timeout(timeout, async {
            loop {
                let found = self.client.execute(
                    "return Array.from(document.querySelectorAll('flt-semantics')).some(e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden' && ((e.getAttribute('aria-label') || '').includes(arguments[0]) || (e.textContent || '').includes(arguments[0])); });",
                    vec![json!(text)],
                ).await?;
                if found.as_bool() == Some(true) { return Ok(()); }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await.map_err(|_| E2eError::Timeout)?
    }

    /// Switch to an OAuth popup rather than the original window.
    /// # Errors
    /// Returns a timeout if no second window appears, or a browser error.
    pub async fn switch_to_popup(&self) -> Result<(), E2eError> {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(window) = self
                    .client
                    .windows()
                    .await?
                    .into_iter()
                    .find(|window| window != &self.main_window)
                {
                    self.client.switch_to_window(window).await?;
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| E2eError::Timeout)?
    }

    /// Snapshot Web Storage contents, `IndexedDB` names and Cache Storage keys.
    /// # Errors
    /// Returns a browser error if any storage API cannot be read.
    pub async fn storage_dump(&self) -> Result<String, E2eError> {
        let value = self.client.execute_async(
            "const done = arguments[arguments.length - 1]; Promise.all([indexedDB.databases(), caches.keys()]).then(([dbs, keys]) => done(JSON.stringify({localStorage: Object.fromEntries(Object.keys(localStorage).map(k => [k, localStorage.getItem(k)])), sessionStorage: Object.fromEntries(Object.keys(sessionStorage).map(k => [k, sessionStorage.getItem(k)])), indexedDB: dbs.map(d => d.name), caches: keys})), e => done({error: e.name}));",
            vec![],
        ).await?;
        value
            .as_str()
            .map(str::to_owned)
            .ok_or(E2eError::BrowserResponse)
    }

    /// Drain Chrome's browser log and return severe entries, including CSP errors.
    /// # Errors
    /// Returns an HTTP or decoding error if `ChromeDriver`'s log command fails.
    pub async fn console_errors(&self) -> Result<Vec<String>, E2eError> {
        let session = self
            .client
            .session_id()
            .await?
            .ok_or(E2eError::BrowserResponse)?;
        let endpoint = self.webdriver.join(&format!("session/{session}/se/log"))?;
        let response: Value = reqwest::Client::new()
            .post(endpoint)
            .json(&json!({"type": "browser"}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let entries = response["value"]
            .as_array()
            .ok_or(E2eError::BrowserResponse)?;
        entries
            .iter()
            .filter(|entry| entry["level"] == "SEVERE")
            .map(|entry| {
                entry["message"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(E2eError::BrowserResponse)
            })
            .collect()
    }

    /// End the Chrome session, including its windows.
    /// # Errors
    /// Returns a browser error if session deletion fails.
    pub async fn close(self) -> Result<(), E2eError> {
        self.client.close().await?;
        Ok(())
    }
}

fn css_string(value: &str) -> String {
    use std::fmt::Write as _;

    value.chars().fold(String::new(), |mut escaped, character| {
        // Writing into a String is infallible.
        let _ = write!(escaped, "\\{:x} ", u32::from(character));
        escaped
    })
}

fn semantic_selector(label: &str) -> String {
    format!("flt-semantics[aria-label=\"{}\"]", css_string(label))
}

#[derive(Clone)]
struct Account {
    email: String,
    verified: bool,
}

/// Control client for fake Google's real mailbox and next-login routes.
pub struct FakeGoogle {
    http: reqwest::Client,
    base: Url,
    accounts: std::sync::Mutex<BTreeMap<String, Account>>,
}

impl FakeGoogle {
    /// Bind the helper to the synthetic stack.
    #[must_use]
    pub fn new(stack: &Stack) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: stack.fake_google.clone(),
            accounts: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Seed a mailbox and remember its OAuth identity for later selection.
    /// # Errors
    /// Returns HTTP/control errors or an error if the account map is poisoned.
    pub async fn seed_account(
        &self,
        sub: &str,
        email: &str,
        verified: bool,
    ) -> Result<(), E2eError> {
        post_control(
            &self.http,
            &self.base,
            "/__fake/gmail/mailboxes",
            &json!({"email": email}),
        )
        .await?;
        self.accounts
            .lock()
            .map_err(|_| E2eError::Control("account map poisoned".into()))?
            .insert(
                sub.to_owned(),
                Account {
                    email: email.to_owned(),
                    verified,
                },
            );
        Ok(())
    }

    fn account(&self, sub: &str) -> Result<Account, E2eError> {
        self.accounts
            .lock()
            .map_err(|_| E2eError::Control("account map poisoned".into()))?
            .get(sub)
            .cloned()
            .ok_or_else(|| E2eError::Control("account not seeded".into()))
    }

    /// Arm the next OAuth authorisation to approve the selected identity.
    /// # Errors
    /// Returns an error for unknown accounts or failed control requests.
    pub async fn select_account_for_next_authorize(&self, sub: &str) -> Result<(), E2eError> {
        let account = self.account(sub)?;
        post_control(&self.http, &self.base, "/__fake/identity/next-login", &json!({"sub": sub, "email": account.email, "email_verified": account.verified, "amr": null, "auth_age_s": 0, "outcome": "approve"})).await?;
        Ok(())
    }

    /// Seed only the requested real corpus cases with their deterministic dates and labels.
    /// # Errors
    /// Returns an error for unknown fixture IDs, unknown accounts or failed requests.
    pub async fn seed_messages(&self, sub: &str, fixture_ids: &[&str]) -> Result<(), E2eError> {
        let account = self.account(sub)?;
        let corpus = testkit::corpus::load().map_err(E2eError::Control)?;
        // Validate the complete selection before mutating the fake mailbox.
        let cases = fixture_ids
            .iter()
            .map(|id| {
                corpus
                    .cases
                    .iter()
                    .find(|case| case.spec.id == *id)
                    .ok_or_else(|| E2eError::Control("unknown corpus fixture".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for case in cases {
            post_control(&self.http, &self.base, "/__fake/gmail/messages", &json!({"email": account.email, "eml_base64": base64::engine::general_purpose::STANDARD.encode(&case.eml), "labels": case.spec.labels, "internal_date": case.spec.date})).await?;
        }
        Ok(())
    }
}

/// Client for API routes available only with the `testkit` feature.
pub struct TestControl {
    http: reqwest::Client,
    base: Url,
}

impl TestControl {
    /// Bind the helper to the API's local internal origin.
    #[must_use]
    pub fn new(stack: &Stack) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: stack.api_internal.clone(),
        }
    }

    /// Create an invite through the normal invite service and return its raw token.
    /// # Errors
    /// Returns a control error if no nonempty token is returned.
    pub async fn create_invite(&self, email: &str) -> Result<String, E2eError> {
        #[derive(Deserialize)]
        struct Invite {
            token: String,
        }
        let response = post_control(
            &self.http,
            &self.base,
            "/internal/test/invites",
            &json!({"email": email}),
        )
        .await?;
        let invite: Invite = serde_json::from_value(response)?;
        if invite.token.is_empty() {
            return Err(E2eError::Control("empty invite token".into()));
        }
        Ok(invite.token)
    }

    /// Advance service virtual time; this never sleeps in real time.
    /// # Errors
    /// Returns an error for subsecond durations or failed control requests.
    pub async fn advance_clock(&self, by: Duration) -> Result<(), E2eError> {
        if by.subsec_nanos() != 0 {
            return Err(E2eError::Control(
                "clock advancement requires whole seconds".into(),
            ));
        }
        post_control(
            &self.http,
            &self.base,
            "/internal/test/advance-clock",
            &json!({"seconds": by.as_secs()}),
        )
        .await?;
        Ok(())
    }
}

async fn post_control(
    http: &reqwest::Client,
    base: &Url,
    path: &str,
    body: &Value,
) -> Result<Value, E2eError> {
    let response = http
        .post(base.join(path)?)
        .json(body)
        .send()
        .await?
        .error_for_status()?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    let response: Value = response.json().await?;
    if response.get("error").is_some() || response.get("ok") == Some(&Value::Bool(false)) {
        return Err(E2eError::Control(
            "control endpoint rejected request".into(),
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn au_03_ac1_e2e_stack_rejects_nonlocal_endpoints() -> Result<(), E2eError> {
        for value in [
            "https://accounts.google.com",
            "http://example.com",
            "http://secret@localhost",
            "http://localhost:80@google.com",
        ] {
            assert!(local_url(value).is_err());
        }
        for value in [
            "http://localhost:1234",
            "http://127.0.0.1:1234",
            "http://[::1]:1234",
        ] {
            local_url(value)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn au_03_ac1_e2e_seed_messages_uses_actual_fake_routes() -> Result<(), E2eError> {
        let clock = std::sync::Arc::new(testkit::clock::VirtualClock::new(testkit::clock::T0));
        let server = fake_google::FakeGoogle::start(clock).await?;
        let base = server.base_url();
        let stack = Stack {
            app_url: base.clone(),
            fake_google: base.clone(),
            testbed: base.clone(),
            api_internal: base.clone(),
        };
        let google = FakeGoogle::new(&stack);
        google
            .seed_account("route-test", "invitee@example.com", true)
            .await?;
        google
            .select_account_for_next_authorize("route-test")
            .await?;
        google
            .seed_messages(
                "route-test",
                &[
                    "one-click-covered",
                    "one-click-plus-mailto",
                    "esp-pass-dmarc-fail",
                ],
            )
            .await?;
        assert!(google
            .seed_messages("route-test", &["not-a-fixture"])
            .await
            .is_err());
        let token = post_control(&google.http, &base, "/__fake/tokens", &json!({"email": "invitee@example.com", "scopes": [fake_google::GMAIL_MODIFY], "ttl_s": 3600})).await?;
        let access_token = token["access_token"]
            .as_str()
            .ok_or(E2eError::BrowserResponse)?;
        let messages: Value = google
            .http
            .get(base.join("/gmail/v1/users/me/messages")?)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let listed = messages["messages"]
            .as_array()
            .ok_or(E2eError::BrowserResponse)?;
        assert_eq!(listed.len(), 3);
        let id = listed
            .first()
            .and_then(|message| message["id"].as_str())
            .ok_or(E2eError::BrowserResponse)?;
        let newest: Value = google
            .http
            .get(base.join(&format!("/gmail/v1/users/me/messages/{id}"))?)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let headers = newest["payload"]["headers"]
            .as_array()
            .ok_or(E2eError::BrowserResponse)?;
        assert!(headers.iter().any(|header| header["name"] == "From"
            && header["value"].as_str().is_some_and(
                |value| value.contains("Acme Deals CANARY-esp-pass-dmarc-fail-display")
            )));
        Ok(())
    }

    #[test]
    fn au_03_ac1_e2e_semantic_selector_escapes_untrusted_labels() {
        assert_eq!(
            semantic_selector("\"'\\"),
            "flt-semantics[aria-label=\"\\22 \\27 \\5c \"]"
        );
    }
}
