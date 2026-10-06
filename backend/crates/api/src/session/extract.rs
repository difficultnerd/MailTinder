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

/// An authenticated session with a fresh Google sign-in inside the step-up
/// window (S7 3.6, T-504).
pub struct SteppedUpUser(pub AuthedSession);

/// An authenticated admin with a fresh Google sign-in. The admin check runs
/// first, so a non-admin learns nothing about step-up state (T-504).
pub struct SteppedUpAdmin(pub AuthedSession);

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

impl AuthedSession {
    /// Build from an already loaded session: `401` unless it is authenticated
    /// and its user still exists (S7 3.2).
    ///
    /// # Errors
    ///
    /// `ApiError::Unauthenticated` when the session is not authenticated or its
    /// user is gone; `ApiError::Internal` on a store failure.
    pub async fn load(state: &AppState, session: &LoadedSession) -> Result<Self, ApiError> {
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
impl FromRequestParts<AppState> for AuthedSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = loaded(parts)?;
        Self::load(state, &session).await
    }
}

#[async_trait]
impl FromRequestParts<AppState> for SteppedUpUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let inner = AuthedSession::from_request_parts(parts, state).await?;
        if let Err(refused) =
            crate::auth::step_up::require_step_up(&inner, state.ports.clock.as_ref())
        {
            // A refused step-up is an authorisation failure (V16.3.2): log it,
            // pseudonymously, before the 403 goes out.
            authz_failure(parts, state, &inner.user);
            return Err(refused);
        }
        Ok(Self(inner))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for SteppedUpAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The admin check runs first: a non-admin gets `403 forbidden`, never
        // `step_up_required` (T-504).
        let admin = AdminSession::from_request_parts(parts, state).await?;
        if let Err(refused) =
            crate::auth::step_up::require_step_up(&admin.0, state.ports.clock.as_ref())
        {
            authz_failure(parts, state, &admin.0.user);
            return Err(refused);
        }
        Ok(Self(admin.0))
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let inner = AuthedSession::from_request_parts(parts, state).await?;
        if inner.is_admin {
            return Ok(Self(inner));
        }
        authz_failure(parts, state, &inner.user);
        Err(ApiError::Forbidden)
    }
}

/// One `authz_failure` event for a request an extractor refused (V16.3.2):
/// the pseudonymous user, the request ID and the route verb only.
fn authz_failure(parts: &Parts, state: &AppState, user: &UserId) {
    crate::http::security::authz_failure(
        state,
        user,
        parts.extensions.get::<RequestId>().copied().map(|r| r.0),
        method_name(&parts.method),
    );
}
