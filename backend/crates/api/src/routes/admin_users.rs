//! T-804 admin user administration: API-ADM-16 `GET /admin/users` and
//! API-ADM-15 `DELETE /admin/users/{user_id}/sessions` (S7 3.7, 5.11, 6).
//!
//! The list joins each user record with the address of the earliest mailbox
//! still linked and the liveness of that user's session. The address is opened
//! with the user's OWN key (`[DEFAULT]` data key), never the admin's: the
//! admin's key opens nothing of theirs. Ending a session needs a fresh Google
//! sign-in (`require_step_up`, AU-01 AC5) and is limited to 20 per admin per
//! day (S7 6).
//!
//! The responses hold addresses, so nothing here is ever logged: the security
//! events carry only the two pseudonymous IDs and the action/outcome, and the
//! `no-store` header comes from the shared security-headers layer.

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use domain::UserId;
use ports::store::aad_fields;
use ports::{Aad, PageRequest, SessionState, UserRecord};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::routes::invites::{open_cursor, rfc3339, seal_cursor, security_event, ListParams};
use crate::session::extract::{AdminSession, AuthedSession, SteppedUpAdmin};
use crate::session::store::{EndReason, SessionService, ABSOLUTE_TIMEOUT, IDLE_TIMEOUT};
use crate::state::AppState;

/// One user in the admin list (S7 5.11, API-ADM-16).
#[derive(Serialize)]
pub struct AdminUserDto {
    /// The user's ID (a UUID; the only identifier the routes accept).
    pub user_id: Uuid,
    /// The address of the earliest linked mailbox still present.
    pub email_address: String,
    /// When the user record was created (RFC 3339).
    pub created_at: String,
    /// Whether the record carries the admin flag (read from the record).
    pub is_admin: bool,
    /// How many mailboxes are linked right now.
    pub mailbox_count: u32,
    /// True when the user has an authenticated session that has not expired.
    pub signed_in: bool,
    /// The live session's `last_seen_at`, or `null` when signed out.
    pub last_seen_at: Option<String>,
}

/// One page of `GET /admin/users`.
#[derive(Serialize)]
pub struct AdminUserPage {
    /// The users on this page, `created_at` ascending.
    pub users: Vec<AdminUserDto>,
    /// The sealed store cursor, absent on the last page.
    pub next_cursor: Option<String>,
}

/// `GET /api/v1/admin/users` (API-ADM-16).
///
/// # Errors
///
/// `ApiError::Unauthenticated` when the admin's user record is gone;
/// `ApiError::InvalidRequest` for a bad query or a cursor that does not open;
/// `ApiError::Internal` on a store or key failure.
pub async fn list(
    State(app): State<AppState>,
    AdminSession(admin): AdminSession,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    // `status` is not part of this list; only `cursor` and `limit` are.
    let params = ListParams::parse(query.as_deref(), false)?;
    let page = list_users(&app, &admin, params.cursor.as_deref(), params.limit).await?;
    Ok(json_ok(StatusCode::OK, &page))
}

/// Build one page of users for an admin (API-ADM-16).
///
/// # Errors
///
/// As [`list`], minus the query parsing (the caller has already parsed).
pub async fn list_users(
    app: &AppState,
    admin: &AuthedSession,
    cursor: Option<&str>,
    limit: u32,
) -> Result<AdminUserPage, ApiError> {
    let after = open_cursor(app, admin, cursor).await?;
    let page = app
        .ports
        .store
        .users()
        .list(PageRequest { limit, after })
        .await?;
    let now = app.ports.clock.now();
    let mut users = Vec::with_capacity(page.items.len());
    for item in &page.items {
        if let Some(dto) = to_dto(app, &item.record, now).await? {
            users.push(dto);
        }
    }
    let next_cursor = seal_cursor(app, admin, page.next).await?;
    Ok(AdminUserPage { users, next_cursor })
}

/// `DELETE /api/v1/admin/users/{user_id}/sessions` (API-ADM-15).
///
/// # Errors
///
/// `ApiError::StepUpRequired` without a fresh sign-in; `ApiError::Forbidden`
/// for a non-admin (checked first, by the extractor); `ApiError::NotFound` for
/// a missing user or a malformed ID; `ApiError::RateLimited` past the daily
/// cap; `ApiError::Internal` on a store failure.
pub async fn end(
    State(app): State<AppState>,
    SteppedUpAdmin(admin): SteppedUpAdmin,
    Path(user_id): Path<String>,
    request_id: RequestId,
) -> Result<StatusCode, ApiError> {
    // A malformed ID is a missing user: no parse detail leaks (S7 4).
    let Ok(user) = Uuid::parse_str(&user_id) else {
        return Err(ApiError::NotFound);
    };
    app.limits
        .check(
            &policies::ADMIN_SESSION_KILL,
            LimitSubject::User(&admin.user),
            request_id,
        )
        .await?;
    end_user_session(&app, &admin, user, request_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// End every session of `user` on an admin's behalf (API-ADM-15, AU-07 AC5).
///
/// Queued unsubscribe jobs keep running: they need no session, so nothing
/// touches them (S7 5.11). An admin may end their own session; it ends.
///
/// # Errors
///
/// `ApiError::NotFound` when the user does not exist; `ApiError::Internal` on
/// a store failure.
pub async fn end_user_session(
    app: &AppState,
    admin: &AuthedSession,
    user: Uuid,
    request_id: RequestId,
) -> Result<(), ApiError> {
    let target = UserId::new(user);
    if app.ports.store.users().get(&target).await?.is_none() {
        return Err(ApiError::NotFound);
    }
    // Two correlated events, as S6 7 documents: a `session_end`/`admin_ended`
    // event per deleted session naming the target, and an `admin_action` event
    // naming the actor. Both carry the same request ID, the documented join key
    // that attributes the termination to the admin; a log line holds one
    // pseudonymous user ID, so neither event can name both (S5 logs). Neither
    // carries an address.
    SessionService::new(app)
        .end_all_for_user(&target, EndReason::AdminEnded, request_id)
        .await?;
    security_event(app, "admin_action", "success", Some(admin), request_id);
    Ok(())
}

/// Build the DTO for one user, or `None` when the user has no mailbox left
/// (a transient state during account deletion; the response requires an
/// address, so the record is skipped rather than guessed).
async fn to_dto(
    app: &AppState,
    record: &UserRecord,
    now: OffsetDateTime,
) -> Result<Option<AdminUserDto>, ApiError> {
    let mailboxes = app.ports.store.mailboxes().by_user(&record.user_id).await?;
    let Some(first) = mailboxes.first() else {
        return Ok(None);
    };
    // The address was sealed with the user's own key, so only their key opens
    // it (S7 5.11 edge case).
    let aad = Aad {
        user: record.user_id,
        scope: first.record.mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let plain = app
        .ports
        .keys
        .open(
            &record.user_id,
            &record.wrapped_data_key,
            &aad,
            &first.record.email_address.0,
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    let email_address = String::from_utf8(plain).map_err(|_| ApiError::Internal)?;

    let (signed_in, last_seen_at) = live_session(app, &record.user_id, now).await?;
    Ok(Some(AdminUserDto {
        user_id: record.user_id.0,
        email_address,
        created_at: rfc3339(record.created_at)?,
        is_admin: record.is_admin,
        mailbox_count: u32::try_from(mailboxes.len()).unwrap_or(u32::MAX),
        signed_in,
        last_seen_at: match last_seen_at {
            Some(at) => Some(rfc3339(at)?),
            None => None,
        },
    }))
}

/// Whether the user has an authenticated session that has not passed its idle
/// or absolute expiry, and the most recent `last_seen_at` of such a session.
async fn live_session(
    app: &AppState,
    user: &UserId,
    now: OffsetDateTime,
) -> Result<(bool, Option<OffsetDateTime>), ApiError> {
    let mut last_seen: Option<OffsetDateTime> = None;
    for session in app.ports.store.sessions().by_user(user).await? {
        if session.record.state != SessionState::Authenticated {
            continue;
        }
        if now >= session.record.created_at + ABSOLUTE_TIMEOUT {
            continue;
        }
        if now >= session.record.last_seen_at + IDLE_TIMEOUT {
            continue;
        }
        last_seen = Some(match last_seen {
            Some(previous) if previous > session.record.last_seen_at => previous,
            _ => session.record.last_seen_at,
        });
    }
    Ok((last_seen.is_some(), last_seen))
}
