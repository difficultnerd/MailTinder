//! Tests for the header guard (T-103).

use domain::{
    header_guard, Classification, GuardNote, HeaderFacts, HeaderRules, MailtoTarget, MessageClass,
    SenderKey, UnsubscribeOptions, PERSONAL_HIGH_CONFIDENCE_MAX_SCORE,
};
use proptest::prelude::*;
use url::Url;

fn test_url(s: &str) -> Url {
    Url::parse(s).unwrap_or_else(|_| panic!("invalid test url: {s}"))
}

fn test_mailto(to: &str) -> MailtoTarget {
    MailtoTarget::new(to, None, None).unwrap_or_else(|_| panic!("invalid test mailto: {to}"))
}

/// A strategy for `HeaderFacts` covering the guard's key cases.
fn facts_strategy() -> impl Strategy<Value = HeaderFacts> {
    let options = prop_oneof![
        Just(None),
        Just(Some(UnsubscribeOptions {
            one_click_https: Some(test_url("https://unsub.example.com/one")),
            https: None,
            mailto: None,
        })),
        Just(Some(UnsubscribeOptions {
            one_click_https: None,
            https: Some(test_url("https://unsub.example.com/two")),
            mailto: None,
        })),
        Just(Some(UnsubscribeOptions {
            one_click_https: None,
            https: None,
            mailto: Some(test_mailto("unsub@example.com")),
        })),
    ];
    (
        options,
        any::<bool>(),
        any::<bool>(),
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
                list_unsubscribe_present,
                precedence_bulk,
                auto_submitted,
                from_authenticated,
                is_reply_or_thread,
                reply_to_mismatch,
                display_name_spoof,
                _a,
                _b,
                _c,
            )| HeaderFacts {
                list_unsubscribe,
                list_unsubscribe_present,
                list_id: None,
                feedback_id: None,
                precedence_bulk,
                auto_submitted,
                from_authenticated,
                esp_hint: None,
                is_reply_or_thread,
                reply_to_mismatch,
                display_name_spoof,
            },
        )
}

/// A strategy for a `Classification` (any class, any score, possibly NaN
/// confidence/probabilities to prove the guard never does arithmetic on them).
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

fn header_rules_classify(facts: &HeaderFacts) -> Classification {
    HeaderRules::classify(facts, &SenderKey::from_address("sender@example.com"))
}

proptest! {
    /// GUARD-1: without a covered unsubscribe option, the guarded class is
    /// never `list`, whatever the candidate says.
    #[test]
    fn guard_1_no_list_without_valid_header(
        facts in facts_strategy().prop_filter("no covered option", |f| {
            matches!(HeaderRules::unsubscribe_route(f), domain::UnsubscribeRoute::None)
        }),
        candidate in prop::option::of(classification_strategy()),
    ) {
        // header_rules is a real HeaderRules::classify result, which never
        // says List without a covered header; the guard must keep it that way.
        let header_rules = header_rules_classify(&facts);
        let guarded = header_guard(&facts, &header_rules, candidate.as_ref());
        prop_assert_ne!(guarded.classification.class, MessageClass::List);
    }

    /// GUARD-2: high-confidence `personal` from header rules is never overridden
    /// to `list` or `suspect` by a candidate.
    #[test]
    fn guard_2_personal_not_overridden(
        // A covered unsubscribe option so GUARD-1 never fires and GUARD-2 is
        // the only rule that can change the class.
        facts in facts_strategy().prop_filter("has covered option", |f| {
            !matches!(HeaderRules::unsubscribe_route(f), domain::UnsubscribeRoute::None)
        }),
        candidate in classification_strategy(),
    ) {
        let header_rules = Classification {
            class: MessageClass::Personal,
            bulk_score: PERSONAL_HIGH_CONFIDENCE_MAX_SCORE,
            bulk_reason: "personal".to_owned(),
            confidence: Some(0.9),
            probabilities: None,
        };
        let guarded = header_guard(&facts, &header_rules, Some(&candidate));
        prop_assert!(!matches!(
            guarded.classification.class,
            MessageClass::List | MessageClass::Suspect
        ));
        if matches!(candidate.class, MessageClass::List | MessageClass::Suspect) {
            prop_assert!(guarded.notes.contains(&GuardNote::PersonalOverridden));
        }
    }

    /// CL-01 AC2: candidate = header rules result gives back the same class.
    #[test]
    fn cl_01_ac2_guard_holds_for_header_rules_as_candidate(facts in facts_strategy()) {
        let hr = header_rules_classify(&facts);
        let guarded = header_guard(&facts, &hr, Some(&hr));
        prop_assert_eq!(guarded.classification.class, hr.class);
    }
}

#[test]
fn guard_1_note_recorded_when_clamped() {
    let facts = HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        list_id: None,
        feedback_id: None,
        precedence_bulk: true,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    };
    let hr = header_rules_classify(&facts);
    let candidate = Classification {
        class: MessageClass::List,
        bulk_score: 90,
        bulk_reason: "model".to_owned(),
        confidence: Some(0.9),
        probabilities: None,
    };
    let guarded = header_guard(&facts, &hr, Some(&candidate));
    assert_eq!(guarded.classification.class, MessageClass::BulkNoHeader);
    assert!(guarded.notes.contains(&GuardNote::ListWithoutCoveredHeader));
}

#[test]
fn guard_2_low_confidence_personal_can_change() {
    let facts = HeaderFacts {
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
    };
    let hr = Classification {
        class: MessageClass::Personal,
        bulk_score: 25,
        bulk_reason: "personal".to_owned(),
        confidence: Some(0.25),
        probabilities: None,
    };
    let candidate = Classification {
        class: MessageClass::Notice,
        bulk_score: 30,
        bulk_reason: "model".to_owned(),
        confidence: Some(0.3),
        probabilities: None,
    };
    let guarded = header_guard(&facts, &hr, Some(&candidate));
    assert_eq!(guarded.classification.class, MessageClass::Notice);
    assert_eq!(guarded.notes.len(), 0);
}

#[test]
fn guard_model_failure_uses_header_rules() {
    let facts = HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: false,
        list_id: None,
        feedback_id: None,
        precedence_bulk: true,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    };
    let hr = header_rules_classify(&facts);
    let guarded = header_guard(&facts, &hr, None);
    assert_eq!(guarded.classification, hr);
    assert_eq!(guarded.notes.len(), 0);
}

#[test]
fn guard_reason_matches_class_after_clamp() {
    let facts = HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        list_id: None,
        feedback_id: None,
        precedence_bulk: true,
        auto_submitted: false,
        from_authenticated: true,
        esp_hint: None,
        is_reply_or_thread: false,
        reply_to_mismatch: false,
        display_name_spoof: false,
    };
    let hr = header_rules_classify(&facts);
    let candidate = Classification {
        class: MessageClass::List,
        bulk_score: 95,
        bulk_reason: "model says list".to_owned(),
        confidence: Some(0.9),
        probabilities: None,
    };
    let guarded = header_guard(&facts, &hr, Some(&candidate));
    // The class was clamped to BulkNoHeader; the reason and score must match
    // the header-rules result, not the candidate's.
    assert_eq!(guarded.classification.class, MessageClass::BulkNoHeader);
    assert_eq!(guarded.classification.bulk_reason, hr.bulk_reason);
    assert_eq!(guarded.classification.bulk_score, hr.bulk_score);
    // Confidence and probabilities stay from the candidate.
    assert_eq!(guarded.classification.confidence, candidate.confidence);
}

#[test]
fn guard_1_header_rules_list_without_header_clamps() {
    // A header-rules result that (incorrectly) says List with no covered
    // header and no header present: the guard clamps to BulkNoHeader.
    let facts = HeaderFacts {
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
    };
    let hr = Classification {
        class: MessageClass::List,
        bulk_score: 0,
        bulk_reason: "list".to_owned(),
        confidence: None,
        probabilities: None,
    };
    let candidate = Classification {
        class: MessageClass::List,
        bulk_score: 90,
        bulk_reason: "model".to_owned(),
        confidence: Some(0.9),
        probabilities: None,
    };
    let guarded = header_guard(&facts, &hr, Some(&candidate));
    assert_eq!(guarded.classification.class, MessageClass::BulkNoHeader);
    assert!(guarded.notes.contains(&GuardNote::ListWithoutCoveredHeader));
}
