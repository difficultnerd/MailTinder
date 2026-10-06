//! API-PROG-1 `GET /progress`: the inbox meter and the backlog level
//! (T-603; GM-01, GM-04).
//!
//! This is a read: it never changes the user's state file or the store. The
//! handler only shapes the response; [`crate::services::progress`] counts.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::routes::feed::MailboxErrorDto;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// The `GET /progress` response body (schema: S7 5.3).
#[derive(Serialize)]
pub struct ProgressDto {
    /// The total inbox count across the mailboxes that answered (GM-01 AC1).
    pub inbox_count: u64,
    /// One entry per mailbox that did not answer (GM-01 AC2); `mailbox_id`
    /// only, never an address.
    pub mailbox_errors: Vec<MailboxErrorDto>,
    /// `null` until new mail is cleared (GM-04 AC1).
    pub level: Option<LevelDto>,
}

/// The current backlog level (schema `Level`).
#[derive(Serialize)]
pub struct LevelDto {
    /// The UTC calendar year being worked through.
    pub year: i32,
    /// Mail left in the inbox for that year.
    pub remaining: u64,
}

/// `[DEFAULT]` stop searching for a non-empty year after this many empty ones.
pub const LEVEL_LOOKBACK_YEARS: i32 = 10;

/// `GET /api/v1/progress` (API-PROG-1).
///
/// The default read rate limit (S7 section 6, "other reads") applies through
/// the router's `default_limit_layer`, so the handler does not count again.
///
/// # Errors
///
/// `MailboxNeedsSignIn` when the primary mailbox cannot reach the state file,
/// and `Internal` on a store or key failure. A mailbox that fails to answer is
/// data in `mailbox_errors`, never an error.
pub async fn get_progress(
    State(app): State<AppState>,
    session: AuthedSession,
) -> Result<Response, ApiError> {
    let meter = crate::services::progress::progress(&app, &session).await?;
    Ok(json_ok(StatusCode::OK, &meter))
}
