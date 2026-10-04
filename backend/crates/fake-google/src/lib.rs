//! `fake-google`: an `axum` HTTP fake of Gmail, Drive, OAuth and OIDC.
//!
//! Usable as a library inside tests (binds port 0) and as a binary for e2e.
//! T-205a covers the Gmail REST fake and the `/__fake` control API; T-205b adds
//! Drive; T-206 adds OAuth and OIDC.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use time::OffsetDateTime;
use url::Url;

pub mod control;
pub mod errors;
pub mod gmail;
pub mod mime;
pub mod seeder;
pub mod state;
pub mod tokens;

pub use state::{FailRule, FakeEvent, FakeMailboxKey};
pub use tokens::{DRIVE_APPDATA, GMAIL_MODIFY, GMAIL_SEND};

use gmail::AppState;
use state::FakeState;

/// The fake-google server. `start` binds `127.0.0.1:0` and serves until the
/// returned handle is dropped.
pub struct FakeGoogle;

impl FakeGoogle {
    pub async fn start(clock: Arc<dyn ports::Clock>) -> Result<FakeGoogleHandle, std::io::Error> {
        let state = Arc::new(std::sync::Mutex::new(FakeState::default()));
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown_guard = Arc::new(ShutdownGuard {
            _tx: shutdown_tx,
            _clock: clock,
        });
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        tokio::spawn(async move {
            let _ = server.await;
        });
        Ok(FakeGoogleHandle {
            addr,
            state,
            _shutdown: shutdown_guard,
        })
    }
}

/// Assemble the full router: Gmail + control.
pub fn build_router(state: Arc<std::sync::Mutex<FakeState>>) -> Router {
    let app_state = AppState(state);
    Router::new()
        .nest("/gmail/v1/users/me", gmail::router())
        .nest("/__fake", control::router())
        .with_state(app_state)
}

/// A running fake-google instance.
#[derive(Clone)]
pub struct FakeGoogleHandle {
    pub addr: SocketAddr,
    state: Arc<std::sync::Mutex<FakeState>>,
    _shutdown: Arc<ShutdownGuard>,
}

struct ShutdownGuard {
    _tx: tokio::sync::oneshot::Sender<()>,
    _clock: Arc<dyn ports::Clock>,
}

impl FakeGoogleHandle {
    pub fn base_url(&self) -> Url {
        Url::parse(&format!("http://{}/", self.addr)).expect("base url")
    }

    pub fn add_mailbox(&self, email: &str) -> FakeMailboxKey {
        let mut st = self.state.lock().unwrap();
        st.mailbox(email);
        FakeMailboxKey(email.to_owned())
    }

    pub fn issue_token(&self, mb: &FakeMailboxKey, scopes: &[&str], ttl: time::Duration) -> String {
        let mut st = self.state.lock().unwrap();
        let token = format!("tok-{}", uuid::Uuid::new_v4());
        st.tokens.insert(
            token.clone(),
            tokens::TokenRecord {
                mailbox: mb.clone(),
                scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
                expires_at: OffsetDateTime::now_utc() + ttl,
            },
        );
        token
    }

    pub fn expire_token(&self, token: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(rec) = st.tokens.get_mut(token) {
            rec.expires_at = OffsetDateTime::UNIX_EPOCH;
        }
    }

    pub fn seed_eml(
        &self,
        mb: &FakeMailboxKey,
        eml: &[u8],
        labels: &[&str],
        internal_date: OffsetDateTime,
    ) -> String {
        let mut st = self.state.lock().unwrap();
        let id = st.next_message_id(&mb.0);
        let m = st.mailboxes.get_mut(&mb.0).expect("mailbox");
        m.messages.insert(
            id.clone(),
            state::StoredMessage {
                id: id.clone(),
                raw: eml.to_vec(),
                labels: labels.iter().map(|s| (*s).to_owned()).collect(),
                internal_date,
            },
        );
        id
    }

    pub fn labels_of(&self, mb: &FakeMailboxKey, id: &str) -> Option<BTreeSet<String>> {
        let st = self.state.lock().unwrap();
        st.mailboxes
            .get(&mb.0)
            .and_then(|m| m.messages.get(id))
            .map(|m| m.labels.clone())
    }

    pub fn sent(&self, mb: &FakeMailboxKey) -> Vec<Vec<u8>> {
        let st = self.state.lock().unwrap();
        st.mailboxes
            .get(&mb.0)
            .map(|m| m.sent.clone())
            .unwrap_or_default()
    }

    pub fn fail(&self, rule: FailRule) {
        let mut st = self.state.lock().unwrap();
        st.fail_rules.push(rule);
    }

    pub fn arm_label_create_race(&self, mb: &FakeMailboxKey) {
        let mut st = self.state.lock().unwrap();
        st.label_create_race.insert(mb.0.clone());
    }

    pub fn events(&self) -> Vec<FakeEvent> {
        let st = self.state.lock().unwrap();
        st.events.clone()
    }

    pub fn permanent_delete_attempts(&self) -> u64 {
        let st = self.state.lock().unwrap();
        st.events
            .iter()
            .filter(|e| matches!(e, FakeEvent::PermanentDeleteAttempted { .. }))
            .count() as u64
    }

    pub fn seeder(
        &self,
        mb: &FakeMailboxKey,
    ) -> Arc<dyn testkit::contract::mail_provider::MailSeeder> {
        Arc::new(seeder::FakeGoogleSeeder::new(
            self.state.clone(),
            mb.clone(),
        ))
    }
}
