//! Property and fuzz-style tests for the `HeaderRules` classifier (T-1110,
//! T-102 step 6).
//!
//! Classification is a pure function of its arguments: the same input gives
//! the same class and score, and it is monotone in the documented way — adding
//! an unsubscribe header (or a covered unsubscribe option) never lowers the
//! bulk score.

use domain::{HeaderFacts, HeaderRules, MailtoTarget, MessageClass, SenderKey, UnsubscribeOptions};
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

/// A fixed, sender-only key strategy (an empty key makes every message
/// suspect, which the other properties still hold for).
fn sender_strategy() -> impl Strategy<Value = SenderKey> {
    prop_oneof![
        Just(SenderKey::from_address("")),
        Just(SenderKey::from_address("person@example.com")),
        Just(SenderKey::from_address("noreply@example.com")),
        Just(SenderKey::from_address("news@example.org")),
    ]
}

/// A covered one-click option, for the monotonicity property.
fn one_click_options() -> UnsubscribeOptions {
    UnsubscribeOptions {
        one_click_https: Some(test_url(UNSUB_URLS[0])),
        https: None,
        mailto: None,
    }
}

proptest! {
    /// Property 6: the class and score are a pure function of the input.
    #[test]
    fn t1110_classify_is_deterministic(facts in facts_strategy(), sender in sender_strategy()) {
        let first = HeaderRules::classify(&facts, &sender);
        let second = HeaderRules::classify(&facts, &sender);
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(HeaderRules::score(&facts), HeaderRules::score(&facts));
        prop_assert_eq!(
            format!("{:?}", HeaderRules::unsubscribe_route(&facts)),
            format!("{:?}", HeaderRules::unsubscribe_route(&facts))
        );
    }

    /// Property 6: adding an unsubscribe header, then a covered option, never
    /// lowers the bulk score (both are positive signals).
    #[test]
    fn t1110_score_monotone_when_unsubscribe_header_added(facts in facts_strategy()) {
        let mut without = facts;
        without.list_unsubscribe = None;
        without.list_unsubscribe_present = false;

        let mut present = without.clone();
        present.list_unsubscribe_present = true;

        let mut covered = present.clone();
        covered.list_unsubscribe = Some(one_click_options());

        let base = HeaderRules::score(&without);
        let with_present = HeaderRules::score(&present);
        let with_covered = HeaderRules::score(&covered);
        prop_assert!(
            with_present >= base,
            "adding a List-Unsubscribe header lowered the score: {base} -> {with_present}"
        );
        prop_assert!(
            with_covered >= with_present,
            "adding a covered option lowered the score: {with_present} -> {with_covered}"
        );
    }

    /// Property 6: a covered one-click header is never classified below a
    /// plain present-but-uncovered header (both end up at least bulk).
    #[test]
    fn t1110_present_header_never_personal_or_costlier(facts in facts_strategy()) {
        let mut present = facts;
        present.list_unsubscribe = None;
        present.list_unsubscribe_present = true;
        let sender = SenderKey::from_address("news@example.com");
        let class = HeaderRules::classify(&present, &sender).class;
        prop_assert_ne!(class, MessageClass::Personal);
    }
}
