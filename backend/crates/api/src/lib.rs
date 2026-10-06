//! The `MailTinder` API service.

pub mod config;
pub mod error;
pub mod http;
pub mod limits;
pub mod sealed;
pub mod session;
pub mod state;
pub mod tokens;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use ports::Ports;
use tower_http::catch_panic::CatchPanicLayer;

pub const ROUTE_TEMPLATES: &[&str] = &["/api/v1/healthz", "/api/v1"];

use crate::config::ApiConfig;
use crate::error::{render_problem, ApiError};
use crate::http::headers::apply_security_headers;
use crate::http::request_id::RequestId;
use crate::limits::{KeyKind, LimitSubject, RateLimiter};
use crate::session::csrf::csrf_layer;
use crate::session::store::{LoadedSession, SessionService};
use crate::state::AppState;
use ports::SessionState;

/// Build the API router. Every route is nested under `/api/v1`.
pub fn build_router(state: AppState) -> Router {
    build_router_with_routes(state, |router| router)
}

/// Compose handlers behind the same security and error middleware.
/// The route factory is used by integration tests and subsequent feature modules.
pub fn build_router_with_routes(
    state: AppState,
    routes: impl FnOnce(Router<AppState>) -> Router<AppState>,
) -> Router {
    routes(Router::new())
        .route("/api/v1/healthz", get(healthz))
        .fallback(fallback)
        .layer(CatchPanicLayer::custom(|_| {
            ApiError::Internal.into_response()
        }))
        .layer(middleware::from_fn(request_log_layer))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            default_limit_layer,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), csrf_layer))
        .layer(middleware::from_fn_with_state(state.clone(), session_layer))
        .layer(middleware::from_fn(problem_layer))
        .layer(middleware::from_fn(security_headers_layer))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_id_layer,
        ))
        .with_state(state)
}

/// Load the session cookie into the request extensions (S7 3.2).
async fn session_layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    match SessionService::new(&state).load(req.headers()).await {
        Ok(Some(loaded)) => {
            req.extensions_mut().insert(loaded);
        }
        Ok(None) => {}
        Err(e) => return e.into_response(),
    }
    next.run(req).await
}

/// Apply the default read/write rate limit to every `/api/v1` request
/// (S7 4; reads keyed by session, writes by user when authenticated).
async fn default_limit_layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(policy) = crate::limits::default_policy(req.method()) else {
        return next.run(req).await;
    };
    let Some(request_id) = req.extensions().get::<RequestId>().copied() else {
        return next.run(req).await;
    };
    let loaded = req.extensions().get::<LoadedSession>();
    let user_id = loaded.and_then(|s| s.record.record.user_id);
    let session_id = loaded.map(|s| s.record.record.session_record_id);
    let authenticated =
        loaded.is_some_and(|s| s.record.record.state == SessionState::Authenticated);
    let ip = crate::http::client_ip::client_ip(req.headers(), state.config.xff_trusted_hops);
    let subject = if policy.key == KeyKind::User && authenticated {
        match &user_id {
            Some(u) => LimitSubject::User(u),
            None => LimitSubject::Ip(&ip),
        }
    } else if let Some(sid) = &session_id {
        LimitSubject::Session(sid)
    } else {
        LimitSubject::Ip(&ip)
    };
    match state.limits.check(policy, subject, request_id).await {
        Ok(info) => {
            let mut resp = next.run(req).await;
            crate::limits::apply_limit_headers(&mut resp, &info);
            resp
        }
        Err(e) => e.into_response(),
    }
}

/// A plain 204 for Cloud Run probes.
async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Any unmatched path or method returns a generic 404 problem.
async fn fallback() -> ApiError {
    ApiError::NotFound
}

/// First layer: set a request ID and echo it on every response.
async fn request_id_layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let id = RequestId(state.ports.rng.uuid_v4());
    req.extensions_mut().insert(id);
    let mut resp = next.run(req).await;
    resp.extensions_mut().insert(id);
    if let Ok(v) = id.0.to_string().parse() {
        resp.headers_mut().insert("x-request-id", v);
    }
    resp
}

/// Security and anti-caching headers on every response.
async fn security_headers_layer(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    apply_security_headers(&mut resp);
    resp
}

/// Render an `ApiError` extension into a problem body, with the request id.
async fn problem_layer(req: Request, next: Next) -> Response {
    let request_id = req.extensions().get::<RequestId>().copied();
    let mut resp = next.run(req).await;
    if let Some(request_id) = request_id {
        render_problem(&mut resp, request_id);
    }
    resp
}

/// Log each request (operation + status only; never request data).
async fn request_log_layer(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let id = req.extensions().get::<RequestId>().copied();
    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    if let Some(id) = id {
        obs::request_log(&obs::RequestLog {
            request_id: id.0,
            user: None,
            route: route_template(&method, &path),
            status: resp.status().as_u16(),
            latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            rate_limit_hit: resp.status() == StatusCode::TOO_MANY_REQUESTS,
        });
    }
    resp
}

/// Map a concrete path to a route template for logging (best effort).
fn route_template(_method: &axum::http::Method, path: &str) -> &'static str {
    if path == "/api/v1/healthz" {
        "/api/v1/healthz"
    } else {
        "/api/v1"
    }
}

/// Build an `AppState` from real ports and config (used by `main`).
#[must_use]
pub fn app_state(ports: Arc<Ports>, config: Arc<ApiConfig>) -> AppState {
    let limits = Arc::new(RateLimiter::new(
        ports.store.clone(),
        ports.clock.clone(),
        config.rate_key.clone(),
    ));
    AppState {
        ports,
        config,
        limits,
        tokens: Arc::new(crate::tokens::TokenService::new()),
    }
}
