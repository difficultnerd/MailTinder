//! API-HIST-1: read-only, filtered History from the user's app folder (T-801).
use std::sync::Arc;

use axum::extract::{rejection::QueryRejection, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use domain::user_state::{HistoryAction, HistoryOutcome};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::sealed::{SealedTokens, TokenType, MAX_TOKEN_TTL};
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;
use crate::text::plain_text;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    pub filter: Option<HistoryFilter>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryFilter {
    All,
    Unsubscribes,
    RuleActions,
    Filing,
}

#[derive(Serialize)]
pub struct HistoryEntryDto {
    pub entry_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub mailbox_id: Uuid,
    pub sender_display: String,
    pub action: HistoryAction,
    pub outcome: HistoryOutcome,
    pub rule_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntryDto>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCursor {
    pub filter: HistoryFilter,
    pub before_at: OffsetDateTime,
    pub before_id: Uuid,
}

#[must_use]
pub fn matches_filter(f: HistoryFilter, a: HistoryAction) -> bool {
    match f {
        HistoryFilter::All => match a {
            HistoryAction::TrashedByRule
            | HistoryAction::Unsubscribe
            | HistoryAction::Filed
            | HistoryAction::FiledByRule
            | HistoryAction::Blocked
            | HistoryAction::ReportedSpam => true,
        },
        HistoryFilter::Unsubscribes => match a {
            HistoryAction::Unsubscribe => true,
            HistoryAction::TrashedByRule
            | HistoryAction::Filed
            | HistoryAction::FiledByRule
            | HistoryAction::Blocked
            | HistoryAction::ReportedSpam => false,
        },
        HistoryFilter::RuleActions => match a {
            HistoryAction::TrashedByRule | HistoryAction::FiledByRule | HistoryAction::Blocked => {
                true
            }
            HistoryAction::Unsubscribe | HistoryAction::Filed | HistoryAction::ReportedSpam => {
                false
            }
        },
        HistoryFilter::Filing => match a {
            HistoryAction::Filed | HistoryAction::FiledByRule => true,
            HistoryAction::TrashedByRule
            | HistoryAction::Unsubscribe
            | HistoryAction::Blocked
            | HistoryAction::ReportedSpam => false,
        },
    }
}

fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: vec![field.to_owned()],
    }
}

/// `GET /api/v1/history`. Default read limits are applied by the router.
///
/// # Errors
/// Returns `400 invalid_request` for invalid query fields or cursors, and
/// propagates state-file, provider and key-service failures.
pub async fn list(
    State(app): State<AppState>,
    session: AuthedSession,
    query: Result<Query<HistoryQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|_| invalid(""))?;
    let limit = query.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(invalid("limit"));
    }
    let filter = query.filter.unwrap_or(HistoryFilter::All);
    let now = app.ports.clock.now();
    let mut history = UserStateStore::new(Arc::new(app.clone()))
        .load(&session.user)
        .await?
        .state
        .history;
    history.retain(|e| e.at >= now - Duration::days(365) && matches_filter(filter, e.action));
    history.sort_unstable_by_key(|e| std::cmp::Reverse((e.at, e.entry_id)));
    let tokens = SealedTokens::new(app.ports.keys.clone(), app.ports.clock.clone());
    let wrapped = app
        .ports
        .store
        .users()
        .get(&session.user)
        .await?
        .ok_or(ApiError::Internal)?
        .record
        .wrapped_data_key;
    if let Some(token) = query.cursor {
        let cursor: HistoryCursor = tokens
            .open(
                TokenType::Cursor,
                &session.user,
                &wrapped,
                &session.session_record_id,
                &token,
            )
            .await
            .map_err(|_| invalid("cursor"))?;
        if cursor.filter != filter {
            return Err(invalid("cursor"));
        }
        history.retain(|e| (e.at, e.entry_id) < (cursor.before_at, cursor.before_id));
    }
    let has_more = history.len() > limit as usize;
    history.truncate(limit as usize);
    let next_cursor = if has_more {
        let last = history.last().ok_or(ApiError::Internal)?;
        Some(
            tokens
                .seal(
                    TokenType::Cursor,
                    &session.user,
                    &wrapped,
                    &session.session_record_id,
                    now + MAX_TOKEN_TTL,
                    &HistoryCursor {
                        filter,
                        before_at: last.at,
                        before_id: last.entry_id,
                    },
                )
                .await
                .map_err(|_| ApiError::Internal)?,
        )
    } else {
        None
    };
    let entries = history
        .into_iter()
        .map(|e| HistoryEntryDto {
            entry_id: e.entry_id,
            at: e.at,
            mailbox_id: e.mailbox_id.0,
            sender_display: plain_text(&e.sender_display, 256),
            action: e.action,
            outcome: e.outcome,
            rule_id: e.rule_id.map(|id| id.0),
        })
        .collect();
    Ok(json_ok(
        StatusCode::OK,
        &HistoryPage {
            entries,
            next_cursor,
        },
    ))
}
