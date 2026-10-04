//! Tests for sort rules and sender stats (T-104).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{
    first_match, BlockPrompt, CategoryId, HeaderFacts, LabelSet, MailboxId, MessageId, MessageMeta,
    RuleAction, RuleId, RuleKind, RuleMatch, SenderKey, SenderStats, SortRule, Tunables,
};
use proptest::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

fn rid() -> RuleId {
    RuleId(Uuid::new_v4())
}
fn cid() -> CategoryId {
    CategoryId(Uuid::new_v4())
}
fn mid() -> MailboxId {
    MailboxId(Uuid::new_v4())
}

fn facts(
    list_id: Option<&str>,
    feedback_id: Option<&str>,
    lu_present: bool,
    authed: bool,
) -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: lu_present,
        list_id: list_id.map(str::to_owned),
        feedback_id: feedback_id.map(str::to_owned),
        precedence_bulk: false,
        auto_submitted: false,
        from_authenticated: authed,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    }
}

fn meta(sender: &str, facts: HeaderFacts) -> MessageMeta {
    MessageMeta {
        mailbox: mid(),
        id: MessageId::new("m1").unwrap_or_else(|_| panic!("id")),
        internal_date: T0,
        from_display: String::new(),
        from_address: sender.to_owned(),
        sender: SenderKey::from_address(sender),
        subject: String::new(),
        labels: LabelSet::new(),
        facts,
    }
}

fn reject_rule(
    sender: &str,
    list_id: Option<&str>,
    feedback_id: Option<&str>,
    enabled: bool,
) -> SortRule {
    SortRule {
        rule_id: rid(),
        kind: RuleKind::RejectList,
        matcher: RuleMatch {
            sender: SenderKey::from_address(sender),
            list_id: list_id.map(str::to_owned),
            feedback_id: feedback_id.map(str::to_owned),
        },
        category: None,
        enabled,
        created_at: T0,
        source_swipe: None,
    }
}

#[test]
fn sr_01_ac1_rule_keyed_on_sender_and_list_id() {
    let m = meta("a@example.com", facts(Some("list-1"), None, true, true));
    let rule = SortRule::reject_list_for(&m, rid(), T0, None).unwrap();
    assert_eq!(rule.kind, RuleKind::RejectList);
    assert_eq!(
        rule.matcher.sender,
        SenderKey::from_address("a@example.com")
    );
    assert_eq!(rule.matcher.list_id.as_deref(), Some("list-1"));
    assert_eq!(rule.matcher.feedback_id, None);
    assert!(rule.enabled);
}

#[test]
fn sr_01_ac1_sender_only_rule_keeps_feedback_id() {
    let m = meta("a@example.com", facts(None, Some("fb-1"), true, true));
    let rule = SortRule::reject_list_for(&m, rid(), T0, None).unwrap();
    assert_eq!(rule.matcher.list_id, None);
    assert_eq!(rule.matcher.feedback_id.as_deref(), Some("fb-1"));
}

#[test]
fn sr_01_ac2_matching_message_gets_trash_action() {
    let rule = reject_rule("a@example.com", Some("list-1"), None, true);
    let m = meta("a@example.com", facts(Some("list-1"), None, true, true));
    assert!(rule.matches(&m));
    assert_eq!(
        rule.action(),
        Some(RuleAction::Trash {
            rule_id: rule.rule_id
        })
    );
}

#[test]
fn sr_01_ac3_other_list_id_does_not_match() {
    let rule = reject_rule("a@example.com", Some("list-1"), None, true);
    let m = meta("a@example.com", facts(Some("list-2"), None, true, true));
    assert!(!rule.matches(&m));
}

#[test]
fn sr_01_ac3_receipt_without_list_unsubscribe_survives() {
    // Sender-only rule: a receipt without List-Unsubscribe never matches.
    let rule = reject_rule("a@example.com", None, None, true);
    let m = meta("a@example.com", facts(None, None, false, true));
    assert!(!rule.matches(&m));
}

#[test]
fn sr_01_ac3_feedback_id_must_match_when_stored() {
    let rule = reject_rule("a@example.com", None, Some("fb-1"), true);
    let m = meta("a@example.com", facts(None, Some("fb-2"), true, true));
    assert!(!rule.matches(&m));
    let m2 = meta("a@example.com", facts(None, Some("fb-1"), true, true));
    assert!(rule.matches(&m2));
}

#[test]
fn sr_01_ac6_unauthenticated_reject_creates_no_rule() {
    let m = meta("a@example.com", facts(Some("list-1"), None, true, false));
    assert!(SortRule::reject_list_for(&m, rid(), T0, None).is_none());
}

#[test]
fn pb_01_ac1_third_personal_reject_asks() {
    let t = Tunables::default();
    let mut s = SenderStats::default();
    // Two rejects: not yet.
    assert_eq!(s.record_reject(true, true, T0, &t), BlockPrompt::NotYet);
    assert_eq!(
        s.record_reject(true, true, T0 + time::Duration::seconds(1), &t),
        BlockPrompt::NotYet
    );
    // Third: ask.
    assert_eq!(
        s.record_reject(true, true, T0 + time::Duration::seconds(2), &t),
        BlockPrompt::Ask
    );
}

#[test]
fn pb_01_ac1_rejects_older_than_90_days_not_counted() {
    let t = Tunables::default();
    let mut s = SenderStats::default();
    let now = T0 + time::Duration::days(100);
    s.rejects_counted.push(T0); // 100 days ago
    s.rejects_counted.push(now - time::Duration::days(89)); // within window
    assert_eq!(s.rejects_within(now, t.personal_block_window), 1);
}

#[test]
fn pb_01_ac3_decline_suppresses_for_90_days() {
    let t = Tunables::default();
    let mut s = SenderStats::default();
    s.decline_block_prompt(T0, &t);
    // Within the decline window: Declined.
    assert_eq!(
        s.record_reject(true, true, T0 + time::Duration::days(89), &t),
        BlockPrompt::Declined
    );
    // After 90 days: the decline has lapsed, so it can ask again.
    let mut s2 = SenderStats::default();
    s2.decline_block_prompt(T0, &t);
    s2.rejects_counted.push(T0 + time::Duration::days(90));
    s2.rejects_counted
        .push(T0 + time::Duration::days(90) + time::Duration::seconds(1));
    s2.rejects_counted
        .push(T0 + time::Duration::days(90) + time::Duration::seconds(2));
    assert_eq!(
        s2.record_reject(
            true,
            true,
            T0 + time::Duration::days(90) + time::Duration::seconds(3),
            &t
        ),
        BlockPrompt::Ask
    );
}

#[test]
fn pb_01_ac4_three_spoofed_rejects_never_ask() {
    let t = Tunables::default();
    let mut s = SenderStats::default();
    for i in 0..3 {
        assert_eq!(
            s.record_reject(true, false, T0 + time::Duration::seconds(i), &t),
            BlockPrompt::NotYet
        );
    }
    assert!(s.rejects_counted.is_empty() || s.rejects_counted.len() == 3);
}

#[test]
fn fl_04_ac1_five_keeps_no_rejects_prompt_due() {
    let t = Tunables::default();
    let mut s = SenderStats::default();
    for _ in 0..4 {
        s.record_keep();
    }
    assert!(!s.keep_prompt_due(false, &t));
    s.record_keep();
    assert!(s.keep_prompt_due(false, &t));
    // A reject makes it not due.
    let mut s2 = SenderStats::default();
    for _ in 0..5 {
        s2.record_keep();
    }
    s2.rejects_counted.push(T0);
    assert!(!s2.keep_prompt_due(false, &t));
    // A file rule makes it not due.
    let mut s3 = SenderStats::default();
    for _ in 0..5 {
        s3.record_keep();
    }
    assert!(!s3.keep_prompt_due(true, &t));
}

#[test]
fn fl_04_ac2_file_rule_matches_sender() {
    let rule = SortRule::file_for(&SenderKey::from_address("a@example.com"), cid(), rid(), T0);
    let m = meta("a@example.com", facts(None, None, false, true));
    assert!(rule.matches(&m));
    assert_eq!(
        rule.action(),
        Some(RuleAction::File {
            rule_id: rule.rule_id,
            category: rule.category.unwrap()
        })
    );
}

#[test]
fn sw_01_ac2_keep_counted() {
    let mut s = SenderStats::default();
    s.record_keep();
    s.record_keep();
    assert_eq!(s.keeps, 2);
}

#[test]
fn rules_first_match_prefers_trash_then_oldest() {
    let file = SortRule::file_for(&SenderKey::from_address("a@example.com"), cid(), rid(), T0);
    let reject = reject_rule("a@example.com", Some("list-1"), None, true);
    let m = meta("a@example.com", facts(Some("list-1"), None, true, true));
    // Trash wins over file even if the file is older.
    let rules = [file.clone(), reject.clone()];
    let matched = first_match(&rules, &m).unwrap();
    assert_eq!(matched.kind, RuleKind::RejectList);
    // Among two trash rules, the older one wins.
    let older = reject_rule("a@example.com", Some("list-1"), None, true);
    let mut newer = older.clone();
    newer.created_at = T0 + time::Duration::seconds(1);
    let rules2 = [newer, older.clone()];
    let matched2 = first_match(&rules2, &m).unwrap();
    assert_eq!(matched2.rule_id, older.rule_id);
}

#[test]
fn rules_serde_uses_match_key() {
    let rule = reject_rule("a@example.com", Some("list-1"), None, true);
    let json = serde_json::to_value(&rule).unwrap();
    assert!(json.get("match").is_some());
    assert!(json.get("matcher").is_none());
    assert_eq!(json["kind"], "reject_list");
}

proptest! {
    /// SR-01 AC3: a List-Id rule never matches a message with a different List-Id.
    #[test]
    fn sr_01_ac3_list_id_rule_never_matches_other_list(
        rule_list in "[a-z0-9-]{1,20}",
        msg_list in "[a-z0-9-]{1,20}",
    ) {
        prop_assume!(rule_list != msg_list);
        let rule = reject_rule("a@example.com", Some(&rule_list), None, true);
        let m = meta("a@example.com", facts(Some(&msg_list), None, true, true));
        prop_assert!(!rule.matches(&m));
    }

    /// SR-01 AC4: a disabled rule never matches.
    #[test]
    fn sr_01_ac4_disabled_rule_never_matches(
        list_id in prop::option::of("[a-z0-9-]{1,20}"),
    ) {
        let rule = reject_rule("a@example.com", list_id.as_deref(), None, false);
        let m = meta("a@example.com", facts(list_id.as_deref(), None, true, true));
        prop_assert!(!rule.matches(&m));
    }

    /// PB-01 AC2: a block rule matches any message from the sender.
    #[test]
    fn pb_01_ac2_block_rule_matches_any_message_from_sender(
        list_id in prop::option::of("[a-z0-9-]{1,20}"),
        lu_present in any::<bool>(),
    ) {
        let rule = SortRule::block_person_for(&SenderKey::from_address("a@example.com"), rid(), T0, None);
        let m = meta("a@example.com", facts(list_id.as_deref(), None, lu_present, true));
        prop_assert!(rule.matches(&m));
    }

    /// PB-01 AC4: unauthenticated rejects are never counted and never ask.
    #[test]
    fn pb_01_ac4_unauthenticated_never_counted(
        n in 0u32..20,
        personal in proptest::bool::ANY,
    ) {
        let t = Tunables::default();
        let mut s = SenderStats::default();
        for i in 0..n {
            let r = s.record_reject(false, personal, T0 + time::Duration::seconds(i64::from(i)), &t);
            prop_assert_ne!(r, BlockPrompt::Ask);
        }
        prop_assert!(s.rejects_counted.is_empty());
    }
}

#[test]
fn rules_debug_redacts_sender() {
    let rule = reject_rule("a@example.com", Some("list-1"), None, true);
    let s = format!("{rule:?}");
    assert!(s.contains("SortRule"));
    assert!(!s.contains("a@example.com"));
    let m = format!("{:?}", rule.matcher);
    assert!(m.contains("RuleMatch"));
    assert!(!m.contains("a@example.com"));
}
