//! API-CAT-1 to API-CAT-5: the Filed tab (T-607a; S2 FL-02 AC1, FL-05 AC1,
//! S3 INV-5).
//!
//! Five handlers: list every category with its per-mailbox message counts,
//! create, rename and delete a category, and page the messages filed in one
//! category across the user's mailboxes. `category_id` is always a path
//! segment; `cursor` and `limit` are the only query parameters, so no label
//! name, address or message content ever arrives in a query string (S7 5.6,
//! S5, S6 5).
//!
//! The handlers only shape the request and the response; every provider call
//! and state write lives in [`crate::services::categories`].

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::routes::feed::MailboxErrorDto;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// S7 section 2: the default page size for API-CAT-5.
pub const CATEGORY_LIMIT_DEFAULT: u32 = 20;
/// S7 section 2: the largest page a client may ask for.
pub const CATEGORY_LIMIT_MAX: u32 = 50;
/// `[DEFAULT]` how long a page cursor lives (S7 2.1).
pub const CATEGORY_CURSOR_TTL: Duration = Duration::hours(12);

/// One category with its message counts (schema `Category`).
#[derive(Serialize)]
pub struct CategoryDto {
    /// The category ID.
    pub category_id: Uuid,
    /// The category name, 1 to 100 characters.
    pub name: String,
    /// The messages filed in this category across every mailbox.
    pub message_count: u64,
    /// One entry per mailbox that carries the label and answered.
    pub per_mailbox: Vec<PerMailboxCount>,
}

/// The messages a category holds in one mailbox.
#[derive(Serialize)]
pub struct PerMailboxCount {
    /// The mailbox the count came from.
    pub mailbox_id: Uuid,
    /// The messages carrying the category's label in that mailbox.
    pub message_count: u64,
}

/// The `GET /categories` response body.
#[derive(Serialize)]
pub struct CategoryListDto {
    /// The user's categories, sorted by name.
    pub categories: Vec<CategoryDto>,
}

/// The request body of API-CAT-2 and API-CAT-3 (schema `CategoryName`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryNameDto {
    /// The category name.
    pub name: String,
}

/// One filed message (schema `FiledMessage`); every string is plain text.
#[derive(Serialize)]
pub struct FiledMessageDto {
    /// The mailbox the message is filed in.
    pub mailbox_id: Uuid,
    /// A provider message ID, opaque to the client.
    pub message_id: String,
    /// The sender's display name, plain text.
    pub sender_name: String,
    /// The subject, plain text.
    pub subject: String,
    /// When the message arrived (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    /// An `https` link that opens the message in the provider's web client.
    pub provider_web_url: String,
}

/// The `GET /categories/{category_id}/messages` response body.
#[derive(Serialize)]
pub struct FiledPage {
    /// The page of filed messages, newest first.
    pub messages: Vec<FiledMessageDto>,
    /// A sealed cursor for the next page, or `None` when every mailbox is
    /// exhausted.
    pub next_cursor: Option<String>,
    /// One entry per mailbox that failed this page; never an error.
    pub mailbox_errors: Vec<MailboxErrorDto>,
}

/// The sealed cursor payload: the category the page belongs to, the provider
/// page token of every mailbox, and the mailboxes whose pages are used up.
#[derive(Serialize, Deserialize)]
pub struct CategoryCursor {
    /// The category the cursor was issued for.
    pub category_id: Uuid,
    /// `MailboxId` -> the provider page token to resume that mailbox from.
    pub pages: BTreeMap<Uuid, Option<String>>,
    /// Mailboxes whose last page was returned (no token follows).
    pub exhausted: BTreeSet<Uuid>,
}

/// The `GET /categories/{category_id}/messages` query string.
#[derive(Debug)]
pub struct MessagesParams {
    /// A sealed cursor from an earlier page, or `None` to start.
    pub cursor: Option<String>,
    /// Messages wanted, 1 to [`CATEGORY_LIMIT_MAX`].
    pub limit: u32,
}

impl MessagesParams {
    /// Parse `cursor` and `limit` (1 to 50, default 20); an unknown parameter
    /// or a bad value is `400 invalid_request` (S7 4).
    ///
    /// # Errors
    ///
    /// `ApiError::InvalidRequest` for an unknown parameter or a `limit`
    /// outside 1 to 50.
    pub fn parse(query: Option<&str>) -> Result<Self, ApiError> {
        let mut out = Self {
            cursor: None,
            limit: CATEGORY_LIMIT_DEFAULT,
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "cursor" => out.cursor = Some(value.into_owned()),
                "limit" => {
                    out.limit = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=CATEGORY_LIMIT_MAX).contains(n))
                        .ok_or_else(|| invalid("/limit"))?;
                }
                _ => return Err(invalid("")),
            }
        }
        Ok(out)
    }
}

/// `GET /api/v1/categories` (API-CAT-1).
///
/// The default read rate limit (S7 section 6, "other reads") applies through
/// the router's `default_limit_layer`, so the handler does not count again.
///
/// # Errors
///
/// `MailboxNeedsSignIn` when the primary mailbox cannot reach the state file,
/// and `Internal` on a store or key failure. A mailbox whose count fails is
/// left out of `per_mailbox`, never an error.
pub async fn list(
    State(app): State<AppState>,
    session: AuthedSession,
) -> Result<Response, ApiError> {
    let categories = crate::services::categories::list(&app, &session).await?;
    Ok(json_ok(StatusCode::OK, &CategoryListDto { categories }))
}

/// `POST /api/v1/categories` (API-CAT-2).
///
/// # Errors
///
/// `400 invalid_request` for a name that is empty, over-long or slash-
/// delimited; `409 category_exists` for a case-insensitive clash; and the
/// state-file errors of API-CAT-1.
pub async fn create(
    State(app): State<AppState>,
    session: AuthedSession,
    ApiJson(body): ApiJson<CategoryNameDto>,
) -> Result<Response, ApiError> {
    let category = crate::services::categories::create(&app, &session, &body.name).await?;
    Ok(json_ok(StatusCode::CREATED, &category))
}

/// `PATCH /api/v1/categories/{category_id}` (API-CAT-3).
///
/// # Errors
///
/// `404 not_found` for a missing or malformed ID; `409 category_exists` for a
/// clash with another category or a provider label; `502 provider_error` when
/// a mailbox refuses the rename; and the state-file errors of API-CAT-1.
pub async fn rename(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(raw): Path<String>,
    ApiJson(body): ApiJson<CategoryNameDto>,
) -> Result<Response, ApiError> {
    let id = category_id(&raw)?;
    let category = crate::services::categories::rename(&app, &session, id, &body.name).await?;
    Ok(json_ok(StatusCode::OK, &category))
}

/// `DELETE /api/v1/categories/{category_id}` (API-CAT-4).
///
/// # Errors
///
/// `404 not_found` for a missing or malformed ID, `502 provider_error` when a
/// mailbox refuses to drop the label, and the state-file errors of API-CAT-1.
pub async fn delete(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(raw): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = category_id(&raw)?;
    crate::services::categories::delete(&app, &session, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/categories/{category_id}/messages` (API-CAT-5).
///
/// # Errors
///
/// `400 invalid_request` for a bad `limit`, cursor or unknown parameter;
/// `404 not_found` for a missing or malformed ID; and the state-file errors of
/// API-CAT-1. A failing mailbox is data in `mailbox_errors`.
pub async fn messages(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(raw): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let id = category_id(&raw)?;
    let params = MessagesParams::parse(query.as_deref())?;
    let page = crate::services::categories::messages(
        &app,
        &session,
        id,
        params.cursor.as_deref(),
        params.limit,
    )
    .await?;
    Ok(json_ok(StatusCode::OK, &page))
}

/// A malformed path UUID is a missing category: no parse detail leaks (S7 4).
fn category_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::NotFound)
}

/// A `400 invalid_request` naming the offending field.
fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: if field.is_empty() {
            Vec::new()
        } else {
            vec![field.to_owned()]
        },
    }
}
