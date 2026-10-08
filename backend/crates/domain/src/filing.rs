//! Filing suggestions and the keep prompt (T-607b; FL-01, FL-03, FL-04,
//! SW-04).
//!
//! Everything here is pure and in memory: no provider call, no clock and no
//! randomness, so the Feed can ship `suggestion` and `keep_prompt` on every
//! card inside the FL-01 AC3 budget. A suggestion never files anything by
//! itself (FL-03 AC2): it only tells the app what to offer.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::class::MessageClass;
use crate::ids::CategoryId;
use crate::sender::SenderStats;
use crate::tunables::Tunables;
use crate::user_state::Category;

/// FL-03 AC1 `[TUNABLE]`: confirmations of the same category for one sender
/// that make the suggestion `learned`.
pub const LEARNED_THRESHOLD: u32 = 3;
/// S7 `Suggestion.alternates` `maxItems`.
pub const MAX_ALTERNATES: usize = 2;

/// How sure the server is about a suggestion (S7 `confidence`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// FL-03 AC1: the same category has been confirmed often enough, so the
    /// app shows one-tap confirm with the alternates collapsed.
    Learned,
    /// A best guess from sender, domain or overall use.
    Suggested,
    /// SW-04 AC3: nothing to suggest, so the app goes straight to naming.
    None,
}

impl Confidence {
    /// The wire name (`learned`, `suggested`, `none`).
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Learned => "learned",
            Confidence::Suggested => "suggested",
            Confidence::None => "none",
        }
    }
}

/// What the Feed offers on a card (S7 `Suggestion`).
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The best category, or `None` when the user has no categories.
    pub category: Option<CategoryId>,
    /// The name to show; the header-rules hint when there is no category.
    pub name: Option<String>,
    /// Up to [`MAX_ALTERNATES`] further categories, best first.
    pub alternates: Vec<(CategoryId, String)>,
    pub confidence: Confidence,
}

/// Everything `suggest` reads. All borrowed; nothing is copied out of the
/// user's state beyond the suggestion itself.
pub struct FilingInput<'a> {
    /// The sender key of the card's message.
    pub sender_key: &'a str,
    /// Part after `@` of the sender key.
    pub sender_domain: &'a str,
    /// The header-rules class of the message (FL-01 AC2 fallback).
    pub class: MessageClass,
    pub categories: &'a [Category],
    pub sender_stats: &'a BTreeMap<String, SenderStats>,
}

/// One ranked category while the tiers are built.
#[derive(Clone, Copy)]
struct Ranked<'a> {
    id: CategoryId,
    name: &'a str,
    count: u32,
}

/// The user's categories that still exist (FL-01: a deleted category is never
/// suggested), by ID.
fn live_categories(categories: &[Category]) -> BTreeMap<Uuid, (CategoryId, &str)> {
    let mut live = BTreeMap::new();
    for category in categories {
        live.insert(
            category.category_id.0,
            (category.category_id, category.name.as_str()),
        );
    }
    live
}

/// Deterministic rank: count descending, ties by name ascending, then by ID so
/// two equally named categories still order the same way every time.
fn by_count_then_name(rows: &mut [Ranked<'_>]) {
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.name.cmp(b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// FL-01 AC1: how often this sender's mail was filed in each live category.
fn rank_sender<'a>(
    sender: Option<&SenderStats>,
    live: &BTreeMap<Uuid, (CategoryId, &'a str)>,
    last_filed: Option<CategoryId>,
) -> Vec<Ranked<'a>> {
    let Some(stats) = sender else {
        return Vec::new();
    };
    let mut rows: Vec<Ranked<'a>> = stats
        .files
        .iter()
        .filter_map(|(uuid, count)| {
            live.get(uuid).map(|(id, name)| Ranked {
                id: *id,
                name,
                count: *count,
            })
        })
        .collect();
    // Ties: the category filed last time first, then by name.
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| last_rank(a.id, last_filed).cmp(&last_rank(b.id, last_filed)))
            .then_with(|| a.name.cmp(b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
}

/// `0` for the category filed most recently, `1` for the others.
fn last_rank(id: CategoryId, last_filed: Option<CategoryId>) -> u8 {
    u8::from(last_filed != Some(id))
}

/// FL-01 AC2 tier two: the same sender domain, every other sender.
fn rank_domain<'a>(
    input: &FilingInput<'_>,
    live: &BTreeMap<Uuid, (CategoryId, &'a str)>,
) -> Vec<Ranked<'a>> {
    let suffix = format!("@{}", input.sender_domain);
    let mut sums: BTreeMap<Uuid, u32> = BTreeMap::new();
    for (key, stats) in input.sender_stats {
        if key == input.sender_key || !key.ends_with(suffix.as_str()) {
            continue;
        }
        for (uuid, count) in &stats.files {
            if live.contains_key(uuid) {
                let slot = sums.entry(*uuid).or_insert(0);
                *slot = slot.saturating_add(*count);
            }
        }
    }
    rank_sums(&sums, live)
}

/// FL-01 AC2 tier three: every sender, most-used categories first, categories
/// never used after them by name.
fn rank_overall<'a>(
    input: &FilingInput<'_>,
    live: &BTreeMap<Uuid, (CategoryId, &'a str)>,
) -> Vec<Ranked<'a>> {
    let mut sums: BTreeMap<Uuid, u32> = BTreeMap::new();
    for stats in input.sender_stats.values() {
        for (uuid, count) in &stats.files {
            if live.contains_key(uuid) {
                let slot = sums.entry(*uuid).or_insert(0);
                *slot = slot.saturating_add(*count);
            }
        }
    }
    // A category nobody has used yet still ranks, last, by name.
    for uuid in live.keys() {
        sums.entry(*uuid).or_insert(0);
    }
    rank_sums(&sums, live)
}

fn rank_sums<'a>(
    sums: &BTreeMap<Uuid, u32>,
    live: &BTreeMap<Uuid, (CategoryId, &'a str)>,
) -> Vec<Ranked<'a>> {
    let mut rows: Vec<Ranked<'a>> = sums
        .iter()
        .filter_map(|(uuid, count)| {
            live.get(uuid).map(|(id, name)| Ranked {
                id: *id,
                name,
                count: *count,
            })
        })
        .collect();
    by_count_then_name(&mut rows);
    rows
}

/// The filing suggestion for a card (FL-01, FL-03, SW-04 AC1, SW-04 AC3).
pub fn suggest(input: &FilingInput<'_>) -> Suggestion {
    let live = live_categories(input.categories);
    if live.is_empty() {
        // SW-04 AC3: nothing to suggest, so the app goes straight to naming.
        return Suggestion {
            category: None,
            name: name_hint(input.class).map(str::to_owned),
            alternates: Vec::new(),
            confidence: Confidence::None,
        };
    }

    let sender = input.sender_stats.get(input.sender_key);
    let last_filed = sender.and_then(|stats| stats.last_filed);
    let tier1 = rank_sender(sender, &live, last_filed);
    let tier2 = rank_domain(input, &live);
    let tier3 = rank_overall(input, &live);

    // Tier one first, then the tiers' new entries in order.
    let mut ranked: Vec<Ranked<'_>> = Vec::new();
    let mut seen: BTreeSet<CategoryId> = BTreeSet::new();
    for tier in [&tier1, &tier2, &tier3] {
        for entry in tier {
            if seen.insert(entry.id) {
                ranked.push(*entry);
            }
        }
    }
    let Some(primary) = ranked.first() else {
        return Suggestion {
            category: None,
            name: name_hint(input.class).map(str::to_owned),
            alternates: Vec::new(),
            confidence: Confidence::None,
        };
    };

    // FL-03 AC1: learned needs a tier-one primary confirmed often enough that
    // is also the category filed last time.
    let learned = tier1.first().is_some_and(|top| top.id == primary.id)
        && primary.count >= LEARNED_THRESHOLD
        && last_filed == Some(primary.id);
    let confidence = if learned {
        Confidence::Learned
    } else {
        Confidence::Suggested
    };
    Suggestion {
        category: Some(primary.id),
        name: Some(primary.name.to_owned()),
        alternates: ranked
            .iter()
            .skip(1)
            .take(MAX_ALTERNATES)
            .map(|entry| (entry.id, entry.name.to_owned()))
            .collect(),
        confidence,
    }
}

/// FL-04 AC1: `Some((category, name))` when keep-learning applies and a
/// category can be named.
pub fn keep_prompt(
    input: &FilingInput<'_>,
    has_file_rule: bool,
    t: &Tunables,
) -> Option<(CategoryId, String)> {
    let stats = input.sender_stats.get(input.sender_key)?;
    if !stats.keep_prompt_due(has_file_rule, t) {
        return None;
    }
    let suggestion = suggest(input);
    match (suggestion.category, suggestion.name) {
        (Some(category), Some(name)) => Some((category, name)),
        _ => None,
    }
}

/// Header-rules name hint when the user has no categories (FL-01 AC2
/// `[DEFAULT]` names).
pub fn name_hint(class: MessageClass) -> Option<&'static str> {
    match class {
        MessageClass::List => Some("Newsletters"),
        MessageClass::BulkNoHeader => Some("Updates"),
        MessageClass::Notice => Some("Accounts"),
        MessageClass::Personal => Some("People"),
        MessageClass::Suspect => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn category(n: u128, name: &str) -> Category {
        Category {
            category_id: CategoryId::new(Uuid::from_u128(n)),
            name: name.to_owned(),
            labels: BTreeMap::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    /// A sender that filed `files` (ID, count) and last filed `last`.
    fn filed(files: &[(u128, u32)], last: Option<u128>) -> SenderStats {
        let mut stats = SenderStats::default();
        for (id, count) in files {
            stats.files.insert(Uuid::from_u128(*id), *count);
        }
        stats.last_filed = last.map(|id| CategoryId::new(Uuid::from_u128(id)));
        stats
    }

    fn feed_input<'a>(
        sender_key: &'a str,
        domain: &'a str,
        class: MessageClass,
        categories: &'a [Category],
        sender_stats: &'a BTreeMap<String, SenderStats>,
    ) -> FilingInput<'a> {
        FilingInput {
            sender_key,
            sender_domain: domain,
            class,
            categories,
            sender_stats,
        }
    }

    #[test]
    fn fl_01_ac1_sender_history_wins() {
        let categories = vec![category(1, "Bills"), category(2, "Azalea")];
        let mut stats = BTreeMap::new();
        stats.insert(
            "a@example.com".to_owned(),
            filed(&[(1, 1), (2, 5)], Some(2)),
        );
        // Another sender used "Bills" far more, which must not outrank the
        // sender's own history.
        stats.insert("b@example.com".to_owned(), filed(&[(1, 9)], Some(1)));
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &stats,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(2))));
        assert_eq!(s.name.as_deref(), Some("Azalea"));
        // Five files with the last one here: FL-03 AC1 makes this `learned`.
        assert_eq!(s.confidence, Confidence::Learned);
        assert_eq!(
            s.alternates,
            vec![(CategoryId::new(Uuid::from_u128(1)), "Bills".to_owned())]
        );

        // Equal counts: the category filed last time comes first.
        let mut tie = BTreeMap::new();
        tie.insert(
            "a@example.com".to_owned(),
            filed(&[(1, 2), (2, 2)], Some(2)),
        );
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &tie,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(2))));
    }

    #[test]
    fn fl_01_ac2_domain_then_overall_then_hint() {
        let categories = vec![category(1, "Bills"), category(2, "Newsletters")];

        // No sender history: the sender domain decides.
        let mut domain_stats = BTreeMap::new();
        domain_stats.insert("other@example.com".to_owned(), filed(&[(1, 4)], Some(1)));
        let s = suggest(&feed_input(
            "new@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &domain_stats,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(1))));
        assert_eq!(s.confidence, Confidence::Suggested);

        // No sender and no domain history: overall use decides.
        let mut overall = BTreeMap::new();
        overall.insert("elsewhere@other.test".to_owned(), filed(&[(2, 3)], Some(2)));
        let s = suggest(&feed_input(
            "new@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &overall,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(2))));
        assert_eq!(s.name.as_deref(), Some("Newsletters"));

        // Nothing used at all: the first category by name, then the hint when
        // there is no category to name.
        let empty = BTreeMap::new();
        let s = suggest(&feed_input(
            "new@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &empty,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(1))));

        let s = suggest(&feed_input(
            "new@example.com",
            "example.com",
            MessageClass::BulkNoHeader,
            &[],
            &empty,
        ));
        assert_eq!(s.category, None);
        assert_eq!(s.name.as_deref(), Some("Updates"));
        assert_eq!(s.confidence, Confidence::None);
    }

    #[test]
    fn fl_03_ac1_learned_after_three() {
        let categories = vec![category(1, "Bills"), category(2, "Newsletters")];
        let make = |count: u32, last: Option<u128>| {
            let mut stats = BTreeMap::new();
            stats.insert("a@example.com".to_owned(), filed(&[(1, count)], last));
            stats
        };
        let two = make(2, Some(1));
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &two,
        ));
        assert_eq!(s.confidence, Confidence::Suggested);

        let three = make(3, Some(1));
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &three,
        ));
        assert_eq!(s.confidence, Confidence::Learned);

        // Three confirmations, but the last file went elsewhere: not learned.
        let elsewhere = make(3, Some(2));
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &elsewhere,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(1))));
        assert_eq!(s.confidence, Confidence::Suggested);

        // A category deleted since the files were recorded is never suggested.
        let deleted = make(3, Some(1));
        let s = suggest(&feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories[1..],
            &deleted,
        ));
        assert_eq!(s.category, Some(CategoryId::new(Uuid::from_u128(2))));
        assert_eq!(s.confidence, Confidence::Suggested);
    }

    #[test]
    fn fl_04_ac1_keep_prompt_after_five_keeps() {
        let t = Tunables::default();
        let categories = vec![category(1, "Bills")];
        let mut stats = BTreeMap::new();
        let mut sender = filed(&[(1, 2)], Some(1));
        for _ in 0..5 {
            sender.record_keep();
        }
        stats.insert("a@example.com".to_owned(), sender);
        let input = feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &stats,
        );
        assert_eq!(
            keep_prompt(&input, false, &t),
            Some((CategoryId::new(Uuid::from_u128(1)), "Bills".to_owned()))
        );
        // A filing rule for the sender means there is nothing left to teach.
        assert_eq!(keep_prompt(&input, true, &t), None);
    }

    #[test]
    fn fl_04_ac1_no_keep_prompt_after_reject() {
        let t = Tunables::default();
        let categories = vec![category(1, "Bills")];
        let mut stats = BTreeMap::new();
        let mut sender = filed(&[(1, 2)], Some(1));
        for _ in 0..5 {
            sender.record_keep();
        }
        sender.rejects_counted.push(OffsetDateTime::UNIX_EPOCH);
        stats.insert("a@example.com".to_owned(), sender);
        let input = feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &stats,
        );
        assert_eq!(keep_prompt(&input, false, &t), None);

        // Four keeps is below the threshold.
        let mut stats = BTreeMap::new();
        let mut sender = filed(&[(1, 2)], Some(1));
        for _ in 0..4 {
            sender.record_keep();
        }
        stats.insert("a@example.com".to_owned(), sender);
        let input = feed_input(
            "a@example.com",
            "example.com",
            MessageClass::List,
            &categories,
            &stats,
        );
        assert_eq!(keep_prompt(&input, false, &t), None);

        // No categories to name: no prompt, though keep-learning is due.
        let empty_stats = BTreeMap::new();
        assert_eq!(
            keep_prompt(
                &feed_input(
                    "a@example.com",
                    "example.com",
                    MessageClass::List,
                    &[],
                    &empty_stats
                ),
                false,
                &t
            ),
            None
        );
    }

    proptest! {
        /// SW-04 AC1: at most two alternates, always distinct from the primary.
        #[test]
        fn sw_04_ac1_at_most_two_alternates(
            counts in prop::collection::vec(0u32..5, 1..6),
            last in prop::option::of(0usize..6),
            unknown in prop::option::of(0u128..3),
        ) {
            let categories: Vec<Category> = (0..6u128)
                .map(|n| category(n, &format!("Category {n}")))
                .collect();
            let mut files: Vec<(u128, u32)> = Vec::new();
            for (n, count) in counts.iter().enumerate() {
                if *count > 0 {
                    files.push((u128::try_from(n).unwrap_or(0), *count));
                }
            }
            let mut stats = BTreeMap::new();
            stats.insert(
                "a@example.com".to_owned(),
                filed(&files, last.map(|n| u128::try_from(n).unwrap_or(0))),
            );
            // A sender whose files name a category that no longer exists.
            if let Some(gone) = unknown {
                let mut other = SenderStats::default();
                other.files.insert(Uuid::from_u128(1000 + gone), 7);
                stats.insert("b@example.com".to_owned(), other);
            }
            let s = suggest(&feed_input(
                "a@example.com",
                "example.com",
                MessageClass::Personal,
                &categories,
                &stats,
            ));
            prop_assert!(s.alternates.len() <= MAX_ALTERNATES);
            prop_assert!(s
                .alternates
                .iter()
                .all(|(id, _)| Some(*id) != s.category));
            prop_assert!(s.alternates.iter().all(|(id, _)| categories
                .iter()
                .any(|c| c.category_id == *id)));
        }

        /// SW-04 AC3: with no categories the confidence is `none`.
        #[test]
        fn sw_04_ac3_no_categories_confidence_none(class in 0usize..5) {
            let class = MessageClass::ALL[class];
            let stats = BTreeMap::new();
            let s = suggest(&feed_input("a@example.com", "example.com", class, &[], &stats));
            prop_assert_eq!(s.confidence, Confidence::None);
            prop_assert_eq!(s.category, None);
            prop_assert!(s.alternates.is_empty());
            prop_assert_eq!(s.name.as_deref(), name_hint(class));
        }
    }
}
