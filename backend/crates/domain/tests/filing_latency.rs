//! FL-01 AC3: the server suggestion stays at p95 100 ms or less with a large
//! user state (5,000 senders, 200 categories).
//!
//! `std::time::Instant` is allowed here, in the test, and nowhere in `domain`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use domain::filing::{suggest, FilingInput};
use domain::user_state::Category;
use domain::{CategoryId, MessageClass, SenderStats};
use time::OffsetDateTime;
use uuid::Uuid;

const SENDERS: usize = 5_000;
const CATEGORIES: usize = 200;
const CALLS: usize = 1_000;
const BUDGET: Duration = Duration::from_millis(100);

#[test]
fn fl_01_ac3_suggest_p95_under_100ms() {
    let categories: Vec<Category> = (0..CATEGORIES)
        .map(|n| Category {
            category_id: CategoryId::new(Uuid::from_u128(u128::try_from(n).unwrap_or(0))),
            name: format!("Category {n}"),
            labels: BTreeMap::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        })
        .collect();

    let mut sender_stats: BTreeMap<String, SenderStats> = BTreeMap::new();
    for n in 0..SENDERS {
        let mut stats = SenderStats::default();
        // Three files each, across ten domains, so the domain tier has work to
        // do too.
        for offset in 0..3usize {
            let category = (n + offset) % CATEGORIES;
            stats
                .files
                .insert(Uuid::from_u128(u128::try_from(category).unwrap_or(0)), 2);
        }
        let last = n % CATEGORIES;
        stats.last_filed = Some(CategoryId::new(Uuid::from_u128(
            u128::try_from(last).unwrap_or(0),
        )));
        sender_stats.insert(format!("sender{n}@d{}.test", n % 10), stats);
    }

    let input = FilingInput {
        sender_key: "sender0@d0.test",
        sender_domain: "d0.test",
        class: MessageClass::List,
        categories: &categories,
        sender_stats: &sender_stats,
    };

    // Warm up, then time every call.
    let _ = suggest(&input);
    let mut samples: Vec<Duration> = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let started = Instant::now();
        let suggestion = suggest(&input);
        samples.push(started.elapsed());
        assert!(suggestion.category.is_some());
    }

    samples.sort_unstable();
    let index = samples.len() * 95 / 100;
    let p95 = samples[index.saturating_sub(1).min(samples.len() - 1)];
    assert!(p95 <= BUDGET, "p95 {p95:?} exceeds the {BUDGET:?} budget");
}
