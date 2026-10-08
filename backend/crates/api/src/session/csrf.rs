//! The CSRF layer: synchroniser token plus `Origin` check on every unsafe
//! method under `/api/v1` (S7 3.3; V3.5.1, V16.3.3).
//!
//! There is no exemption list. `GET` and `HEAD` are never checked and must
//! never change state; the OAuth callback is a `GET` protected by `state`.

use axum::extract::{Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use obs::SecurityEvent;
use ports::SessionState;
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::http::request_id::RequestId;
use crate::session::store::LoadedSession;
use crate::state::AppState;

/// The header carrying the session's synchroniser token.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Compare two token byte strings in constant time; different lengths fail.
#[must_use]
pub fn tokens_equal(a: &[u8], b: &[u8]) -> bool {
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Reject an unsafe request whose `Origin` is absent or not the app origin, or
/// whose `X-CSRF-Token` does not match the session's token.
pub async fn csrf_layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Only API writes are CSRF protected. Unmatched internal paths remain 404
    // in release builds; test controls are composed outside these layers.
    if !req.uri().path().starts_with("/api/v1/") {
        return next.run(req).await;
    }
    if !matches!(
        req.method(),
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    ) {
        return next.run(req).await;
    }
    let request_id = req.extensions().get::<RequestId>().copied();
    let origin_ok = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin == state.config.app_origin);
    let token = req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok());
    let loaded = req.extensions().get::<LoadedSession>();
    let token_ok = match (token, loaded) {
        (Some(token), Some(session)) => tokens_equal(
            token.as_bytes(),
            session.record.record.csrf_token.as_bytes(),
        ),
        _ => false,
    };
    if origin_ok && token_ok {
        return next.run(req).await;
    }
    let user = loaded.and_then(|session| {
        (session.record.record.state == SessionState::Authenticated)
            .then_some(session.record.record.user_id)
            .flatten()
    });
    obs::security_event(&SecurityEvent {
        action: "csrf_failure",
        outcome: "refused",
        user: user.map(|u| obs::Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&u.0)),
        request_id: request_id.map(|r| r.0),
        amr: None,
        provider: None,
        method: None,
    });
    ApiError::CsrfFailed.into_response()
}
