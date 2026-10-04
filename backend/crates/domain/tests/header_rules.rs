//! Tests for the `HeaderRules` classifier (T-102).

use domain::{
    HeaderFacts, HeaderRules, MailtoTarget, MessageClass, SenderKey, UnsubscribeOptions,
    UnsubscribeRoute, BULK_REASON_MAX_CHARS,
};
use proptest::prelude::*;
use url::Url;

/// A fixed list of unsubscribe URLs, https and plain http, for the strategy.
const UNSUB_URLS: [&str; 6] = [
    "https://unsub.example.com/one",
    "https://unsub.example.com/two",
    "https://unsub.example.net/x",
    "https://unsub.example.org/y",
    "http://unsub.example.org/plain",
    "http://unsub.example.net/plain2",
];

/// Parse a constant test URL, panicking on a programming error. `unwrap` and
/// `expect` are banned, so use `unwrap_or_else` with an explicit panic.
fn test_url(s: &str) -> Url {
    Url::parse(s).unwrap_or_else(|_| panic!("invalid test url: {s}"))
}

fn test_mailto(to: &str) -> MailtoTarget {
    MailtoTarget::new(to, None, None).unwrap_or_else(|_| panic!("invalid test mailto: {to}"))
}

/// An `Arbitrary`-style strategy for `HeaderFacts` (T-102 edge cases).
fn facts_strategy() -> impl Strategy<Value = HeaderFacts> {
    let url = prop_oneof![
        Just(test_url(UNSUB_URLS[0])),
        Just(test_url(UNSUB_URLS[1])),
        Just(test_url(UNSUB_URLS[2])),
        Just(test_url(UNSUB_URLS[3])),
        Just(test_url(UNSUB_URLS[4])),
        Just(test_url(UNSUB_URLS[5])),
    ];
    let mailto = prop_oneof![
        Just(test_mailto("unsub@example.com")),
        Just(
            MailtoTarget::new("leave@example.net", Some("unsub"), None)
                .unwrap_or_else(|_| panic!("invalid test mailto"))
        ),
    ];
    let options = prop_oneof![
        Just(None),
        Just(Some(UnsubscribeOptions {
            one_click_https: None,
            https: None,
            mailto: None,
        })),
        (url.clone(), url.clone()).prop_map(|(one_click, https)| {
            Some(UnsubscribeOptions {
                one_click_https: Some(one_click),
                https: Some(https),
                mailto: None,
            })
        }),
        (url.clone(), mailto.clone()).prop_map(|(https, mailto)| {
            Some(UnsubscribeOptions {
                one_click_https: None,
                https: Some(https),
                mailto: Some(mailto),
            })
        }),
        (url, mailto).prop_map(|(one_click, mailto)| {
            Some(UnsubscribeOptions {
                one_click_https: Some(one_click),
                https: None,
                mailto: Some(mailto),
            })
        }),
    ];
    (
        options,
        any::<bool>(),
        prop::option::of("[a-z0-9._-]{0,40}"),
        prop::option::of("[a-z0-9._-]{0,40}"),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop::option::of(prop_oneof![
            Just("mailchimp".to_owned()),
            Just("sendgrid".to_owned()),
            Just("unknown_esp".to_owned()),
            Just("CANARY-esp".to_owned()),
        ]),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                list_unsubscribe,
                list_unsubscribe_present,
                list_id,
                feedback_id,
                precedence_bulk,
                auto_submitted,
                from_authenticated,
                esp_hint,
                is_reply_or_thread,
                reply_to_mismatch,
                display_name_spoof,
            )| {
                // A covered option implies the header is present; the bool
                // only adds noise when there is no covered option.
                let list_unsubscribe_present =
                    list_unsubscribe.is_some() || list_unsubscribe_present;
                HeaderFacts {
                    list_unsubscribe,
                    list_unsubscribe_present,
                    list_id,
                    feedback_id,
                    precedence_bulk,
                    auto_submitted,
                    from_authenticated,
                    esp_hint,
                    is_reply_or_thread,
                    reply_to_mismatch,
                    display_name_spoof,
                }
            },
        )
}

fn sender_strategy() -> impl Strategy<Value = SenderKey> {
    prop_oneof![
        Just(SenderKey::from_address("")),
        Just(SenderKey::from_address("person@example.com")),
        Just(SenderKey::from_address("noreply@example.com")),
        Just(SenderKey::from_address("no-reply@example.net")),
        Just(SenderKey::from_address("news@example.org")),
    ]
}

/// A covered one-click header.
fn one_click_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(test_url("https://unsub.example.com/one")),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    }
}

/// A present but uncovered header (no DKIM-covered options).
fn uncovered_present_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    }
}

#[test]
fn cl_01_ac2_present_uncovered_header_is_bulk_no_header() {
    let facts = uncovered_present_facts();
    let sender = SenderKey::from_address("news@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::BulkNoHeader);
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::None
    ));
}

#[test]
fn un_02_ac2_uncovered_one_click_is_bulk_no_header() {
    // One-click headers present but not covered by a passing DKIM signature.
    let facts = HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    };
    let sender = SenderKey::from_address("news@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::BulkNoHeader);
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::None
    ));
}

#[test]
fn un_02_ac2a_list_without_from_alignment() {
    // A covered one-click header gives `list` even when the From is not
    // DMARC-aligned (`from_authenticated` false).
    let facts = HeaderFacts {
        from_authenticated: false,
        ..one_click_facts()
    };
    let sender = SenderKey::from_address("news@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::List);
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::OneClick(_)
    ));
}

#[test]
fn un_03_ac3_uncovered_mailto_is_bulk_no_header() {
    let facts = HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    };
    let sender = SenderKey::from_address("news@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::BulkNoHeader);
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::None
    ));
}

#[test]
fn un_04_ac6_https_only_gives_manual_link() {
    let facts = HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: None,
            https: Some(test_url("https://unsub.example.com/page")),
            mailto: None,
        }),
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    };
    let sender = SenderKey::from_address("news@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::List);
    match HeaderRules::unsubscribe_route(&facts) {
        UnsubscribeRoute::ManualLink(Some(url)) => {
            assert_eq!(url.scheme(), "https");
        }
        other => panic!("expected ManualLink(Some), got {other:?}"),
    }
}

#[test]
fn header_rules_one_click_preferred_over_mailto() {
    let facts = HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: Some(test_url("https://unsub.example.com/one")),
            https: None,
            mailto: Some(test_mailto("unsub@example.com")),
        }),
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    };
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::OneClick(_)
    ));
}

#[test]
fn header_rules_plain_http_link_has_no_url() {
    let facts = HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: None,
            https: Some(test_url("http://unsub.example.org/plain")),
            mailto: None,
        }),
        list_unsubscribe_present: true,
        ..HeaderFacts::default()
    };
    assert!(matches!(
        HeaderRules::unsubscribe_route(&facts),
        UnsubscribeRoute::ManualLink(None)
    ));
}

#[test]
fn header_rules_phishing_lookalike_is_suspect() {
    let facts = HeaderFacts {
        from_authenticated: false,
        reply_to_mismatch: true,
        ..HeaderFacts::default()
    };
    let sender = SenderKey::from_address("bank@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::Suspect);
}

#[test]
fn header_rules_empty_sender_is_suspect() {
    let facts = HeaderFacts::default();
    let sender = SenderKey::from_address("");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::Suspect);
}

#[test]
fn header_rules_notice_for_auto_submitted_noreply() {
    let facts = HeaderFacts {
        auto_submitted: true,
        ..HeaderFacts::default()
    };
    let sender = SenderKey::from_address("noreply@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::Notice);
}

#[test]
fn header_rules_personal_for_plain_mail() {
    let facts = HeaderFacts::default();
    let sender = SenderKey::from_address("person@example.com");
    let c = HeaderRules::classify(&facts, &sender);
    assert_eq!(c.class, MessageClass::Personal);
}

#[test]
fn header_rules_one_click_alone_scores_45() {
    // A valid one-click header alone: 30 (present) + 15 (route) = 45.
    let facts = one_click_facts();
    assert_eq!(HeaderRules::score(&facts), 45);
}

proptest! {
    #[test]
    fn cl_01_ac2_no_covered_option_never_list(
        facts in facts_strategy().prop_filter("no covered option", |f| f.list_unsubscribe.is_none()),
        sender in sender_strategy(),
    ) {
        let c = HeaderRules::classify(&facts, &sender);
        prop_assert_ne!(c.class, MessageClass::List);
    }

    #[test]
    fn sw_03_ac5_no_header_no_route(
        facts in facts_strategy().prop_filter("no header present", |f| !f.list_unsubscribe_present),
    ) {
        prop_assert!(matches!(
            HeaderRules::unsubscribe_route(&facts),
            UnsubscribeRoute::None
        ));
    }

    #[test]
    fn header_rules_score_always_0_to_100(facts in facts_strategy()) {
        let score = HeaderRules::score(&facts);
        prop_assert!(score <= 100);
    }

    #[test]
    fn header_rules_reason_never_contains_header_values(
        facts in facts_strategy(),
        sender in sender_strategy(),
    ) {
        let c = HeaderRules::classify(&facts, &sender);
        prop_assert!(!c.bulk_reason.contains("CANARY"));
        prop_assert!(c.bulk_reason.chars().count() <= BULK_REASON_MAX_CHARS);
    }

    #[test]
    fn header_rules_deterministic(facts in facts_strategy(), sender in sender_strategy()) {
        let a = HeaderRules::classify(&facts, &sender);
        let b = HeaderRules::classify(&facts, &sender);
        prop_assert_eq!(a, b);
    }
}
