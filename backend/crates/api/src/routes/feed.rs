//! API-FEED-1 `POST /feed/next` (T-602c; FD-01 to FD-04, SW-02 AC2, GM-08).
//!
//! The route is a `POST` because it advances the stored Feed position. The
//! handler enforces the per-user rate limit and hands the work to
//! [`crate::services::feed`].

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use domain::user_state::MailboxPosition;
use domain::Classification;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// S7 section 2: the default page size.
pub const FEED_LIMIT_DEFAULT: u32 = 20;
/// S7 section 2: the largest page a client may ask for.
pub const FEED_LIMIT_MAX: u32 = 50;
/// `[DEFAULT]` the session's absolute lifetime (S7 3.2).
pub const SEALED_TTL_HOURS: i64 = 12;

fn default_limit() -> u32 {
    FEED_LIMIT_DEFAULT
}

/// The request body (schema `FeedRequest`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedRequest {
    /// A sealed cursor from an earlier page, or `null` to resume.
    pub cursor: Option<String>,
    /// Cards wanted, 1 to [`FEED_LIMIT_MAX`].
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Check every mailbox for new mail first (FD-03 AC5).
    #[serde(default)]
    pub refresh: bool,
}

/// The response body (schema `FeedPage`).
#[derive(Serialize)]
pub struct FeedPage {
    pub cards: Vec<CardDto>,
    pub next_cursor: Option<String>,
    pub phase: Phase,
    pub phase_changed: bool,
    pub mailbox_errors: Vec<MailboxErrorDto>,
    pub rule_actions_applied: u32,
}

/// Which stretch of the inbox the page came from (FD-03).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    New,
    Backlog,
}

/// One card (schema `Card`). Every string is plain text.
#[derive(Serialize)]
pub struct CardDto {
    pub mailbox_id: Uuid,
    pub message_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    pub sender_name: String,
    pub sender_address: String,
    pub subject: String,
    pub preview: String,
    pub bulk_score: u8,
    pub bulk_reason: String,
    pub class: &'static str,
    /// `one_click`, `mailto`, `manual` or `none`, from header rules.
    pub unsubscribe_method: &'static str,
    pub has_one_click: bool,
    /// Filled by T-607b.
    pub suggestion: Option<SuggestionDto>,
    /// Filled by T-607b.
    pub keep_prompt: Option<CategoryRefDto>,
    pub skip_count: u8,
    pub boss: Option<BossDto>,
    pub provider_web_url: String,
    pub classification_token: String,
    /// Admin sessions only; absent, never `null`, for everyone else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifier_id: Option<String>,
}

/// Filing suggestion; T-607b fills it, so this task never builds one.
#[derive(Serialize)]
pub struct SuggestionDto {
    pub category_id: Option<Uuid>,
    pub name: String,
    pub confidence: &'static str,
}

/// Keep prompt; T-607b fills it, so this task never builds one.
#[derive(Serialize)]
pub struct CategoryRefDto {
    pub category_id: Uuid,
    pub name: String,
}

/// Inbox mail left from a boss sender (GM-08 AC2).
#[derive(Serialize)]
pub struct BossDto {
    pub remaining: u64,
}

/// A mailbox that failed this page (FD-02 AC3).
#[derive(Serialize)]
pub struct MailboxErrorDto {
    pub mailbox_id: Uuid,
    pub code: &'static str,
}

/// The sealed cursor payload.
#[derive(Serialize, Deserialize)]
pub struct CursorPayload {
    pub positions: BTreeMap<Uuid, MailboxPosition>,
    pub phase: Phase,
}

/// The sealed classification payload (S7 5.13).
#[derive(Serialize, Deserialize)]
pub struct ClassificationPayload {
    pub mailbox_id: Uuid,
    pub message_id: String,
    pub header_rules: Classification,
    /// `header_rules@1`.
    pub classifier_id: String,
}

/// `POST /api/v1/feed/next` (API-FEED-1).
///
/// # Errors
///
/// `400 invalid_request` for a bad `limit` or cursor, `429` over the rate
/// limit, and whatever the service returns for the user's state file.
pub async fn next_page_handler(
    State(app): State<AppState>,
    session: AuthedSession,
    request_id: RequestId,
    ApiJson(req): ApiJson<FeedRequest>,
) -> Result<Response, ApiError> {
    app.limits
        .check(
            &policies::FEED,
            LimitSubject::User(&session.user),
            request_id,
        )
        .await?;
    let page = crate::services::feed::next_page(&app, &session, req).await?;
    Ok(json_ok(StatusCode::OK, &page))
}
