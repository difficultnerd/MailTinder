//! Block-rule creation and achievement recording (T-802).
use std::sync::Arc;

use domain::user_state::{AchievementRecord, StoredRule};
use domain::RuleKind;

use crate::error::ApiError;
use crate::services::achievements::record_unlocks;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// Persist a block rule once and unlock achievements in the same update.
///
/// # Errors
/// Rejects non-block rules and propagates state-file errors.
pub async fn create_block(
    app: &AppState,
    session: &AuthedSession,
    rule: StoredRule,
) -> Result<Vec<AchievementRecord>, ApiError> {
    if rule.rule.kind != RuleKind::BlockPerson || !rule.rule.enabled {
        return Err(ApiError::InvalidRequest {
            fields: vec!["kind".to_owned()],
        });
    }
    let now = app.ports.clock.now();
    UserStateStore::new(Arc::new(app.clone()))
        .update(&session.user, |state| {
            if state.rules.iter().any(|r| {
                r.rule.rule_id == rule.rule.rule_id
                    || (r.rule.enabled
                        && r.rule.kind == RuleKind::BlockPerson
                        && r.rule.matcher == rule.rule.matcher)
            }) {
                return Vec::new();
            }
            state.rules.push(rule.clone());
            state.totals.people_blocked = state.totals.people_blocked.saturating_add(1);
            state.totals.senders_silenced = state.totals.senders_silenced.saturating_add(1);
            record_unlocks(state, session.session_record_id.0, now)
        })
        .await
}
