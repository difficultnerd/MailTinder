//! API-MBX-1 `GET /mailboxes` (T-601a; ST-03 AC1).
//!
//! Lists every linked mailbox with its provider, address and status. The
//! address is decrypted for the response only; the refresh token and the
//! provider subject ID are never returned.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use domain::{MailboxId, MailboxStatus, Provider};
use ports::store::aad_fields;
use ports::Aad;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::session::extract::{AuthedSession, SteppedUpUser};
use crate::state::AppState;

/// One linked mailbox (schema `Mailbox`).
#[derive(Serialize)]
pub struct MailboxDto {
    /// The mailbox ID.
    pub mailbox_id: Uuid,
    /// `"gmail"` in v1 (matched exhaustively, never with `_`).
    pub provider: &'static str,
    /// The linked address, decrypted for the response only.
    pub email_address: String,
    /// `"connected"`, `"needs_sign_in"` or `"consent_blocked"`.
    pub status: &'static str,
    /// True for the mailbox whose Drive holds the app folder file.
    pub is_primary: bool,
    /// When the mailbox was linked (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub linked_at: OffsetDateTime,
}

/// The `GET /mailboxes` response body.
#[derive(Serialize)]
pub struct MailboxList {
    /// Every mailbox linked to the caller, `linked_at` ascending.
    pub mailboxes: Vec<MailboxDto>,
}

/// `GET /api/v1/mailboxes` (API-MBX-1).
///
/// # Errors
///
/// `ApiError::Unauthenticated` when the session's user is gone;
/// `ApiError::Internal` on a store or key failure.
pub async fn list_mailboxes(
    State(app): State<AppState>,
    session: AuthedSession,
) -> Result<Json<MailboxList>, ApiError> {
    let Some(user_record) = app.ports.store.users().get(&session.user).await? else {
        return Err(ApiError::Unauthenticated);
    };
    let mailboxes = app.ports.store.mailboxes().by_user(&session.user).await?;
    let mut out = Vec::with_capacity(mailboxes.len());
    for mailbox in mailboxes {
        let aad = Aad {
            user: session.user,
            scope: mailbox.record.mailbox_id.0.to_string(),
            field: aad_fields::MAILBOX_EMAIL,
        };
        let plain = app
            .ports
            .keys
            .open(
                &session.user,
                &user_record.record.wrapped_data_key,
                &aad,
                &mailbox.record.email_address.0,
            )
            .await
            .map_err(|_| ApiError::Internal)?;
        let email_address = String::from_utf8(plain).map_err(|_| ApiError::Internal)?;
        out.push(MailboxDto {
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
            linked_at: mailbox.record.linked_at,
        });
    }
    Ok(Json(MailboxList { mailboxes: out }))
}

/// `DELETE /api/v1/mailboxes/{mailbox_id}` (API-MBX-2, T-601b; AU-05).
///
/// The service does the work in the S7 5.3 order; this handler only parses the
/// path parameter and turns success into `204`.
///
/// # Errors
///
/// `ApiError::StepUpRequired` without a fresh sign-in; `NotFound` for a
/// missing or another user's mailbox; `LastMailbox` for the only one;
/// `AppFolderMoveFailed` when the primary's app folder cannot move.
pub async fn disconnect_mailbox(
    State(app): State<AppState>,
    SteppedUpUser(session): SteppedUpUser,
    Path(mailbox_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // A malformed ID is a missing mailbox: no parse detail leaks (S7 4).
    let Ok(id) = Uuid::parse_str(&mailbox_id) else {
        return Err(ApiError::NotFound);
    };
    crate::services::mailbox_disconnect::disconnect(&app, &session, &MailboxId::new(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
