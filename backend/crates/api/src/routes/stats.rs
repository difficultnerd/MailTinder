//! API-STAT-1: read-only stats and unlocked achievements (T-802).
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use domain::{mail_stopped_per_year, AchievementId};
use serde::Serialize;
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::http::json::json_ok;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StatsDto {
    pub emails_triaged: u64,
    pub senders_unsubscribed: u64,
    pub unsubscribes_confirmed: u64,
    pub mail_stopped_per_year: u64,
    pub achievements: Vec<AchievementDto>,
}

#[derive(Serialize)]
pub struct AchievementDto {
    pub achievement_id: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub unlocked_at: OffsetDateTime,
}

/// Read stats without changing the user's state file.
///
/// # Errors
/// Returns state-file loading errors.
pub async fn stats(app: &AppState, session: &AuthedSession) -> Result<StatsDto, ApiError> {
    let loaded = UserStateStore::new(Arc::new(app.clone()))
        .load(&session.user)
        .await?;
    let state = loaded.state;
    let achievements = state
        .achievements
        .iter()
        .map(|record| {
            let id = AchievementId::ALL
                .into_iter()
                .find(|id| id.as_str() == record.achievement_id)
                .ok_or(ApiError::Internal)?;
            Ok(AchievementDto {
                achievement_id: id.as_str(),
                unlocked_at: record.unlocked_at,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(StatsDto {
        emails_triaged: state.totals.triaged,
        senders_unsubscribed: state.totals.senders_unsubscribed,
        unsubscribes_confirmed: state.totals.unsubscribes_confirmed,
        mail_stopped_per_year: mail_stopped_per_year(
            state.rules.iter().map(|r| (r.rule.enabled, r.yearly_rate)),
        ),
        achievements,
    })
}

/// # Errors
/// Returns state-file loading errors.
pub async fn get_stats(
    State(app): State<AppState>,
    session: AuthedSession,
) -> Result<Response, ApiError> {
    Ok(json_ok(StatusCode::OK, &stats(&app, &session).await?))
}
