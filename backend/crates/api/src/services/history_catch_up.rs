//! Transfer terminal job outcomes to the user's History (T-609).
use domain::user_state::{
    HistoryAction, HistoryEntry, HistoryOutcome, PendingUnsubscribe, UserState,
};
use domain::{JobId, JobStatus, UserId};
use obs::{Pseudonymiser, SecurityEvent};
use ports::Precondition;
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Default)]
pub struct CollectedOutcomes {
    pub entries: Vec<HistoryEntry>,
    pub job_ids: Vec<JobId>,
    pub sent: Vec<(JobId, PendingUnsubscribe, OffsetDateTime)>,
}

/// Exhaustive mapping: unfinished jobs never enter History.
#[must_use]
pub fn history_outcome(status: JobStatus) -> Option<HistoryOutcome> {
    match status {
        JobStatus::Sent => Some(HistoryOutcome::Sent),
        JobStatus::NeedsAttention => Some(HistoryOutcome::NeedsAttention),
        JobStatus::Failed => Some(HistoryOutcome::Failed),
        JobStatus::Expired => Some(HistoryOutcome::Expired),
        JobStatus::Cancelled => Some(HistoryOutcome::Cancelled),
        JobStatus::Queued | JobStatus::Running => None,
    }
}

/// Read outcomes without deleting them; deletion requires a successful state write.
///
/// # Errors
/// Returns a server-store error when the outcome query fails.
pub async fn collect_job_outcomes(
    app: &AppState,
    user: &UserId,
    state: &UserState,
) -> Result<CollectedOutcomes, ApiError> {
    let mut collected = CollectedOutcomes::default();
    for job in app
        .ports
        .store
        .jobs()
        .by_user_with_outcome(user, 500)
        .await?
    {
        let record = job.record;
        let Some(outcome) = record.outcome else {
            continue;
        };
        let Some(history) = history_outcome(record.status) else {
            continue;
        };
        let pending = state.pending_unsubscribes.get(&record.job_id.0);
        collected.entries.push(HistoryEntry {
            entry_id: record.job_id.0,
            at: outcome.at,
            mailbox_id: record.mailbox_id,
            action: HistoryAction::Unsubscribe,
            outcome: history,
            rule_id: pending.and_then(|p| p.rule_id),
            sender_display: pending
                .map_or_else(|| "Unknown sender".to_owned(), |p| p.sender_display.clone()),
        });
        collected.job_ids.push(record.job_id);
        if history == HistoryOutcome::Sent {
            if let Some(pending) = pending {
                collected
                    .sent
                    .push((record.job_id, pending.clone(), outcome.at));
            }
        }
    }
    Ok(collected)
}

/// Best-effort cleanup after History is durable; a failed deletion is retried.
pub(crate) async fn delete_collected(app: &AppState, user: &UserId, collected: &CollectedOutcomes) {
    for job in &collected.job_ids {
        if app
            .ports
            .store
            .jobs()
            .delete(job, Precondition::None)
            .await
            .is_err()
        {
            obs::security_event(&SecurityEvent {
                action: "history_catch_up",
                outcome: "delete_failed",
                user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
                request_id: None,
                amr: None,
                provider: None,
                method: None,
            });
        }
    }
}
