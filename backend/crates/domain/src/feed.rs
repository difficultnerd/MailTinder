//! Feed helpers: the ordering every Feed page uses, the card visibility rule,
//! where a skipped card comes back, the backlog level (calendar year) and
//! level completion, and boss senders. Pure: no clock, no randomness (the
//! draw comes in as an argument), no I/O.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use time::{Date, Month, OffsetDateTime};

use crate::message::MessageMeta;
use crate::sender::SenderStats;
use crate::tunables::Tunables;

/// Newest first by `internal_date`, then mailbox ID, then message ID (stable,
/// deterministic). FD-02 AC1.
pub fn feed_order(a: &MessageMeta, b: &MessageMeta) -> Ordering {
    b.internal_date
        .cmp(&a.internal_date)
        .then_with(|| a.mailbox.cmp(&b.mailbox))
        .then_with(|| a.id.as_str().cmp(b.id.as_str()))
}

/// Sort a slice of message metadata for the Feed, newest first. FD-02 AC1.
pub fn sort_for_feed(metas: &mut [MessageMeta]) {
    metas.sort_by(feed_order);
}

/// S3 "Card visibility": hidden when it matched an enabled rule, left the
/// inbox, or reached the skip limit. A card skipped twice has returned twice;
/// the third skip makes the count 3 and hides it until a new session.
pub fn card_visible(
    in_inbox: bool,
    matched_enabled_rule: bool,
    skips_this_session: u8,
    t: &Tunables,
) -> bool {
    in_inbox && !matched_enabled_rule && skips_this_session <= t.skip_max_returns
}

/// `[DEFAULT]` a random return lands 1..=20 cards after the page end.
pub const SKIP_RANDOM_SPAN: u32 = 20;

/// Called when a card is skipped. `skips_this_session` includes this skip.
/// Returns how many cards later it comes back (T-602b `SkipReturn.after_cards`),
/// or `None` when it must not return this session (SW-02 AC2). `draw` comes
/// from the Rng port (seeded in tests).
pub fn choose_skip_return(
    skips_this_session: u8,
    cards_left_on_page: u32,
    draw: u64,
    t: &Tunables,
) -> Option<u32> {
    if skips_this_session > t.skip_max_returns {
        return None;
    }
    // `[DEFAULT]` the two S2 options are both used: when `draw % 2 == 0` the
    // card returns at the end of the current page; otherwise at a random later
    // position. Mixing them keeps the queue from feeling mechanical.
    if draw % 2 == 0 {
        Some(cards_left_on_page.max(1))
    } else {
        let later = u64::from(cards_left_on_page) + 1 + (draw / 2) % u64::from(SKIP_RANDOM_SPAN);
        Some(u32::try_from(later).unwrap_or(u32::MAX))
    }
}

/// GM-04: the level is the calendar year of the backlog position, shown only
/// in the backlog phase. `None` unless `in_backlog` (AC1: only once new mail
/// is cleared). With a `backlog_ceiling`, the year of that time in UTC; with
/// none (backlog just started), `None` until the first backlog page sets a
/// ceiling.
pub fn current_level(backlog_ceiling: Option<OffsetDateTime>, in_backlog: bool) -> Option<i32> {
    if !in_backlog {
        return None;
    }
    backlog_ceiling.map(OffsetDateTime::year)
}

/// `[start, end)` UTC for a calendar year: `[year-01-01T00:00:00Z,
/// (year+1)-01-01T00:00:00Z)`. `None` for a year the `time` crate cannot
/// represent. `[DEFAULT]` UTC year boundaries; S2 does not say whose time
/// zone, and one provider date-range query per mailbox needs fixed bounds.
pub fn level_range(year: i32) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let start = Date::from_calendar_date(year, Month::January, 1)
        .ok()?
        .midnight()
        .assume_utc();
    let end = Date::from_calendar_date(year + 1, Month::January, 1)
        .ok()?
        .midnight()
        .assume_utc();
    Some((start, end))
}

/// GM-04 level progress. `remaining == 0` gives `Complete` (AC2), else
/// `Continue`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelProgress {
    Continue { year: i32 },
    Complete { cleared: i32, next: i32 },
}

/// GM-04 AC2: when a year has no mail left, it is complete and the next older
/// year begins.
pub fn level_progress(year: i32, remaining: u64) -> LevelProgress {
    if remaining == 0 {
        LevelProgress::Complete {
            cleared: year,
            next: year - 1,
        }
    } else {
        LevelProgress::Continue { year }
    }
}

/// GM-08 AC1: at least `boss_min_seen` cards seen, among the top
/// `boss_top_n` senders by `seen`, not defeated. Rank all non-defeated senders
/// with `seen >= boss_min_seen` by `seen` descending, ties by key ascending;
/// the sender is a boss if its rank is below `boss_top_n`.
pub fn is_boss(sender_key: &str, all: &BTreeMap<String, SenderStats>, t: &Tunables) -> bool {
    let Some(stats) = all.get(sender_key) else {
        return false;
    };
    if stats.seen < t.boss_min_seen || stats.boss_defeated {
        return false;
    }
    let rank = all
        .iter()
        .filter(|(_, s)| s.seen >= t.boss_min_seen && !s.boss_defeated)
        .filter(|(k, _)| *k != sender_key)
        .filter(|(k, s)| s.seen > stats.seen || (s.seen == stats.seen && k.as_str() < sender_key))
        .count();
    rank < t.boss_top_n
}

/// GM-08 AC3: a reject of a boss defeats it. Returns true when this reject
/// defeated a boss. The caller computes `was_boss` with `is_boss` before
/// applying the reject.
pub fn defeat_boss_on_reject(stats: &mut SenderStats, was_boss: bool) -> bool {
    if was_boss {
        stats.boss_defeated = true;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use proptest::prelude::*;
    use std::collections::BTreeMap;
    use time::macros::datetime;

    use crate::ids::{MailboxId, MessageId};
    use crate::message::{HeaderFacts, LabelSet};
    use crate::sender::SenderKey;

    fn meta(mailbox: &str, id: &str, internal_date: OffsetDateTime) -> MessageMeta {
        MessageMeta {
            mailbox: MailboxId::new(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                mailbox.as_bytes(),
            )),
            id: MessageId::new({
                let cleaned: String = id.chars().filter(|c| !c.is_control()).take(256).collect();
                if cleaned.is_empty() {
                    "x".to_owned()
                } else {
                    cleaned
                }
            })
            .unwrap_or_else(|_| panic!("valid message id")),
            internal_date,
            from_display: String::new(),
            from_address: String::new(),
            sender: SenderKey::from_address("a@example.com"),
            subject: String::new(),
            labels: LabelSet::new(),
            facts: HeaderFacts::default(),
        }
    }

    #[test]
    fn fd_02_ac1_interleaved_newest_first() {
        let t1 = datetime!(2026-01-01 00:00:00 UTC);
        let t2 = datetime!(2026-01-02 00:00:00 UTC);
        let t3 = datetime!(2026-01-03 00:00:00 UTC);
        let mut metas = vec![
            meta("mb-a", "a1", t1),
            meta("mb-b", "b1", t3),
            meta("mb-a", "a2", t2),
            meta("mb-b", "b2", t2),
        ];
        sort_for_feed(&mut metas);
        let order: Vec<&str> = metas.iter().map(|m| m.id.as_str()).collect();
        // Newest first; the two t2 cards tie on time and are ordered by
        // mailbox (mb-a before mb-b), then by id.
        assert_eq!(order, vec!["b1", "a2", "b2", "a1"]);
    }

    proptest::proptest! {
        #[test]
        fn fd_02_ac1_order_total_and_stable(
            metas in proptest::collection::vec(
                (any::<String>(), any::<String>(), 0i64..1_000_000_000),
                0..30,
            ),
        ) {
            let built: Vec<MessageMeta> = metas
                .iter()
                .map(|(mb, id, secs)| {
                    meta(mb, id, OffsetDateTime::from_unix_timestamp(*secs).unwrap_or_else(|_| panic!("valid")))
                })
                .collect();
            let mut expected = built.clone();
            sort_for_feed(&mut expected);
            // Shuffle deterministically and re-sort: the result is identical.
            let mut shuffled = built.clone();
            shuffled.sort_by_key(|m| m.id.as_str().len());
            sort_for_feed(&mut shuffled);
            let expected_ids: Vec<String> = expected.iter().map(|m| m.id.as_str().to_owned()).collect();
            let shuffled_ids: Vec<String> = shuffled.iter().map(|m| m.id.as_str().to_owned()).collect();
            prop_assert_eq!(expected_ids, shuffled_ids);
            // Total order: no two distinct messages compare equal.
            for i in 0..built.len() {
                for j in (i + 1)..built.len() {
                    prop_assert_ne!(feed_order(&built[i], &built[j]), Ordering::Equal);
                }
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn fd_03_ac4_no_daily_cap(
            in_inbox in proptest::bool::ANY,
            matched in proptest::bool::ANY,
            skips in 0u8..=3,
            shown_before in 0u64..1_000_000,
        ) {
            let t = Tunables::default();
            let _ = shown_before; // the visibility rule never reads it
            let visible = card_visible(in_inbox, matched, skips, &t);
            // The rule depends only on its three inputs, never on how many
            // cards were shown before.
            prop_assert_eq!(visible, in_inbox && !matched && skips <= t.skip_max_returns);
        }
    }

    #[test]
    fn fd_04_ac1_message_left_inbox_hidden() {
        let t = Tunables::default();
        assert!(!card_visible(false, false, 0, &t));
        assert!(card_visible(true, false, 0, &t));
    }

    #[test]
    fn sw_02_ac2_skip_returns_at_most_twice() {
        let t = Tunables::default();
        // Skips 1 and 2 return; skip 3 gives None and hides the card.
        assert!(choose_skip_return(1, 5, 0, &t).is_some());
        assert!(choose_skip_return(2, 5, 0, &t).is_some());
        assert_eq!(choose_skip_return(3, 5, 0, &t), None);
        assert!(!card_visible(true, false, 3, &t));
        // Boundary: two returns are allowed.
        assert!(card_visible(true, false, 2, &t));
    }

    proptest::proptest! {
        #[test]
        fn sw_02_ac2_seeded_draw_is_deterministic(
            skips in 1u8..=2,
            cards_left in 0u32..100,
            draw in 0u64..1_000_000,
        ) {
            let t = Tunables::default();
            let a = choose_skip_return(skips, cards_left, draw, &t);
            let b = choose_skip_return(skips, cards_left, draw, &t);
            prop_assert_eq!(a, b);
            let pos = a.unwrap_or_else(|| panic!("within skip limit"));
            prop_assert!(pos >= 1);
            prop_assert!(pos <= cards_left + SKIP_RANDOM_SPAN);
        }
    }

    #[test]
    fn gm_04_ac1_level_is_backlog_year() {
        let ceiling = datetime!(2023-06-15 12:00:00 UTC);
        assert_eq!(current_level(Some(ceiling), true), Some(2023));
    }

    #[test]
    fn gm_04_ac1_no_level_in_new_phase() {
        assert_eq!(
            current_level(Some(datetime!(2023-06-15 12:00:00 UTC)), false),
            None
        );
        assert_eq!(current_level(None, true), None);
        assert_eq!(current_level(None, false), None);
    }

    #[test]
    fn gm_04_ac2_year_complete_moves_to_previous_year() {
        assert_eq!(
            level_progress(2023, 0),
            LevelProgress::Complete {
                cleared: 2023,
                next: 2022
            }
        );
        assert_eq!(
            level_progress(2023, 1),
            LevelProgress::Continue { year: 2023 }
        );
    }

    #[test]
    fn gm_04_ac3_level_from_position_only() {
        // The function takes only the position; nothing else is stored or
        // read. Calling it with a ceiling and the backlog flag is the whole
        // contract.
        let ceiling = datetime!(2024-03-01 00:00:00 UTC);
        assert_eq!(current_level(Some(ceiling), true), Some(2024));
    }

    #[test]
    fn gm_04_level_range_bounds_utc() {
        let (start, end) = level_range(2023).unwrap_or_else(|| panic!("representable year"));
        assert_eq!(start, datetime!(2023-01-01 00:00:00 UTC));
        assert_eq!(end, datetime!(2024-01-01 00:00:00 UTC));
        // The range is half-open: the start is included, the end is not.
        assert!(start < end);
    }

    fn stats(seen: u32, defeated: bool) -> SenderStats {
        SenderStats {
            seen,
            boss_defeated: defeated,
            ..SenderStats::default()
        }
    }

    #[test]
    fn gm_08_ac1_boss_needs_20_seen_and_top_5() {
        let t = Tunables::default();
        let mut all = BTreeMap::new();
        // Six senders with 20+ seen; the sixth by count is not a boss.
        for (i, seen) in [100u32, 90, 80, 70, 60, 20].into_iter().enumerate() {
            all.insert(format!("s{i}@example.com"), stats(seen, false));
        }
        // 19 seen is not a boss even when it would be top by count.
        all.insert("low@example.com".to_owned(), stats(19, false));
        assert!(is_boss("s0@example.com", &all, &t));
        assert!(is_boss("s4@example.com", &all, &t));
        assert!(!is_boss("s5@example.com", &all, &t));
        assert!(!is_boss("low@example.com", &all, &t));
        assert!(!is_boss("missing@example.com", &all, &t));
    }

    #[test]
    fn gm_08_ac1_ties_broken_by_key() {
        let t = Tunables::default();
        let mut all = BTreeMap::new();
        // Six senders all with the same seen count; ties break by key
        // ascending, so the first five keys are bosses and the sixth is not.
        for key in [
            "a@example.com",
            "b@example.com",
            "c@example.com",
            "d@example.com",
            "e@example.com",
            "f@example.com",
        ] {
            all.insert(key.to_owned(), stats(50, false));
        }
        assert!(is_boss("a@example.com", &all, &t));
        assert!(is_boss("e@example.com", &all, &t));
        assert!(!is_boss("f@example.com", &all, &t));
    }

    #[test]
    fn gm_08_ac3_rejecting_boss_defeats_it() {
        let t = Tunables::default();
        let mut all = BTreeMap::new();
        for (i, seen) in [100u32, 90, 80, 70, 60, 20].into_iter().enumerate() {
            all.insert(format!("s{i}@example.com"), stats(seen, false));
        }
        let key = "s0@example.com";
        assert!(is_boss(key, &all, &t));
        let stats = all.get_mut(key).unwrap_or_else(|| panic!("present"));
        assert!(defeat_boss_on_reject(stats, true));
        // Afterwards is_boss is false and the next sender moves into the top 5.
        assert!(!is_boss(key, &all, &t));
        assert!(is_boss("s5@example.com", &all, &t));
        // A non-boss reject does not defeat anything.
        let other = all
            .get_mut("s5@example.com")
            .unwrap_or_else(|| panic!("present"));
        assert!(!defeat_boss_on_reject(other, false));
        assert!(!other.boss_defeated);
    }
}
