//! API-NA-1 to API-NA-3 (T-705; S2 NA-01 AC1, AC2, UN-05 AC1, ASVS V8.2.2,
//! V1.2.2).
//!
//! `GET /needs-attention` serves the open list newest first, paged with a
//! sealed cursor and carrying the open count for the tab badge. The `resolve`
//! and `dismiss` routes both delete one item (S3: every exit deletes the
//! record), so they behave the same on the server; the difference is only what
//! the user meant. Another user's item is a `404`, never a `403` (S7 1
//! principle 5), and the refusal is logged as an `authz_failure` event.
//!
//! Nothing sensitive (an item ID, sender display or link) goes in a query
//! string or a log line (S5, S6 5).

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use domain::NeedsAttentionReason;
use ports::store::aad_fields;
use ports::{Aad, Ciphertext, PageRequest, Precondition, StoreCursor, StoreError};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::sealed::{SealedTokens, TokenType};
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// S7 section 2: the default page size.
pub const NA_LIMIT_DEFAULT: u32 = 20;
/// S7 section 2: the largest page a client may ask for.
pub const NA_LIMIT_MAX: u32 = 50;
/// `[DEFAULT]` how long a page cursor lives (S7 2.1).
pub const CURSOR_TTL: Duration = Duration::hours(12);
/// T-704's `safe_link` limit: longer links are almost always tracking payloads.
pub const MAX_LINK_CHARS: usize = 2048;

/// The `GET /needs-attention` query string: `cursor` and `limit` only.
#[derive(Debug)]
pub struct ListParams {
    /// A sealed cursor from an earlier page, or `None` to start.
    pub cursor: Option<String>,
    /// Items wanted, 1 to [`NA_LIMIT_MAX`].
    pub limit: u32,
}

impl ListParams {
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
            limit: NA_LIMIT_DEFAULT,
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "cursor" => out.cursor = Some(value.into_owned()),
                "limit" => {
                    out.limit = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=NA_LIMIT_MAX).contains(n))
                        .ok_or_else(|| invalid("/limit"))?;
                }
                _ => return Err(invalid("")),
            }
        }
        Ok(out)
    }
}

/// One open item (schema `NeedsAttentionItem`).
#[derive(Serialize)]
pub struct NeedsAttentionItemDto {
    /// The item ID.
    pub item_id: Uuid,
    /// The mailbox the item belongs to.
    pub mailbox_id: Uuid,
    /// The sender's display name, decrypted for this response only.
    pub sender_display: String,
    /// Why the item was raised; the S7 5.8 values plus `sign_in_required`.
    pub reason_code: NeedsAttentionReason,
    /// An `https` link, or `None`; the app opens it with `rel="noopener"`.
    pub link: Option<String>,
    /// When the item was raised (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// The `GET /needs-attention` response body.
#[derive(Serialize)]
pub struct NeedsAttentionListDto {
    /// The page of open items, newest first.
    pub items: Vec<NeedsAttentionItemDto>,
    /// The open count for the tab badge.
    pub open_count: u64,
    /// A sealed cursor for the next page, or `None` at the end.
    pub next_cursor: Option<String>,
}

/// The sealed cursor payload; `after` is the store's own cursor.
#[derive(Serialize, Deserialize)]
pub struct NaCursorPayload {
    /// The store cursor to resume after.
    pub after: String,
}

/// `GET /api/v1/needs-attention` (API-NA-1).
///
/// # Errors
///
/// `400 invalid_request` for a bad `limit`, cursor or unknown parameter;
/// `401` when the session's user is gone; `500` on a store or key failure.
pub async fn list(
    State(app): State<AppState>,
    session: AuthedSession,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let params = ListParams::parse(query.as_deref())?;
    let Some(user_record) = app.ports.store.users().get(&session.user).await? else {
        return Err(ApiError::Unauthenticated);
    };
    let wrapped = &user_record.record.wrapped_data_key;
    let after = open_cursor(&app, &session, wrapped, params.cursor.as_deref()).await?;
    let page = app
        .ports
        .store
        .needs_attention()
        .by_user(
            &session.user,
            PageRequest {
                limit: params.limit,
                after,
            },
        )
        .await?;
    let now = app.ports.clock.now();
    let mut items = Vec::with_capacity(page.items.len());
    for item in &page.items {
        // The sweeper may not have run yet (NA-01 AC2): never list an item
        // whose TTL has passed.
        if item.record.expires_at <= now {
            continue;
        }
        let scope = item.record.item_id.0.to_string();
        let sender_display = open_field(
            &app,
            &session,
            wrapped,
            &scope,
            aad_fields::NA_SENDER_DISPLAY,
            &item.record.sender_display,
        )
        .await?;
        // Defence in depth (V1.2.2): a link that is not https never leaves the
        // server, whatever is in the record.
        let link = match &item.record.link {
            Some(sealed) => {
                match open_field(&app, &session, wrapped, &scope, aad_fields::NA_LINK, sealed).await
                {
                    Ok(raw) => safe_link(&raw).map(|u| u.to_string()),
                    Err(_) => None,
                }
            }
            None => None,
        };
        items.push(NeedsAttentionItemDto {
            item_id: item.record.item_id.0,
            mailbox_id: item.record.mailbox_id.0,
            sender_display,
            reason_code: item.record.reason_code,
            link,
            created_at: item.record.created_at,
        });
    }
    let open_count = app
        .ports
        .store
        .needs_attention()
        .count_for_user(&session.user)
        .await?;
    let next_cursor = seal_cursor(&app, &session, wrapped, page.next).await?;
    Ok(json_ok(
        StatusCode::OK,
        &NeedsAttentionListDto {
            items,
            open_count,
            next_cursor,
        },
    ))
}

/// `POST /api/v1/needs-attention/{item_id}/resolve` (API-NA-2).
///
/// # Errors
///
/// `404 not_found` for a missing, malformed or another user's item; `500` on a
/// store failure.
pub async fn resolve(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(item_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    delete_one(&app, &session, &item_id).await
}

/// `POST /api/v1/needs-attention/{item_id}/dismiss` (API-NA-3).
///
/// # Errors
///
/// `404 not_found` for a missing, malformed or another user's item; `500` on a
/// store failure.
pub async fn dismiss(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(item_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    delete_one(&app, &session, &item_id).await
}

/// Resolve and dismiss both delete the item (S3). A concurrent delete wins the
/// race and still counts as success (the item is gone either way).
async fn delete_one(
    app: &AppState,
    session: &AuthedSession,
    raw: &str,
) -> Result<StatusCode, ApiError> {
    // A malformed ID is a missing item: no parse detail leaks (S7 4).
    let Ok(id) = Uuid::parse_str(raw) else {
        return Err(ApiError::NotFound);
    };
    let item_id = ports::NeedsAttentionId(id);
    let Some(found) = app.ports.store.needs_attention().get(&item_id).await? else {
        return Err(ApiError::NotFound);
    };
    // Never `403` for another user's item (S7 1 principle 5); log the refusal
    // with the route template rather than any request data (V16.3.2).
    if found.record.user_id != session.user {
        crate::http::security::authz_failure(app, &session.user, None, Some("POST"));
        return Err(ApiError::NotFound);
    }
    match app
        .ports
        .store
        .needs_attention()
        .delete(&item_id, Precondition::Matches(found.version))
        .await
    {
        Ok(()) | Err(StoreError::PreconditionFailed) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(e.into()),
    }
}

/// Open a presented page cursor; any failure is `400 invalid_request`.
async fn open_cursor(
    app: &AppState,
    session: &AuthedSession,
    wrapped: &ports::WrappedKey,
    cursor: Option<&str>,
) -> Result<Option<StoreCursor>, ApiError> {
    let Some(token) = cursor else {
        return Ok(None);
    };
    let payload: NaCursorPayload = sealed_tokens(app)
        .open(
            TokenType::Cursor,
            &session.user,
            wrapped,
            &session.session_record_id,
            token,
        )
        .await
        .map_err(|e| match e {
            crate::sealed::TokenError::Unavailable => ApiError::Internal,
            _ => invalid(""),
        })?;
    Ok(Some(StoreCursor(payload.after)))
}

/// Seal the store's next cursor for the wire, or `None` at the end.
async fn seal_cursor(
    app: &AppState,
    session: &AuthedSession,
    wrapped: &ports::WrappedKey,
    next: Option<StoreCursor>,
) -> Result<Option<String>, ApiError> {
    let Some(next) = next else {
        return Ok(None);
    };
    let token = sealed_tokens(app)
        .seal(
            TokenType::Cursor,
            &session.user,
            wrapped,
            &session.session_record_id,
            app.ports.clock.now() + CURSOR_TTL,
            &NaCursorPayload { after: next.0 },
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok(Some(token))
}

/// Open one sealed item field under the user's key; a failure is `500`.
async fn open_field(
    app: &AppState,
    session: &AuthedSession,
    wrapped: &ports::WrappedKey,
    scope: &str,
    field: &'static str,
    sealed: &Ciphertext,
) -> Result<String, ApiError> {
    let aad = Aad {
        user: session.user,
        scope: scope.to_owned(),
        field,
    };
    let plain = app
        .ports
        .keys
        .open(&session.user, wrapped, &aad, &sealed.0)
        .await
        .map_err(|_| ApiError::Internal)?;
    String::from_utf8(plain).map_err(|_| ApiError::Internal)
}

/// The `safe_link` rules (T-704) until that task lands in `svc-common`.
///
/// Keeps only: scheme `https`, a host, no username or password, at most
/// [`MAX_LINK_CHARS`] characters, and no ASCII control character or whitespace
/// anywhere in the input (S7 5.8, V1.2.2).
// TODO(T-704): replace with `svc_common::links::safe_link` once T-704 merges.
fn safe_link(raw: &str) -> Option<Url> {
    if raw.chars().count() > MAX_LINK_CHARS {
        return None;
    }
    if raw
        .chars()
        .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    url.host()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
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

fn sealed_tokens(app: &AppState) -> SealedTokens {
    SealedTokens::new(app.ports.keys.clone(), app.ports.clock.clone())
}
