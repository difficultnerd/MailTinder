//! T-602b domain tests: the user state file schema.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;

use domain::user_state::{
    AchievementRecord, Category, HistoryAction, HistoryEntry, HistoryOutcome, MailboxPosition,
    PendingDeliveryCheck, PendingUnsubscribe, RecentSwipe, SkipReturn, SkipState, StoredRule,
    Totals, UserState, HISTORY_RETENTION_DAYS, RECENT_SWIPES_MAX,
};
use domain::{
    CategoryId, MailboxId, RuleId, RuleKind, RuleMatch, SenderKey, SenderStats, SortRule,
};
use proptest::prelude::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(secs).unwrap()
}

fn entry(n: u128, when: OffsetDateTime) -> HistoryEntry {
    HistoryEntry {
        entry_id: Uuid::from_u128(n),
        at: when,
        mailbox_id: MailboxId(Uuid::from_u128(1)),
        sender_display: "Sender".into(),
        action: HistoryAction::Filed,
        outcome: HistoryOutcome::Done,
        rule_id: None,
    }
}

#[test]
fn gm_06_ac2_achievement_record_has_two_fields() {
    let record = AchievementRecord {
        achievement_id: "first_swipe".into(),
        unlocked_at: at(1_700_000_000),
    };
    let json = serde_json::to_value(&record).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["achievement_id", "unlocked_at"]);
}

#[test]
fn user_state_history_trimmed_at_twelve_months() {
    let now = at(1_800_000_000);
    let mut state = UserState::default();
    let old = now - Duration::days(HISTORY_RETENTION_DAYS + 1);
    let edge = now - Duration::days(HISTORY_RETENTION_DAYS);
    let recent = now - Duration::days(10);
    state.push_history(entry(1, old));
    state.push_history(entry(2, edge));
    state.push_history(entry(3, recent));
    state.trim(now);
    let ids: Vec<u128> = state.history.iter().map(|e| e.entry_id.as_u128()).collect();
    assert_eq!(ids, [2, 3]);
}

#[test]
fn user_state_push_history_is_idempotent() {
    let mut state = UserState::default();
    assert!(state.push_history(entry(7, at(1_700_000_000))));
    assert!(!state.push_history(entry(7, at(1_700_000_500))));
    assert_eq!(state.history.len(), 1);
}

#[test]
fn user_state_trim_keeps_newest_recent_swipes() {
    let mut state = UserState::default();
    for i in 0..(RECENT_SWIPES_MAX as i64 + 5) {
        state.recent_swipes.push(RecentSwipe {
            swipe_id: Uuid::from_u128(i as u128),
            at: at(1_700_000_000 + i),
            result_json: "{}".into(),
        });
    }
    state.trim(at(1_700_000_100));
    assert_eq!(state.recent_swipes.len(), RECENT_SWIPES_MAX);
    assert_eq!(state.recent_swipes[0].swipe_id, Uuid::from_u128(5));
}

#[test]
fn user_state_category_by_name_is_case_and_space_insensitive() {
    let mut state = UserState::default();
    let id = CategoryId(Uuid::from_u128(9));
    state.categories.push(Category {
        category_id: id,
        name: " Receipts ".into(),
        labels: BTreeMap::new(),
        created_at: at(1_700_000_000),
    });
    assert_eq!(state.category_by_name("receipts").unwrap().category_id, id);
    assert!(state.category(&id).is_some());
    assert!(state.category_by_name("other").is_none());
}

fn arb_time() -> impl Strategy<Value = OffsetDateTime> {
    (0i64..4_000_000_000).prop_map(at)
}

fn arb_uuid() -> impl Strategy<Value = Uuid> {
    any::<u128>().prop_map(Uuid::from_u128)
}

fn arb_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ._@-]{0,20}"
}

fn arb_rule() -> impl Strategy<Value = StoredRule> {
    (
        arb_uuid(),
        prop_oneof![
            Just(RuleKind::RejectList),
            Just(RuleKind::BlockPerson),
            Just(RuleKind::File)
        ],
        arb_text(),
        proptest::option::of(arb_text()),
        proptest::option::of(arb_text()),
        proptest::option::of(arb_uuid()),
        any::<bool>(),
        arb_time(),
        proptest::option::of(arb_uuid()),
        (any::<u64>(), proptest::option::of(any::<u32>())),
    )
        .prop_map(
            |(
                id,
                kind,
                sender,
                list_id,
                feedback_id,
                cat,
                enabled,
                created_at,
                swipe,
                (n, rate),
            )| {
                StoredRule {
                    rule: SortRule {
                        rule_id: RuleId(id),
                        kind,
                        matcher: RuleMatch {
                            sender: SenderKey::from_address(&sender),
                            list_id,
                            feedback_id,
                        },
                        category: cat.map(CategoryId),
                        enabled,
                        created_at,
                        source_swipe: swipe,
                    },
                    times_applied: n,
                    yearly_rate: rate,
                }
            },
        )
}

fn arb_category() -> impl Strategy<Value = Category> {
    (
        arb_uuid(),
        arb_text(),
        proptest::collection::btree_map(arb_uuid(), arb_text(), 0..3),
        arb_time(),
    )
        .prop_map(|(id, name, labels, created_at)| Category {
            category_id: CategoryId(id),
            name,
            labels,
            created_at,
        })
}

fn arb_stats() -> impl Strategy<Value = SenderStats> {
    (
        arb_text(),
        (any::<u32>(), any::<u32>()),
        proptest::collection::vec(arb_time(), 0..3),
        proptest::collection::btree_map(arb_uuid(), any::<u32>(), 0..3),
        proptest::option::of(arb_uuid()),
        (
            proptest::option::of(arb_time()),
            proptest::option::of(arb_time()),
            any::<bool>(),
        ),
    )
        .prop_map(
            |(display, (seen, keeps), rejects_counted, files, last_filed, (ls, bp, boss))| {
                SenderStats {
                    display,
                    seen,
                    keeps,
                    rejects_counted,
                    files,
                    last_filed: last_filed.map(CategoryId),
                    last_seen: ls,
                    block_prompt_declined_until: bp,
                    boss_defeated: boss,
                }
            },
        )
}

fn arb_history() -> impl Strategy<Value = HistoryEntry> {
    (
        arb_uuid(),
        arb_time(),
        arb_uuid(),
        arb_text(),
        prop_oneof![
            Just(HistoryAction::TrashedByRule),
            Just(HistoryAction::Unsubscribe),
            Just(HistoryAction::Filed),
            Just(HistoryAction::FiledByRule),
            Just(HistoryAction::Blocked),
            Just(HistoryAction::ReportedSpam)
        ],
        prop_oneof![
            Just(HistoryOutcome::Sent),
            Just(HistoryOutcome::NeedsAttention),
            Just(HistoryOutcome::Failed),
            Just(HistoryOutcome::Cancelled),
            Just(HistoryOutcome::Expired),
            Just(HistoryOutcome::Done)
        ],
        proptest::option::of(arb_uuid()),
    )
        .prop_map(|(e, t, m, s, action, outcome, r)| HistoryEntry {
            entry_id: e,
            at: t,
            mailbox_id: MailboxId(m),
            sender_display: s,
            action,
            outcome,
            rule_id: r.map(RuleId),
        })
}

fn arb_position() -> impl Strategy<Value = MailboxPosition> {
    (
        proptest::option::of(arb_time()),
        proptest::option::of(arb_time()),
        proptest::option::of(arb_time()),
        any::<bool>(),
        proptest::option::of(arb_time()),
        proptest::collection::vec(arb_text(), 0..3),
        proptest::option::of(arb_uuid()),
    )
        .prop_map(|(a, b, c, d, e, f, g)| MailboxPosition {
            newest_seen: a,
            new_floor: b,
            new_ceiling: c,
            new_done: d,
            backlog_ceiling: e,
            boundary_ids: f,
            session_record_id: g,
        })
}

fn arb_skips() -> impl Strategy<Value = SkipState> {
    (
        proptest::option::of(arb_uuid()),
        proptest::collection::btree_map(arb_text(), any::<u8>(), 0..3),
        proptest::collection::vec(
            (arb_uuid(), arb_text(), any::<u32>()).prop_map(|(m, id, n)| SkipReturn {
                mailbox_id: MailboxId(m),
                message_id: id,
                after_cards: n,
            }),
            0..3,
        ),
    )
        .prop_map(|(session_record_id, counts, queue)| SkipState {
            session_record_id,
            counts,
            queue,
        })
}

fn arb_pending_unsub() -> impl Strategy<Value = PendingUnsubscribe> {
    (
        arb_uuid(),
        arb_text(),
        arb_text(),
        proptest::option::of(arb_text()),
        proptest::option::of(arb_uuid()),
        arb_time(),
    )
        .prop_map(|(m, d, k, l, r, t)| PendingUnsubscribe {
            mailbox_id: MailboxId(m),
            sender_display: d,
            sender_key: k,
            list_id: l,
            rule_id: r.map(RuleId),
            created_at: t,
        })
}

fn arb_totals() -> impl Strategy<Value = Totals> {
    (
        proptest::collection::vec(any::<u64>(), 10),
        proptest::option::of(arb_uuid()),
    )
        .prop_map(|(v, round_session)| Totals {
            triaged: v[0],
            cleared: v[1],
            senders_unsubscribed: v[2],
            unsubscribes_confirmed: v[3],
            unsubscribes_queued: v[4],
            senders_silenced: v[5],
            years_cleared: v[6],
            levels_cleared: Vec::new(),
            categories_created: v[7],
            people_blocked: v[8],
            round_unsubscribes: v[9],
            round_session,
        })
}

fn arb_state() -> impl Strategy<Value = UserState> {
    let part1 = (
        any::<u32>(),
        proptest::collection::vec(arb_rule(), 0..3),
        proptest::collection::vec(arb_category(), 0..3),
        proptest::collection::btree_map(arb_text(), arb_stats(), 0..3),
        proptest::collection::vec(arb_history(), 0..4),
        proptest::collection::btree_map(arb_uuid(), arb_position(), 0..3),
    );
    let part2 = (
        arb_skips(),
        proptest::collection::vec(
            (arb_uuid(), arb_time(), arb_text()).prop_map(|(swipe_id, at, result_json)| {
                RecentSwipe {
                    swipe_id,
                    at,
                    result_json,
                }
            }),
            0..3,
        ),
        proptest::collection::btree_map(arb_uuid(), arb_pending_unsub(), 0..3),
        proptest::collection::vec(
            (
                arb_text(),
                proptest::option::of(arb_text()),
                arb_uuid(),
                arb_time(),
            )
                .prop_map(|(sender_key, list_id, m, unsubscribed_at)| {
                    PendingDeliveryCheck {
                        sender_key,
                        list_id,
                        mailbox_id: MailboxId(m),
                        unsubscribed_at,
                        mail_seen: false,
                        confirm_counted: false,
                        pending_ignored_display: None,
                    }
                }),
            0..3,
        ),
        proptest::collection::vec(
            (arb_text(), arb_time()).prop_map(|(achievement_id, unlocked_at)| AchievementRecord {
                achievement_id,
                unlocked_at,
            }),
            0..3,
        ),
        arb_totals(),
    );
    (part1, part2).prop_map(
        |(
            (version, rules, categories, sender_stats, history, positions),
            (
                skips,
                recent_swipes,
                pending_unsubscribes,
                pending_delivery_checks,
                achievements,
                totals,
            ),
        )| UserState {
            version,
            rules,
            categories,
            sender_stats,
            history,
            positions,
            skips,
            recent_swipes,
            pending_unsubscribes,
            pending_delivery_checks,
            achievements,
            totals,
        },
    )
}

proptest! {
    #[test]
    fn user_state_round_trip(state in arb_state()) {
        let json = serde_json::to_vec(&state).unwrap();
        let back: UserState = serde_json::from_slice(&json).unwrap();
        prop_assert_eq!(back, state);
    }
}
