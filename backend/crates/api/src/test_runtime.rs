//! Local-only adapter wiring and controls, absent unless `testkit` is enabled.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use crate::{config::ApiConfig, error::ApiError, state::AppState};
use adapters_gcp::{FirestoreConfig, FirestoreStore, GcpHttp, StaticTokenSource};
use adapters_gmail::identity::{GoogleIdentity, GoogleIdentityConfig};
use adapters_gmail::{DriveAppFolder, GmailHttp, GmailProvider};
use async_trait::async_trait;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use domain::EmailAddress;
use obs::Sensitive;
use ports::{EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, OneClickOutcome};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
use url::Url;

type SetupResult<T> = Result<T, Box<dyn std::error::Error>>;
static CLOCK: OnceLock<Arc<testkit::VirtualClock>> = OnceLock::new();
fn clock() -> Arc<testkit::VirtualClock> {
    CLOCK
        .get_or_init(|| Arc::new(testkit::VirtualClock::new(testkit::T0)))
        .clone()
}

/// Restricts every outbound request to the configured synthetic server socket.
pub struct LoopbackEgress {
    origin: Url,
}
#[async_trait]
impl HttpEgress for LoopbackEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }
    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        if req.url.origin() != self.origin.origin()
            || !req.url.username().is_empty()
            || req.url.password().is_some()
        {
            return Err(EgressError::HostNotAllowed);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(req.timeout)
            .build()
            .map_err(|_| EgressError::Connect)?;
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut request = client.request(method, req.url);
        for (key, value) in req.headers {
            request = request.header(key, value.expose());
        }
        if let Some(body) = req.body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(|_| EgressError::Connect)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
            .collect();
        let body = response
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

fn local_url(name: &str) -> SetupResult<Url> {
    let url = Url::parse(&std::env::var(name)?)?;
    if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
        return Err("test stack requires loopback HTTP".into());
    }
    Ok(url)
}

/// Build the actual service ports with Google adapters and the Firestore emulator.
pub fn stack_state() -> SetupResult<AppState> {
    let fake = local_url("MT_E2E_FAKE_GOOGLE_URL")?;
    let app = local_url("MT_E2E_APP_URL")?;
    let (mut ports, _) = testkit::fake_ports();
    ports.clock = clock();
    let emulator: std::net::SocketAddr = std::env::var("FIRESTORE_EMULATOR_HOST")?.parse()?;
    if !emulator.ip().is_loopback() {
        return Err("emulator must be loopback".into());
    }
    let http = Arc::new(GcpHttp::with_emulator(
        Arc::new(StaticTokenSource(Sensitive::new("owner".into()))),
        emulator,
    )?);
    ports.store = Arc::new(FirestoreStore::new(
        http,
        FirestoreConfig::new(std::env::var("GOOGLE_CLOUD_PROJECT")?),
    ));
    let egress: Arc<dyn HttpEgress> = Arc::new(LoopbackEgress {
        origin: fake.clone(),
    });
    ports.egress = egress.clone();
    ports.identity = Arc::new(GoogleIdentity::new(
        GoogleIdentityConfig {
            client_id: "e2e-client".into(),
            client_secret: Sensitive::new("synthetic-client-secret".into()),
            auth_endpoint: fake.join("o/oauth2/v2/auth")?,
            token_endpoint: fake.join("token")?,
            revoke_endpoint: fake.join("revoke")?,
            jwks_uri: fake.join("oauth2/v3/certs")?,
        },
        egress.clone(),
        ports.clock.clone(),
    ));
    let gmail = Arc::new(GmailProvider::new(GmailHttp::new(
        egress.clone(),
        fake.join("gmail/v1/users/me")?,
        ports.clock.clone(),
    )));
    ports.gmail = gmail.clone();
    ports.invite_mailer = gmail;
    ports.app_folder = Arc::new(DriveAppFolder::new(GmailHttp::new(
        egress,
        fake,
        ports.clock.clone(),
    )));
    let config = ApiConfig::new(
        app.origin().ascii_serialization(),
        "e2e-client".into(),
        Sensitive::new(b"synthetic-rate-key".to_vec()),
        Sensitive::new(b"synthetic-email-key".to_vec()),
    )?;
    Ok(crate::app_state(Arc::new(ports), Arc::new(config)))
}

/// Serve the test stack, never a production adapter or non-loopback listener.
pub async fn serve() -> SetupResult<()> {
    let state = stack_state()?;
    obs::init(
        "api",
        Arc::new(obs::StdoutSink),
        Arc::new(adapters_gcp::SystemClock),
    )?;
    obs::register_http_routes(crate::ROUTE_TEMPLATES);
    let addr: std::net::SocketAddr = std::env::var("API_ADDR")?.parse()?;
    if !addr.ip().is_loopback() {
        return Err("test API must bind loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, crate::build_router(state)).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invite {
    email: String,
}
async fn invite(
    State(state): State<AppState>,
    Json(body): Json<Invite>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let email = EmailAddress::parse(&body.email).map_err(|_| ApiError::Internal)?;
    let (_, token, _) = svc_common::invites::upsert_pending_invite(
        state.ports.store.as_ref(),
        state.ports.system_keys.as_ref(),
        &state.config.email_lookup_key,
        state.ports.clock.as_ref(),
        state.ports.rng.as_ref(),
        &email,
    )
    .await
    .map_err(|_| ApiError::Internal)?;
    Ok(Json(serde_json::json!({"token":token.expose()})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Advance {
    seconds: u32,
}
async fn advance(Json(body): Json<Advance>) -> StatusCode {
    clock().advance(time::Duration::seconds(i64::from(body.seconds)));
    StatusCode::NO_CONTENT
}
/// Compose controls outside browser session/CSRF middleware.
pub fn control_router(state: AppState) -> Router {
    Router::new()
        .route("/internal/test/invites", post(invite))
        .route("/internal/test/advance-clock", post(advance))
        .with_state(state)
}
