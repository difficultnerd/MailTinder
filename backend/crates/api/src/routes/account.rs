//! API-ACCT-1 `DELETE /account` (T-803; S2 AU-06).
//!
//! The service does the work in the S7 5.10 order; this handler adds the rate
//! limit, turns success into `202` and tells the browser to forget everything.

use axum::extract::{Extension, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::services::account_deletion;
use crate::session::cookie::clear_cookie;
use crate::session::extract::SteppedUpUser;
use crate::state::AppState;

/// The exact `Clear-Site-Data` value sign-out sends (S7 5.2; ASVS V14.3.1).
const CLEAR_SITE_DATA: &str = "\"cache\", \"storage\"";

/// The `202` body (schema `DeletionAccepted`).
#[derive(Serialize)]
pub struct DeletionAccepted {
    /// When every leftover record is gone at the latest (now plus 24 hours).
    #[serde(with = "time::serde::rfc3339")]
    pub deletion_due_by: OffsetDateTime,
    /// Mailboxes whose app folder file could not be deleted; the user removes
    /// it by hand. The response is the only place that says so.
    pub app_folders_not_deleted: Vec<NotDeleted>,
}

/// One app folder file left behind.
#[derive(Serialize)]
pub struct NotDeleted {
    /// The mailbox ID.
    pub mailbox_id: Uuid,
    /// The address, decrypted before the key was destroyed.
    pub email_address: String,
}

/// `DELETE /api/v1/account` (API-ACCT-1).
///
/// # Errors
///
/// `ApiError::StepUpRequired` without a fresh sign-in; `RateLimited` after
/// three deletions in a day; `ProviderUnavailable` when the user record cannot
/// be deleted (retry is safe); `Internal` on a store failure before that.
pub async fn delete_account(
    State(app): State<AppState>,
    SteppedUpUser(session): SteppedUpUser,
    Extension(rid): Extension<RequestId>,
) -> Result<Response, ApiError> {
    app.limits
        .check(
            &policies::ACCOUNT_DELETE,
            LimitSubject::User(&session.user),
            rid,
        )
        .await?;
    let (accepted, _steps) = account_deletion::delete_account(&app, &session).await?;
    let mut resp = json_ok(StatusCode::ACCEPTED, &accepted);
    resp.headers_mut()
        .insert(header::SET_COOKIE, clear_cookie());
    resp.headers_mut().insert(
        HeaderName::from_static("clear-site-data"),
        HeaderValue::from_static(CLEAR_SITE_DATA),
    );
    Ok(resp)
}
