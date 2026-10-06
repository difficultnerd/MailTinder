//! API-SW-1 `POST /swipes` and API-SW-2 `POST /swipes/undo` (T-604; SW-01,
//! SW-02, SW-04, SW-05).
//!
//! Thin handlers: each enforces the swipe rate limit and hands the work to
//! [`crate::services::swipe`] and [`crate::services::undo`]. Nothing here reads
//! or logs message content.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use domain::SwipeOutcome;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::routes::feed::CategoryRefDto;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// The `Idempotency-Key` header (S7 5.5): one UUID v4 per swipe, reused on
/// retry.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// The request body (schema `SwipeRequest`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwipeRequest {
    pub mailbox_id: Uuid,
    /// A provider message ID, opaque to the client.
    pub message_id: String,
    pub action: ActionDto,
    pub category_id: Option<Uuid>,
    pub new_category_name: Option<String>,
    pub classification_token: String,
}

/// The action the card was swiped.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionDto {
    Keep,
    Skip,
    Reject,
    File,
}

/// The response body (schema `SwipeResult`). Stored verbatim for idempotent
/// retries, so it round-trips.
#[derive(Serialize, Deserialize, Clone)]
pub struct SwipeResultDto {
    pub outcome: SwipeOutcome,
    #[serde(with = "time::serde::rfc3339::option")]
    pub unsubscribe_due_at: Option<OffsetDateTime>,
    pub filed_category: Option<CategoryRefDto>,
    pub undo_token: String,
    /// Empty here; T-605 fills `block_person`.
    pub prompts: Vec<PromptDto>,
    /// Empty here; T-802 fills.
    pub achievements_unlocked: Vec<AchievementDto>,
    /// False here; T-605 sets it.
    pub boss_defeated: bool,
}

/// A prompt the app should show after a swipe (schema `SwipeResult.prompts`).
#[derive(Serialize, Deserialize, Clone)]
pub struct PromptDto {
    #[serde(rename = "type")]
    pub kind: PromptKindDto,
    pub prompt_ref: String,
    pub sender_name: String,
}

/// The only prompt type in v1.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptKindDto {
    BlockPerson,
}

/// An achievement this swipe unlocked (schema `Achievement`).
#[derive(Serialize, Deserialize, Clone)]
pub struct AchievementDto {
    pub achievement_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub unlocked_at: OffsetDateTime,
}

/// The request body of API-SW-2.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoRequest {
    pub undo_token: String,
}

/// `POST /api/v1/swipes` (API-SW-1).
///
/// # Errors
///
/// `InvalidRequest` for a missing or non-UUID `Idempotency-Key` (or a bad
/// body), `429` over the rate limit, `NotFound` for another user's mailbox,
/// `MessageChanged` when the message moved, and whatever the service returns.
pub async fn create(
    State(app): State<AppState>,
    session: AuthedSession,
    request_id: RequestId,
    headers: HeaderMap,
    ApiJson(req): ApiJson<SwipeRequest>,
) -> Result<Response, ApiError> {
    app.limits
        .check(
            &policies::SWIPES,
            LimitSubject::User(&session.user),
            request_id,
        )
        .await?;
    let key = idempotency_key(&headers)?;
    let result = crate::services::swipe::swipe(&app, &session, key, req).await?;
    Ok(json_ok(StatusCode::OK, &result))
}

/// `POST /api/v1/swipes/undo` (API-SW-2).
///
/// # Errors
///
/// `410 undo_expired` for a token that fails to open, `429` over the rate
/// limit, `502 provider_error` when the provider refuses the restore, and
/// whatever the service returns.
pub async fn undo(
    State(app): State<AppState>,
    session: AuthedSession,
    request_id: RequestId,
    ApiJson(req): ApiJson<UndoRequest>,
) -> Result<Response, ApiError> {
    app.limits
        .check(
            &policies::SWIPES,
            LimitSubject::User(&session.user),
            request_id,
        )
        .await?;
    let result = crate::services::undo::undo(&app, &session, &req.undo_token).await?;
    Ok(json_ok(StatusCode::OK, &result))
}

/// The `Idempotency-Key` header, as a UUID.
fn idempotency_key(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| ApiError::InvalidRequest {
            fields: vec!["Idempotency-Key".to_owned()],
        })
}
