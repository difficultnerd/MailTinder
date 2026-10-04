//! Tests for the swipe planner (T-105a).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{
    derive_swipe_ids, header_guard, plan_swipe, Classification, HeaderFacts, JobId, LabelSet,
    MailboxChange, MailboxId, MessageClass, MessageId, MessageMeta, RuleId, SenderKey, SenderStats,
    SwipeAction, SwipeIds, SwipeInput, SwipeOutcome, Tunables, UnsubscribeOptions, UserId,
};
use proptest::prelude::*;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

fn test_url(s: &str) -> Url {
    Url::parse(s).unwrap_or_else(|_| panic!("invalid test url: {s}"))
}

fn mid() -> MailboxId {
    MailboxId(Uuid::new_v4())
}
fn uid() -> UserId {
    UserId(Uuid::new_v4())
}
fn ids() -> SwipeIds {
    SwipeIds {
        job_id: JobId(Uuid::new_v4()),
        rule_id: RuleId(Uuid::new_v4()),
    }
}

fn facts(
    list_unsubscribe: Option<UnsubscribeOptions>,
    lu_present: bool,
    list_id: Option<&str>,
    authed: bool,
) -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe,
        list_unsubscribe_present: lu_present,
        list_id: list_id.map(str::to_owned),
        feedback_id: None,
        precedence_bulk: false,
        auto_submitted: false,
        from_authenticated: authed,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    }
}

fn meta(facts: HeaderFacts) -> MessageMeta {
    MessageMeta {
        mailbox: mid(),
        id: MessageId::new("m1").unwrap_or_else(|_| panic!("id")),
        internal_date: T0,
        from_display: String::new(),
        from_address: "a@example.com".to_owned(),
        sender: SenderKey::from_address("a@example.com"),
        subject: String::new(),
        labels: LabelSet::new(),
        facts,
    }
}

fn plan(action: SwipeAction, meta: &MessageMeta, stats: &SenderStats) -> domain::SwipePlan {
    let badge = Classification {
        class: MessageClass::List,
        bulk_score: 0,
        bulk_reason: String::new(),
        confidence: None,
        probabilities: None,
    };
    plan_swipe(&SwipeInput {
        action,
        meta,
        badge: &badge,
        stats,
        ids: ids(),
        source_swipe: Uuid::new_v4(),
        now: T0,
        tunables: &Tunables::default(),
    })
}

#[test]
fn sw_01_ac1_keep_changes_nothing() {
    let m = meta(facts(None, false, None, true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Keep, &m, &stats);
    assert_eq!(p.change, MailboxChange::None);
    assert_eq!(p.outcome, SwipeOutcome::Kept);
    assert_eq!(p.stats_after.keeps, 1);
}

#[test]
fn sw_02_ac1_skip_changes_nothing() {
    let m = meta(facts(None, false, None, true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Skip, &m, &stats);
    assert_eq!(p.change, MailboxChange::None);
    assert_eq!(p.outcome, SwipeOutcome::Skipped);
    assert_eq!(p.stats_after, stats);
}

#[test]
fn sw_03_ac2_reject_queues_unsubscribe_with_delay() {
    let opts = Some(UnsubscribeOptions {
        one_click_https: Some(test_url("https://unsub.example.com/one")),
        https: None,
        mailto: None,
    });
    let m = meta(facts(opts, true, Some("list-1"), true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.change, MailboxChange::Trash);
    assert_eq!(p.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    let unsub = p.unsubscribe.unwrap();
    assert_eq!(unsub.due_at, T0 + time::Duration::minutes(5));
    assert!(p.rule.is_some());
}

#[test]
fn sw_03_ac2_mailto_route_queues_mailto_job() {
    let opts = Some(UnsubscribeOptions {
        one_click_https: None,
        https: None,
        mailto: Some(
            domain::MailtoTarget::new("unsub@example.com", None, None)
                .unwrap_or_else(|_| panic!("mailto")),
        ),
    });
    let m = meta(facts(opts, true, Some("list-1"), true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.outcome, SwipeOutcome::TrashedUnsubscribeQueued);
    assert!(matches!(
        p.unsubscribe.unwrap().target,
        domain::UnsubscribeTarget::Mailto(_)
    ));
}

#[test]
fn sw_03_ac3_suspect_reported_no_job() {
    // A suspect message: not authenticated and spoofed display name.
    let mut f = facts(None, false, None, false);
    f.display_name_spoof = true;
    let m = meta(f);
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.change, MailboxChange::ReportSpamAndTrash);
    assert_eq!(p.outcome, SwipeOutcome::ReportedSpam);
    assert!(p.unsubscribe.is_none());
    assert!(p.rule.is_none());
}

#[test]
fn sw_03_ac4_personal_trash_and_count() {
    // A personal message: no list id, no unsubscribe, authenticated.
    let m = meta(facts(None, false, None, true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.change, MailboxChange::Trash);
    assert_eq!(p.outcome, SwipeOutcome::Trashed);
    assert!(p.rule.is_none());
    assert_eq!(p.stats_after.rejects_counted.len(), 1);
}

#[test]
fn sw_03_ac5_bulk_no_header_creates_reject_rule() {
    // BulkNoHeader: list_unsubscribe_present true but no covered option.
    let m = meta(facts(None, true, None, true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(p.unsubscribe.is_none());
    assert!(p.manual_unsubscribe.is_none());
    assert!(p.rule.is_some());
}

#[test]
fn sw_04_ac2_file_applies_category() {
    let m = meta(facts(None, false, None, true));
    let stats = SenderStats::default();
    let cat = domain::CategoryId(Uuid::new_v4());
    let p = plan(SwipeAction::File { category: cat }, &m, &stats);
    assert_eq!(p.change, MailboxChange::ApplyCategory { category: cat });
    assert_eq!(p.outcome, SwipeOutcome::Filed);
    assert_eq!(p.stats_after.last_filed, Some(cat));
}

#[test]
fn un_04_ac6_https_only_manual_link_no_job() {
    let opts = Some(UnsubscribeOptions {
        one_click_https: None,
        https: Some(test_url("https://unsub.example.com/page")),
        mailto: None,
    });
    let m = meta(facts(opts, true, Some("list-1"), true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    assert!(p.unsubscribe.is_none());
    assert!(p.manual_unsubscribe.unwrap().link.is_some());
    assert!(p.rule.is_some());
}

#[test]
fn un_04_ac6_plain_http_manual_item_without_link() {
    let opts = Some(UnsubscribeOptions {
        one_click_https: None,
        https: Some(test_url("http://unsub.example.com/page")),
        mailto: None,
    });
    let m = meta(facts(opts, true, Some("list-1"), true));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    assert!(p.manual_unsubscribe.unwrap().link.is_none());
}

#[test]
fn sr_01_ac6_spoofed_list_trashed_no_rule() {
    let opts = Some(UnsubscribeOptions {
        one_click_https: Some(test_url("https://unsub.example.com/one")),
        https: None,
        mailto: None,
    });
    let m = meta(facts(opts, true, Some("list-1"), false));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert_eq!(p.change, MailboxChange::Trash);
    assert!(p.rule.is_none());
    assert_eq!(p.stats_after.rejects_counted.len(), 0);
    // The job is still queued (only the rule and count depend on auth).
    assert!(p.unsubscribe.is_some());
}

#[test]
fn pb_01_ac4_spoofed_personal_never_prompts() {
    let m = meta(facts(None, false, None, false));
    let stats = SenderStats::default();
    let p = plan(SwipeAction::Reject, &m, &stats);
    assert!(!p.block_prompt);
    assert_eq!(p.stats_after.rejects_counted.len(), 0);
}

#[test]
fn swipe_ids_stable_and_distinct() {
    let user = uid();
    let key = Uuid::new_v4();
    let a = derive_swipe_ids(&user, key);
    let b = derive_swipe_ids(&user, key);
    assert_eq!(a, b);
    assert_ne!(a.job_id.0, a.rule_id.0);
    let other = derive_swipe_ids(&uid(), key);
    assert_ne!(a.job_id, other.job_id);
}

proptest! {
    /// SW-03 AC1: every reject plan's change is Trash or ReportSpamAndTrash.
    #[test]
    fn sw_03_ac1_reject_always_trash_never_delete(
        facts in facts_strategy(),
    ) {
        let m = meta(facts);
        let stats = SenderStats::default();
        let p = plan(SwipeAction::Reject, &m, &stats);
        prop_assert!(matches!(
            p.change,
            MailboxChange::Trash | MailboxChange::ReportSpamAndTrash
        ));
    }

    /// SW-03 AC5: no List-Unsubscribe header means no unsubscribe attempt.
    #[test]
    fn sw_03_ac5_no_header_no_unsubscribe_attempt(
        facts in facts_strategy().prop_filter("no header", |f| !f.list_unsubscribe_present),
    ) {
        let m = meta(facts);
        let stats = SenderStats::default();
        let p = plan(SwipeAction::Reject, &m, &stats);
        prop_assert!(p.unsubscribe.is_none());
        prop_assert!(p.manual_unsubscribe.is_none());
    }

    /// GUARD-1: no covered header means no unsubscribe job, whatever the badge.
    #[test]
    fn guard_1_no_job_without_valid_header(
        facts in facts_strategy().prop_filter("no covered option", |f| {
            matches!(domain::HeaderRules::unsubscribe_route(f), domain::UnsubscribeRoute::None)
        }),
        badge in classification_strategy(),
    ) {
        let m = meta(facts);
        let stats = SenderStats::default();
        let p = plan_swipe(&SwipeInput {
            action: SwipeAction::Reject,
            meta: &m,
            badge: &badge,
            stats: &stats,
            ids: ids(),
            source_swipe: Uuid::new_v4(),
            now: T0,
            tunables: &Tunables::default(),
        });
        prop_assert!(p.unsubscribe.is_none());
    }

    /// GUARD-3: plans are identical with any model output as the badge and
    /// with the header-rules result.
    #[test]
    fn guard_3_models_cannot_act(
        facts in facts_strategy(),
        model in classification_strategy(),
        action in prop::sample::select([
            SwipeAction::Keep, SwipeAction::Skip, SwipeAction::Reject,
        ].as_slice()),
    ) {
        let m = meta(facts);
        let stats = SenderStats::default();
        let hr = domain::HeaderRules::classify(&m.facts, &m.sender);
        let guarded_model = header_guard(&m.facts, &hr, Some(&model)).classification;
        let ids = ids();
        let source = Uuid::new_v4();
        let p_model = plan_swipe(&SwipeInput {
            action,
            meta: &m,
            badge: &guarded_model,
            stats: &stats,
            ids,
            source_swipe: source,
            now: T0,
            tunables: &Tunables::default(),
        });
        let p_hr = plan_swipe(&SwipeInput {
            action,
            meta: &m,
            badge: &hr,
            stats: &stats,
            ids,
            source_swipe: source,
            now: T0,
            tunables: &Tunables::default(),
        });
        prop_assert_eq!(p_model, p_hr);
    }
}

fn facts_strategy() -> impl Strategy<Value = HeaderFacts> {
    let url = prop_oneof![
        Just(test_url("https://unsub.example.com/one")),
        Just(test_url("https://unsub.example.com/two")),
        Just(test_url("http://unsub.example.org/plain")),
    ];
    let mailto = Just(
        domain::MailtoTarget::new("unsub@example.com", None, None)
            .unwrap_or_else(|_| panic!("mailto")),
    );
    let options = prop_oneof![
        Just(None),
        (url.clone(), url.clone()).prop_map(|(one_click, https)| Some(UnsubscribeOptions {
            one_click_https: Some(one_click),
            https: Some(https),
            mailto: None,
        })),
        (url, mailto).prop_map(|(one_click, mailto)| Some(UnsubscribeOptions {
            one_click_https: Some(one_click),
            https: None,
            mailto: Some(mailto),
        })),
    ];
    (
        options,
        any::<bool>(),
        prop::option::of("[a-z0-9-]{0,20}"),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                list_unsubscribe,
                lu_present,
                list_id,
                precedence_bulk,
                auto_submitted,
                from_authenticated,
                is_reply_or_thread,
                reply_to_mismatch,
                display_name_spoof,
                _a,
                _b,
            )| {
                let lu_present = lu_present || list_unsubscribe.is_some();
                HeaderFacts {
                    list_unsubscribe,
                    list_unsubscribe_present: lu_present,
                    list_id,
                    feedback_id: None,
                    precedence_bulk,
                    auto_submitted,
                    from_authenticated,
                    esp_hint: None,
                    is_reply_or_thread,
                    reply_to_mismatch,
                    display_name_spoof,
                }
            },
        )
}

fn classification_strategy() -> impl Strategy<Value = Classification> {
    (
        prop::sample::select(MessageClass::ALL.as_slice()),
        0u8..=100u8,
        "[a-z ]{0,40}",
        prop::option::of(prop::num::f32::ANY),
        prop::option::of([prop::num::f32::ANY; 5]),
    )
        .prop_map(
            |(class, bulk_score, bulk_reason, confidence, probabilities)| Classification {
                class,
                bulk_score,
                bulk_reason,
                confidence,
                probabilities,
            },
        )
}
