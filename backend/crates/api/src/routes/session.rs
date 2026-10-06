//! API-AUTH-3 `GET /session` and API-AUTH-4 `POST /auth/sign-out` (T-506).
//!
//! `GET /session` never answers `401`: it reports the state so the app can
//! route, creating an anonymous `pre_auth` session when there is none, so the
//! app always has a CSRF token (S7 5.2). `POST /auth/sign-out` ends the session
//! on the server and tells the browser to clear its stored data (AU-07 AC2).

use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::{MailboxStatus, Provider};
use ports::store::aad_fields;
use ports::{Aad, SessionRecord, SessionState};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::step_up::step_up_valid_until;
use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::http::request_id::RequestId;
use crate::routes::invites::rfc3339;
use crate::session::cookie::clear_cookie;
use crate::session::extract::AnySession;
use crate::session::pre_auth::open_pre_auth;
use crate::session::store::{EndReason, NewCookie, SessionService, ABSOLUTE_TIMEOUT, IDLE_TIMEOUT};
use crate::state::AppState;

/// The exact `Clear-Site-Data` value sign-out sends (S7 5.2; ASVS V14.3.1).
/// Not `"cookies"`: that would also clear `__session` for other tabs mid-request
/// and is not in the contract.
const CLEAR_SITE_DATA: &str = "\"cache\", \"storage\"";

/// The wire form of `GET /session` (schema `Session`).
#[derive(Serialize)]
pub struct SessionDto {
    /// `"anonymous" | "pending_invite_request" | "authenticated"`.
    pub state: &'static str,
    /// The session's synchroniser token; the app echoes it on every write.
    pub csrf_token: String,
    /// Present only when authenticated.
    pub user: Option<SessionUserDto>,
    /// Present only in `pending_invite_request`.
    pub pending_invite_email: Option<String>,
    /// `recent_auth_at` plus five minutes while still in the future, else null.
    pub step_up_valid_until: Option<String>,
    /// Empty unless authenticated.
    pub mailboxes: Vec<SessionMailboxDto>,
    /// `min(last_seen_at + 15 min, created_at + 12 h)`, null unless authenticated.
    pub idle_expires_at: Option<String>,
    /// `created_at + 12 h`, null unless authenticated.
    pub absolute_expires_at: Option<String>,
}

/// The signed-in principal (schema `Session.user`).
#[derive(Serialize)]
pub struct SessionUserDto {
    /// The stable user ID.
    pub user_id: Uuid,
    /// Read from the user record, never from the cookie.
    pub is_admin: bool,
}

/// One linked mailbox (schema `Mailbox`).
#[derive(Serialize)]
pub struct SessionMailboxDto {
    /// The mailbox ID.
    pub mailbox_id: Uuid,
    /// `"gmail"` in v1 (never matched with `_`).
    pub provider: &'static str,
    /// The linked address, opened with the user's `data_key`.
    pub email_address: String,
    /// `"connected"` or `"needs_sign_in"`.
    pub status: &'static str,
    /// Whether this is the user's primary mailbox.
    pub is_primary: bool,
    /// When the mailbox was linked.
    pub linked_at: String,
}

/// `GET /api/v1/session` (API-AUTH-3). Never `401`; creates an anonymous
/// `pre_auth` session when there is none.
///
/// # Errors
///
/// `ApiError::Internal` on a store or key failure.
#[allow(clippy::too_many_lines)]
pub async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(_rid): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let service = SessionService::new(&state);
    let (new_cookie, record): (Option<NewCookie>, SessionRecord) =
        if let Some(loaded) = service.load(&headers).await? {
            (None, loaded.record.record)
        } else {
            let (cookie, versioned) = service.create_anonymous().await?;
            (Some(cookie), versioned.record)
        };

    let now = state.ports.clock.now();
    let state_name = match record.state {
        SessionState::PreAuth => "anonymous",
        SessionState::PendingInviteRequest => "pending_invite_request",
        SessionState::Authenticated => "authenticated",
    };

    let mut dto = SessionDto {
        state: state_name,
        csrf_token: record.csrf_token.clone(),
        user: None,
        pending_invite_email: None,
        step_up_valid_until: None,
        mailboxes: Vec::new(),
        idle_expires_at: None,
        absolute_expires_at: None,
    };

    match record.state {
        SessionState::PreAuth => {}
        SessionState::PendingInviteRequest => {
            if let Some(pre) = record.pre_auth.as_ref() {
                let plain = open_pre_auth(
                    state.ports.system_keys.as_ref(),
                    &record.session_record_id,
                    pre,
                )
                .await?;
                dto.pending_invite_email = plain.pending_email.map(|e| e.expose().clone());
            }
        }
        SessionState::Authenticated => {
            let user_id = record.user_id.ok_or(ApiError::Unauthenticated)?;
            let user_record = state
                .ports
                .store
                .users()
                .get(&user_id)
                .await?
                .ok_or(ApiError::Unauthenticated)?;
            dto.user = Some(SessionUserDto {
                user_id: user_id.0,
                is_admin: user_record.record.is_admin,
            });
            dto.step_up_valid_until = step_up_valid_until(record.recent_auth_at, now)
                .map(rfc3339)
                .transpose()?;
            dto.mailboxes =
                open_mailboxes(&state, &record, &user_record.record.wrapped_data_key).await?;
            dto.idle_expires_at = Some(rfc3339(idle_expires_at(&record))?);
            dto.absolute_expires_at = Some(rfc3339(record.created_at + ABSOLUTE_TIMEOUT)?);
        }
    }

    let mut resp = json_ok(StatusCode::OK, &dto);
    if let Some(NewCookie(value)) = new_cookie {
        resp.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(resp)
}

/// `POST /api/v1/auth/sign-out` (API-AUTH-4). Ends the session on the server and
/// clears the cookie and the browser's stored data (AU-07 AC2, AC6; V7.4.1,
/// V14.3.1).
///
/// # Errors
///
/// `ApiError::Unauthenticated` from the extractor when there is no session;
/// `ApiError::CsrfFailed` when the CSRF check fails; `ApiError::Internal` on a
/// store failure.
pub async fn sign_out(
    State(state): State<AppState>,
    AnySession(session): AnySession,
    Extension(rid): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let record = &session.record.record;
    SessionService::new(&state)
        .end(
            &record.session_hash,
            record.user_id.as_ref(),
            EndReason::SignedOut,
            rid,
        )
        .await?;
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, clear_cookie());
    resp.headers_mut().insert(
        HeaderName::from_static("clear-site-data"),
        HeaderValue::from_static(CLEAR_SITE_DATA),
    );
    Ok(resp)
}

/// Open every linked mailbox for the session response, in linked order.
async fn open_mailboxes(
    state: &AppState,
    record: &SessionRecord,
    wrapped: &ports::WrappedKey,
) -> Result<Vec<SessionMailboxDto>, ApiError> {
    let user = record.user_id.ok_or(ApiError::Unauthenticated)?;
    let mailboxes = state.ports.store.mailboxes().by_user(&user).await?;
    let mut out = Vec::with_capacity(mailboxes.len());
    for mailbox in &mailboxes {
        let aad = Aad {
            user,
            scope: mailbox.record.mailbox_id.0.to_string(),
            field: aad_fields::MAILBOX_EMAIL,
        };
        let plain = state
            .ports
            .keys
            .open(&user, wrapped, &aad, &mailbox.record.email_address.0)
            .await
            .map_err(|_| ApiError::Internal)?;
        let email_address = String::from_utf8(plain).map_err(|_| ApiError::Internal)?;
        out.push(SessionMailboxDto {
            mailbox_id: mailbox.record.mailbox_id.0,
            provider: match mailbox.record.provider {
                Provider::Gmail => "gmail",
            },
            email_address,
            status: match mailbox.record.status {
                MailboxStatus::Connected => "connected",
                MailboxStatus::NeedsSignIn => "needs_sign_in",
                MailboxStatus::ConsentBlocked => "consent_blocked",
            },
            is_primary: mailbox.record.is_primary,
            linked_at: rfc3339(mailbox.record.linked_at)?,
        });
    }
    Ok(out)
}

/// `min(last_seen_at + 15 min, created_at + 12 h)` (S7 API-AUTH-3).
fn idle_expires_at(record: &SessionRecord) -> OffsetDateTime {
    let idle = record.last_seen_at + IDLE_TIMEOUT;
    let absolute = record.created_at + ABSOLUTE_TIMEOUT;
    if idle < absolute {
        idle
    } else {
        absolute
    }
}
