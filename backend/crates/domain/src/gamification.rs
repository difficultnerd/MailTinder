//! Gamification calculators: the yearly mail-stopped estimate (GM-05), the
//! Stats total over enabled rules (ST-02 AC2) and the fixed achievement list
//! with an idempotent unlock check (GM-06). Pure: no clock, no I/O.
//!
//! The spec (T-109) mandates the exact `yearly_rate` signature and its
//! integer-only scaling algorithm, which necessarily casts between `i64`
//! (`time::Duration::whole_days`), `u64` and `u32`. Those casts are safe here
//! (the window is clamped to at least one day and the rate saturates at
//! `u32::MAX`), so the pedantic cast lints are allowed at the crate level
//! rather than suppressing individual items.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_lossless
)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::Duration;

/// GM-05 AC1: yearly rate from the count of the sender's matching messages
/// over the past `window` (90 days). `count` None (the count query failed)
/// gives None (GM-05 AC3: shown as unknown).
pub fn yearly_rate(count: Option<u64>, window: Duration) -> Option<u32> {
    let n = count?;
    let days = window.whole_days().max(1) as u64;
    let scaled = n.saturating_mul(365).saturating_add(days / 2) / days;
    Some(scaled.min(u32::MAX as u64) as u32)
}

/// ST-02 AC2: sum of `yearly_rate` over enabled rules, ignoring None.
pub fn mail_stopped_per_year<I: IntoIterator<Item = (bool, Option<u32>)>>(rules: I) -> u64 {
    rules
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .filter_map(|(_, rate)| rate)
        .map(u64::from)
        .sum()
}

/// The fixed list of achievements (GM-06 AC1). The wire IDs are the S7
/// `snake_case` strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AchievementId {
    FirstUnsubscribe,
    #[serde(rename = "senders_silenced_100")]
    SendersSilenced100,
    YearCleared,
    #[serde(rename = "cleared_1000")]
    Cleared1000,
    FirstFilingCategory,
    FirstBlockedPerson,
    TenUnsubscribesInRound,
}

impl AchievementId {
    pub const ALL: [AchievementId; 7] = [
        AchievementId::FirstUnsubscribe,
        AchievementId::SendersSilenced100,
        AchievementId::YearCleared,
        AchievementId::Cleared1000,
        AchievementId::FirstFilingCategory,
        AchievementId::FirstBlockedPerson,
        AchievementId::TenUnsubscribesInRound,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AchievementId::FirstUnsubscribe => "first_unsubscribe",
            AchievementId::SendersSilenced100 => "senders_silenced_100",
            AchievementId::YearCleared => "year_cleared",
            AchievementId::Cleared1000 => "cleared_1000",
            AchievementId::FirstFilingCategory => "first_filing_category",
            AchievementId::FirstBlockedPerson => "first_blocked_person",
            AchievementId::TenUnsubscribesInRound => "ten_unsubscribes_in_round",
        }
    }
}

/// Running totals the api keeps in the user state file (T-602b `Totals`, plus
/// the fields below).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AchievementProgress {
    /// Jobs queued by rejects, all time.
    pub unsubscribes_queued: u64,
    /// `reject_list` and `block_person` rules created, all time.
    pub senders_silenced: u64,
    /// GM-04 level completions.
    pub years_cleared: u64,
    /// Rejects plus files, all time (T-602b `Totals.cleared`).
    pub cleared: u64,
    pub categories_created: u64,
    pub people_blocked: u64,
    /// Unsubscribes queued in the current round (see Algorithm 4).
    pub round_unsubscribes: u64,
}

/// GM-06: returns the achievements newly unlocked by `progress`, given those
/// already unlocked. Unlocks are idempotent: an achievement already in
/// `already` is never returned again.
pub fn newly_unlocked(
    progress: &AchievementProgress,
    already: &BTreeSet<AchievementId>,
) -> Vec<AchievementId> {
    AchievementId::ALL
        .into_iter()
        .filter(|id| !already.contains(id))
        .filter(|id| threshold_met(*id, progress))
        .collect()
}

/// Whether `progress` meets the threshold for `id` (GM-06 AC1).
fn threshold_met(id: AchievementId, progress: &AchievementProgress) -> bool {
    match id {
        AchievementId::FirstUnsubscribe => progress.unsubscribes_queued >= 1,
        AchievementId::SendersSilenced100 => progress.senders_silenced >= 100,
        AchievementId::YearCleared => progress.years_cleared >= 1,
        AchievementId::Cleared1000 => progress.cleared >= 1000,
        AchievementId::FirstFilingCategory => progress.categories_created >= 1,
        AchievementId::FirstBlockedPerson => progress.people_blocked >= 1,
        AchievementId::TenUnsubscribesInRound => progress.round_unsubscribes >= 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn days(n: i64) -> Duration {
        Duration::days(n)
    }

    impl Arbitrary for AchievementProgress {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
            (
                any::<u64>(),
                any::<u64>(),
                any::<u64>(),
                any::<u64>(),
                any::<u64>(),
                any::<u64>(),
                any::<u64>(),
            )
                .prop_map(
                    |(
                        unsubscribes_queued,
                        senders_silenced,
                        years_cleared,
                        cleared,
                        categories_created,
                        people_blocked,
                        round_unsubscribes,
                    )| Self {
                        unsubscribes_queued,
                        senders_silenced,
                        years_cleared,
                        cleared,
                        categories_created,
                        people_blocked,
                        round_unsubscribes,
                    },
                )
                .boxed()
        }
    }

    #[test]
    fn gm_05_ac1_ninety_day_count_scaled_to_year() {
        assert_eq!(yearly_rate(Some(90), days(90)), Some(365));
        assert_eq!(yearly_rate(Some(0), days(90)), Some(0));
        assert_eq!(yearly_rate(Some(1), days(90)), Some(4));
    }

    #[test]
    fn gm_05_ac1_rate_saturates() {
        assert_eq!(yearly_rate(Some(u64::MAX), days(90)), Some(u32::MAX));
    }

    #[test]
    fn gm_05_ac3_failed_count_is_unknown() {
        assert_eq!(yearly_rate(None, days(90)), None);
    }

    #[test]
    fn st_02_ac2_total_ignores_disabled_and_unknown() {
        let rules = [
            (true, Some(365)),
            (true, None),
            (false, Some(1000)),
            (false, None),
            (true, Some(4)),
        ];
        assert_eq!(mail_stopped_per_year(rules), 369);
    }

    proptest! {
        #[test]
        fn st_02_ac2_total_is_sum(
            rules in proptest::collection::vec(
                (proptest::bool::ANY, proptest::option::of(0u32..=1_000_000)),
                0..20,
            ),
        ) {
            let expected: u64 = rules
                .iter()
                .filter(|(enabled, _)| *enabled)
                .filter_map(|(_, rate)| *rate)
                .map(u64::from)
                .sum();
            prop_assert_eq!(mail_stopped_per_year(rules), expected);
        }
    }

    #[test]
    fn gm_06_ac1_each_threshold() {
        // Just below and at the threshold for each achievement.
        let cases = [
            (AchievementId::FirstUnsubscribe, 0, 1),
            (AchievementId::SendersSilenced100, 99, 100),
            (AchievementId::YearCleared, 0, 1),
            (AchievementId::Cleared1000, 999, 1000),
            (AchievementId::FirstFilingCategory, 0, 1),
            (AchievementId::FirstBlockedPerson, 0, 1),
            (AchievementId::TenUnsubscribesInRound, 9, 10),
        ];

        for (id, below_value, at_value) in cases {
            let mut below = AchievementProgress::default();
            let mut at = AchievementProgress::default();
            match id {
                AchievementId::FirstUnsubscribe => {
                    below.unsubscribes_queued = below_value;
                    at.unsubscribes_queued = at_value;
                }
                AchievementId::SendersSilenced100 => {
                    below.senders_silenced = below_value;
                    at.senders_silenced = at_value;
                }
                AchievementId::YearCleared => {
                    below.years_cleared = below_value;
                    at.years_cleared = at_value;
                }
                AchievementId::Cleared1000 => {
                    below.cleared = below_value;
                    at.cleared = at_value;
                }
                AchievementId::FirstFilingCategory => {
                    below.categories_created = below_value;
                    at.categories_created = at_value;
                }
                AchievementId::FirstBlockedPerson => {
                    below.people_blocked = below_value;
                    at.people_blocked = at_value;
                }
                AchievementId::TenUnsubscribesInRound => {
                    below.round_unsubscribes = below_value;
                    at.round_unsubscribes = at_value;
                }
            }
            let empty = BTreeSet::new();
            assert!(
                newly_unlocked(&below, &empty).is_empty(),
                "{id:?} should not unlock below its threshold"
            );
            assert_eq!(newly_unlocked(&at, &empty), vec![id]);
        }
    }

    proptest! {
        #[test]
        fn gm_06_ac2_unlock_is_idempotent(progress in any::<AchievementProgress>()) {
            let already = BTreeSet::new();
            let first = newly_unlocked(&progress, &already);
            let already: BTreeSet<AchievementId> = first.iter().copied().collect();
            let second = newly_unlocked(&progress, &already);
            prop_assert!(second.is_empty());
        }
    }

    #[test]
    fn gm_06_ac1_ids_serialise_snake_case() -> TestResult {
        assert_eq!(
            serde_json::to_string(&AchievementId::FirstUnsubscribe)?,
            r#""first_unsubscribe""#
        );
        assert_eq!(
            serde_json::to_string(&AchievementId::SendersSilenced100)?,
            r#""senders_silenced_100""#
        );
        assert_eq!(
            serde_json::to_string(&AchievementId::TenUnsubscribesInRound)?,
            r#""ten_unsubscribes_in_round""#
        );
        assert_eq!(
            serde_json::to_string(&AchievementId::Cleared1000)?,
            r#""cleared_1000""#
        );
        Ok(())
    }
}
