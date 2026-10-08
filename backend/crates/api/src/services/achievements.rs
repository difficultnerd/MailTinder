//! Pure achievement recording inside retryable user-state updates (T-802).
use std::collections::BTreeSet;

use domain::user_state::{AchievementRecord, Totals, UserState};
use domain::{newly_unlocked, AchievementId, AchievementProgress};
use time::OffsetDateTime;
use uuid::Uuid;

#[must_use]
pub fn progress_from(totals: &Totals) -> AchievementProgress {
    AchievementProgress {
        unsubscribes_queued: totals.unsubscribes_queued,
        senders_silenced: totals.senders_silenced,
        years_cleared: totals.years_cleared,
        cleared: totals.cleared,
        categories_created: totals.categories_created,
        people_blocked: totals.people_blocked,
        round_unsubscribes: totals.round_unsubscribes,
    }
}

pub fn record_unlocks(
    state: &mut UserState,
    session: Uuid,
    now: OffsetDateTime,
) -> Vec<AchievementRecord> {
    let mut progress = progress_from(&state.totals);
    if state.totals.round_session != Some(session) {
        progress.round_unsubscribes = 0;
    }
    let already: BTreeSet<_> = AchievementId::ALL
        .into_iter()
        .filter(|id| {
            state
                .achievements
                .iter()
                .any(|a| a.achievement_id == id.as_str())
        })
        .collect();
    let records: Vec<_> = newly_unlocked(&progress, &already)
        .into_iter()
        .map(|id| AchievementRecord {
            achievement_id: id.as_str().to_owned(),
            unlocked_at: now,
        })
        .collect();
    state.achievements.extend(records.iter().cloned());
    records
}
