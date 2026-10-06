//! Request extractors that hand a route the server-side session.
//!
//! Each reads the `LoadedSession` the session layer put in the request
//! extensions and enforces the route's authentication level. A user route never
//! reads the user ID from the body or path (V8.2.1, deny by default).

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Method;
use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use ports::{SessionHash, SessionRecordId, SessionState};

use crate::error::ApiError;
use crate::http::request_id::RequestId;
use crate::session::store::LoadedSession;
use crate::state::AppState;

/// Any session state; `401` when there is none.
pub struct AnySession(pub LoadedSession);

/// An authenticated session; `401` otherwise.
pub struct AuthedSession {
    /// The authenticated user.
    pub user: UserId,
    /// The stable record ID, sealed tokens bind to it.
    pub session_record_id: SessionRecordId,
    /// Read from `UserRecord` on every request, never from the cookie.
    pub is_admin: bool,
    /// The last fresh sign-in, for step-up.
    pub recent_auth_at: Option<time::OffsetDateTime>,
    /// The session's store key.
    pub session_hash: SessionHash,
}

/// An authenticated admin; `403` plus an `authz_failure` event otherwise.
pub struct AdminSession(pub AuthedSession);

/// A session awaiting an invite request's approval; `401` otherwise.
pub struct PendingInviteSession {
    /// The loaded session record.
    pub loaded: LoadedSession,
}

fn loaded(parts: &Parts) -> Result<LoadedSession, ApiError> {
    parts
        .extensions
        .get::<LoadedSession>()
        .cloned()
        .ok_or(ApiError::Unauthenticated)
}

/// A static method name for the security event (never request data).
fn method_name(method: &Method) -> Option<&'static str> {
    match *method {
        Method::GET => Some("GET"),
        Method::POST => Some("POST"),
        Method::PUT => Some("PUT"),
        Method::PATCH => Some("PATCH"),
        Method::DELETE => Some("DELETE"),
        _ => None,
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AnySession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(loaded(parts)?))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for PendingInviteSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = loaded(parts)?;
        if session.record.record.state != SessionState::PendingInviteRequest {
            return Err(ApiError::Unauthenticated);
        }
        Ok(Self { loaded: session })
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AuthedSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = loaded(parts)?;
        if session.record.record.state != SessionState::Authenticated {
            return Err(ApiError::Unauthenticated);
        }
        let Some(user) = session.record.record.user_id else {
            return Err(ApiError::Unauthenticated);
        };
        let Some(user_record) = state.ports.store.users().get(&user).await? else {
            return Err(ApiError::Unauthenticated);
        };
        Ok(Self {
            user,
            session_record_id: session.record.record.session_record_id,
            is_admin: user_record.record.is_admin,
            recent_auth_at: session.record.record.recent_auth_at,
            session_hash: session.record.record.session_hash,
        })
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = parts.extensions.get::<RequestId>().copied();
        let method = method_name(&parts.method);
        let inner = AuthedSession::from_request_parts(parts, state).await?;
        if inner.is_admin {
            return Ok(Self(inner));
        }
        obs::security_event(&SecurityEvent {
            action: "authz_failure",
            outcome: "refused",
            user: Some(Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&inner.user.0)),
            request_id: request_id.map(|r| r.0),
            amr: None,
            provider: None,
            method,
        });
        Err(ApiError::Forbidden)
    }
}
