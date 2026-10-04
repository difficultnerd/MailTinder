//! Tests for undo (T-105b).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use domain::{
    plan_swipe, plan_undo, reverse_stats, undo_response, Classification, HeaderFacts,
    JobCancelOutcome, JobId, LabelSet, MailboxChange, MailboxId, MessageClass, MessageId,
    MessageMeta, RuleId, SenderKey, SenderStats, SwipeAction, SwipeIds, SwipeInput, SwipeOutcome,
    SwipeRecord, Tunables, UndoStack,
};
use proptest::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

fn mid() -> MailboxId {
    MailboxId(Uuid::new_v4())
}
fn ids() -> SwipeIds {
    SwipeIds {
        job_id: JobId(Uuid::new_v4()),
        rule_id: RuleId(Uuid::new_v4()),
    }
}

fn meta(labels: LabelSet) -> MessageMeta {
    meta_with_id("m1", labels)
}

fn meta_with_id(id: &str, labels: LabelSet) -> MessageMeta {
    MessageMeta {
        mailbox: mid(),
        id: MessageId::new(id).unwrap_or_else(|_| panic!("id")),
        internal_date: T0,
        from_display: String::new(),
        from_address: "a@example.com".to_owned(),
        sender: SenderKey::from_address("a@example.com"),
        subject: String::new(),
        labels,
        facts: HeaderFacts {
            list_unsubscribe: None,
            list_unsubscribe_present: false,
            list_id: None,
            feedback_id: None,
            precedence_bulk: false,
            auto_submitted: false,
            from_authenticated: true,
            esp_hint: None,
            is_reply_or_thread: false,
            reply_to_mismatch: false,
            display_name_spoof: false,
        },
    }
}

fn plan(action: SwipeAction, m: &MessageMeta, stats: &SenderStats) -> domain::SwipePlan {
    let badge = Classification {
        class: MessageClass::List,
        bulk_score: 0,
        bulk_reason: String::new(),
        confidence: None,
        probabilities: None,
    };
    plan_swipe(&SwipeInput {
        action,
        meta: m,
        badge: &badge,
        stats,
        ids: ids(),
        source_swipe: Uuid::new_v4(),
        now: T0,
        tunables: &Tunables::default(),
    })
}

fn record(plan: &domain::SwipePlan, m: &MessageMeta, action: SwipeAction) -> SwipeRecord {
    SwipeRecord::from_plan(plan, m, action, T0)
}

#[test]
fn sw_05_ac1_undo_restores_exact_previous_labels() {
    let labels = LabelSet::from_ids(vec!["INBOX".to_owned(), "UNREAD".to_owned()]);
    let m = meta(labels.clone());
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    let rec = record(&p, &m, SwipeAction::Reject);
    let undo = plan_undo(&rec);
    assert_eq!(undo.restore_labels, Some(labels));
}

#[test]
fn sw_05_ac2_undo_cancels_job_and_removes_rule() {
    // A list reject with a covered header queues a job and creates a rule.
    let f = HeaderFacts {
        list_unsubscribe: Some(domain::UnsubscribeOptions {
            one_click_https: Some(
                url::Url::parse("https://unsub.example.com/one").unwrap_or_else(|_| panic!("url")),
            ),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some("list-1".to_owned()),
        feedback_id: None,
        precedence_bulk: false,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    };
    let m = MessageMeta {
        mailbox: mid(),
        id: MessageId::new("m1").unwrap_or_else(|_| panic!("id")),
        internal_date: T0,
        from_display: String::new(),
        from_address: "a@example.com".to_owned(),
        sender: SenderKey::from_address("a@example.com"),
        subject: String::new(),
        labels: LabelSet::from_ids(vec!["INBOX".to_owned()]),
        facts: f,
    };
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    let rec = record(&p, &m, SwipeAction::Reject);
    let undo = plan_undo(&rec);
    assert!(undo.cancel_job.is_some());
    assert!(undo.remove_rule.is_some());
}

#[test]
fn sw_05_ac3_undo_after_send_reports_already_sent() {
    let resp = undo_response(JobCancelOutcome::AlreadySent);
    assert!(resp.restored);
    assert!(resp.unsubscribe_already_sent);
    let resp2 = undo_response(JobCancelOutcome::Cancelled);
    assert!(resp2.restored);
    assert!(!resp2.unsubscribe_already_sent);
}

#[test]
fn sw_05_ac3_rule_removed_even_when_sent() {
    // The rule is removed whether or not the job already ran.
    let rec = SwipeRecord {
        mailbox: mid(),
        message: MessageId::new("m1").unwrap_or_else(|_| panic!("id")),
        sender: SenderKey::from_address("a@example.com"),
        action: SwipeAction::Reject,
        outcome: SwipeOutcome::TrashedUnsubscribeQueued,
        previous_labels: LabelSet::from_ids(vec!["INBOX".to_owned()]),
        job_id: Some(JobId(Uuid::new_v4())),
        rule_id: Some(RuleId(Uuid::new_v4())),
        counted_reject_at: None,
        at: T0,
    };
    let undo = plan_undo(&rec);
    assert!(undo.remove_rule.is_some());
    assert!(undo.cancel_job.is_some());
}

#[test]
fn sw_05_ac4a_spam_undo_says_not_recalled() {
    let rec = SwipeRecord {
        mailbox: mid(),
        message: MessageId::new("m1").unwrap_or_else(|_| panic!("id")),
        sender: SenderKey::from_address("a@example.com"),
        action: SwipeAction::Reject,
        outcome: SwipeOutcome::ReportedSpam,
        previous_labels: LabelSet::from_ids(vec!["INBOX".to_owned()]),
        job_id: None,
        rule_id: None,
        counted_reject_at: None,
        at: T0,
    };
    let undo = plan_undo(&rec);
    assert!(undo.spam_report_not_recalled);
    assert!(undo.restore_labels.is_some());
}

#[test]
fn undo_keep_and_skip_restore_nothing_in_mailbox() {
    let m = meta(LabelSet::from_ids(vec!["INBOX".to_owned()]));
    let stats = SenderStats::default();
    let p_keep = plan(SwipeAction::Keep, &m, &stats);
    let rec_keep = record(&p_keep, &m, SwipeAction::Keep);
    assert!(plan_undo(&rec_keep).restore_labels.is_none());

    let p_skip = plan(SwipeAction::Skip, &m, &stats);
    let rec_skip = record(&p_skip, &m, SwipeAction::Skip);
    assert!(plan_undo(&rec_skip).restore_labels.is_none());
}

#[test]
fn undo_reverses_counted_reject_only_once() {
    let m = meta(LabelSet::from_ids(vec!["INBOX".to_owned()]));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    // The reject counted (personal message, authenticated).
    let rec = record(&p, &m, SwipeAction::Reject);
    assert!(rec.counted_reject_at.is_some());
    // Apply the plan's stats, then reverse.
    let mut after = p.stats_after.clone();
    reverse_stats(&rec, &mut after);
    assert_eq!(after.rejects_counted.len(), 0);
}

#[test]
fn undo_stack_lifo() {
    let mut stack = UndoStack::new();
    assert!(stack.is_empty());
    stack.push(1);
    stack.push(2);
    stack.push(3);
    assert_eq!(stack.len(), 3);
    assert_eq!(stack.pop(), Some(3));
    assert_eq!(stack.pop(), Some(2));
    assert_eq!(stack.pop(), Some(1));
    assert_eq!(stack.pop(), None);
    assert!(stack.is_empty());
}

proptest! {
    /// SW-05 AC4: the stack pops in reverse push order and is empty after N pops.
    #[test]
    fn sw_05_ac4_undo_walks_back_every_swipe(items in proptest::collection::vec(0u32..100, 0..30)) {
        let mut stack = UndoStack::new();
        for i in &items {
            stack.push(*i);
        }
        let mut popped = Vec::new();
        while let Some(v) = stack.pop() {
            popped.push(v);
        }
        let mut expected = items.clone();
        expected.reverse();
        prop_assert_eq!(popped, expected);
        prop_assert!(stack.is_empty());
    }
}

// --- INV-6 property harness ---

/// A test-local model of a mailbox: message ID -> label set.
type Mailbox = BTreeMap<MessageId, LabelSet>;

fn apply_change(mb: &mut Mailbox, id: &MessageId, change: &MailboxChange) {
    let labels = mb.entry(id.clone()).or_default();
    match change {
        MailboxChange::None => {}
        MailboxChange::Trash => {
            labels.insert("TRASH".to_owned());
            labels.remove("INBOX");
        }
        MailboxChange::ReportSpamAndTrash => {
            labels.insert("SPAM".to_owned());
            labels.insert("TRASH".to_owned());
            labels.remove("INBOX");
        }
        MailboxChange::ApplyCategory { category } => {
            labels.insert(format!("CAT-{}", category.0));
            labels.remove("INBOX");
        }
    }
}

fn restore_labels(mb: &mut Mailbox, id: &MessageId, labels: &LabelSet) {
    mb.insert(id.clone(), labels.clone());
}

proptest! {
    /// INV-6: every swipe in a session can be reversed, restoring exact label
    /// sets and stats.
    #[test]
    fn inv_6_every_swipe_reversible(
        n_swipes in 1u32..30,
        n_messages in 1u32..5,
        actions in proptest::collection::vec(
            prop::sample::select([
                SwipeAction::Keep, SwipeAction::Skip, SwipeAction::Reject,
            ].as_slice()),
            1..30,
        ),
    ) {
        // Build a starting mailbox: each message starts with INBOX.
        let mut mb: Mailbox = BTreeMap::new();
        for i in 0..n_messages {
            let id = MessageId::new(format!("m{i}")).unwrap();
            mb.insert(id, LabelSet::from_ids(vec!["INBOX".to_owned()]));
        }
        let start = mb.clone();
        let mut stats = SenderStats::default();
        let start_stats = stats.clone();
        let mut stack: UndoStack<SwipeRecord> = UndoStack::new();

        // Apply n_swipes (cycling through the messages and actions).
        for i in 0..n_swipes {
            let id = MessageId::new(format!("m{}", i % n_messages)).unwrap();
            let action = actions[(i as usize) % actions.len()];
            let labels = mb.get(&id).cloned().unwrap_or_default();
            let m = meta_with_id(id.as_str(), labels);
            let p = plan(action, &m, &stats);
            apply_change(&mut mb, &id, &p.change);
            stats = p.stats_after.clone();
            let rec = SwipeRecord::from_plan(&p, &m, action, T0);
            stack.push(rec);
        }

        // Undo everything.
        while let Some(rec) = stack.pop() {
            let undo = plan_undo(&rec);
            if let Some(labels) = undo.restore_labels {
                restore_labels(&mut mb, &rec.message, &labels);
            }
            reverse_stats(&rec, &mut stats);
        }

        prop_assert_eq!(mb, start);
        prop_assert_eq!(stats, start_stats);
    }
}
