//! Property and fuzz-style tests for the raw-header parsers (`headers.rs`) and
//! the DKIM coverage check (`auth_results.rs`) — T-1110, T-406, S6 section 6,
//! S10 5 "Hostile content".
//!
//! Every parser of internet text returns for arbitrary bytes (NULs, control
//! characters, bidirectional marks, CR/LF injection, folded and duplicated
//! headers) and never produces a value carrying a raw CR or LF. `assess_auth`
//! follows the T-406 contract exactly and strictly: only Gmail's own
//! `Authentication-Results` can authenticate a `From` or offer unsubscribe
//! options, a one-click flag stands only for the exact RFC 8058 header pair,
//! and an untrusted authserv-id changes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapters_gmail::auth_results::assess_auth;
use adapters_gmail::headers::{
    build_header_facts, decode_header_text, parse_from, ParsedFrom, RawHeaders,
};
use proptest::prelude::*;

/// The exact `List-Unsubscribe-Post` value that marks a one-click list
/// (RFC 8058).
const ONE_CLICK: &str = "List-Unsubscribe=One-Click";

/// Header names the adapter reads, plus junk and the empty name.
const HEADER_NAMES: [&str; 17] = [
    "From",
    "Subject",
    "Date",
    "List-Unsubscribe",
    "List-Unsubscribe-Post",
    "List-Id",
    "Feedback-ID",
    "Precedence",
    "Auto-Submitted",
    "Authentication-Results",
    "DKIM-Signature",
    "In-Reply-To",
    "References",
    "Reply-To",
    "Return-Path",
    "",
    "X-Random",
];

/// A header name: the real ones in either case, or arbitrary junk.
fn header_name() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(HEADER_NAMES.to_vec()).prop_map(str::to_owned),
        prop::sample::select(HEADER_NAMES.to_vec()).prop_map(str::to_lowercase),
        "[A-Za-z0-9-]{0,20}",
    ]
}

/// A header value: hostile Unicode, real shapes, and raw CR/LF folds.
fn hostile_value() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::collection::vec(any::<char>(), 0..80).prop_map(|c| c.into_iter().collect()),
        Just("mx.google.com; dkim=pass header.d=example.com; dmarc=pass header.from=example.com"
            .to_owned()),
        Just(
            "v=1; a=rsa-sha256; d=example.com; s=sel; h=from:to:list-unsubscribe:list-unsubscribe-post; b=AbCdEf12"
                .to_owned()
        ),
        Just(ONE_CLICK.to_owned()),
        Just("<https://u.example.com/x>".to_owned()),
        Just("<mailto:u@example.com?subject=stop>".to_owned()),
        Just("List <List.Example.COM>".to_owned()),
        Just("bulk".to_owned()),
        Just("auto-replied".to_owned()),
        Just("Sender\r\nX-Evil: 1 <a@example.com>".to_owned()),
        Just("invoice\r\n folded".to_owned()),
        Just("= utf-8?q?Ren=C3=A9?=".to_owned()),
    ]
}

/// Arbitrary header list: any names, any order, duplicates kept. Returned as
/// pairs because `RawHeaders` itself deliberately has no `Debug` (it holds
/// message text; S5).
fn header_pairs() -> impl Strategy<Value = Vec<(String, String)>> {
    prop::collection::vec((header_name(), hostile_value()), 0..10)
}

/// A `From` for arbitrary headers: parsed when possible, empty otherwise.
fn parsed_from(h: &RawHeaders) -> ParsedFrom {
    parse_from(h.first("From").unwrap_or("")).unwrap_or(ParsedFrom {
        display: String::new(),
        address: String::new(),
    })
}

/// True when `s` carries a raw carriage return, line feed or NUL.
fn has_raw_control(s: &str) -> bool {
    s.contains(['\r', '\n', '\0'])
}

proptest! {
    /// Property 1 and 4: arbitrary headers never panic any of the parsers.
    #[test]
    fn t1110_headers_never_panic(pairs in header_pairs()) {
        let h = RawHeaders(pairs);
        let from = parsed_from(&h);
        let _ = parse_from(h.first("From").unwrap_or(""));
        for v in h.all("Subject") {
            let _ = decode_header_text(v);
        }
        let _ = build_header_facts(&h, &from);
        let _ = assess_auth(&h, &from.address);
    }

    /// Property 4: no parser of header text emits a value with a raw CR, LF or
    /// NUL — subject, display name, address, list/feedback id, or a mailto
    /// target taken from a covered header.
    #[test]
    fn t1110_header_values_have_no_raw_crlf(pairs in header_pairs()) {
        let h = RawHeaders(pairs);
        for (_, value) in &h.0 {
            let text = decode_header_text(value);
            prop_assert!(!has_raw_control(&text), "decode_header_text leaked a control: {text:?}");
        }
        if let Some(p) = parse_from(h.first("From").unwrap_or("")) {
            prop_assert!(!has_raw_control(&p.display), "display leaked a control");
            prop_assert!(!has_raw_control(&p.address), "address leaked a control");
        }
        let from = parsed_from(&h);
        let facts = build_header_facts(&h, &from);
        for field in [facts.list_id.as_deref(), facts.feedback_id.as_deref(), facts.esp_hint.as_deref()]
            .into_iter()
            .flatten()
        {
            prop_assert!(!has_raw_control(field), "header fact leaked a control: {field:?}");
        }
        let assessment = assess_auth(&h, &from.address);
        if let Some(options) = &assessment.unsubscribe {
            if let Some(mailto) = &options.mailto {
                prop_assert!(!has_raw_control(mailto.to()));
                for text in [mailto.subject(), mailto.body()].into_iter().flatten() {
                    prop_assert!(!has_raw_control(text));
                }
            }
        }
    }

    /// Property 4: an `Authentication-Results` header whose authserv-id is not
    /// `mx.google.com` never authenticates a `From` and never offers
    /// unsubscribe options, even with `dkim=pass` and `dmarc=pass` results.
    #[test]
    fn t1110_auth_untrusted_authserv_changes_nothing(
        authserv in "[A-Za-z0-9.-]{0,30}"
            .prop_filter("only untrusted ids", |s| !s.eq_ignore_ascii_case("mx.google.com")),
        domain in "[a-z][a-z0-9-]{0,20}\\.[a-z]{2,6}",
    ) {
        let h = RawHeaders(vec![
            (
                "Authentication-Results".to_owned(),
                format!(
                    "{authserv}; dkim=pass header.d={domain}; dmarc=pass header.from={domain}"
                ),
            ),
            (
                "DKIM-Signature".to_owned(),
                format!("v=1; a=rsa-sha256; d={domain}; s=sel; h=from:to:subject:list-unsubscribe:list-unsubscribe-post; b=AbCdEf12"),
            ),
            ("List-Unsubscribe".to_owned(), "<https://u.example.com/x>".to_owned()),
            ("List-Unsubscribe-Post".to_owned(), ONE_CLICK.to_owned()),
        ]);
        let a = assess_auth(&h, &domain);
        prop_assert!(!a.from_authenticated, "untrusted authserv {authserv:?} authenticated a From");
        prop_assert!(a.unsubscribe.is_none(), "untrusted authserv {authserv:?} offered unsubscribe options");
    }

    /// Property 3: the one-click flag is set only when the header pair is
    /// exactly the RFC 8058 form — one `List-Unsubscribe-Post` whose trimmed
    /// value is `List-Unsubscribe=One-Click` — and never otherwise.
    #[test]
    fn t1110_one_click_only_for_exact_rfc8058_pair(pairs in header_pairs()) {
        let h = RawHeaders(pairs);
        let from = parsed_from(&h);
        let a = assess_auth(&h, &from.address);
        if let Some(options) = &a.unsubscribe {
            prop_assert_eq!(h.count("List-Unsubscribe"), 1, "unsubscribe offered without one header");
            if options.one_click_https.is_some() {
                prop_assert_eq!(h.count("List-Unsubscribe-Post"), 1);
                let value = h.first("List-Unsubscribe-Post").unwrap_or_default();
                prop_assert_eq!(value.trim(), ONE_CLICK, "one-click for a non-RFC8058 post value");
            }
        }
    }

    /// Property 4: a `From` is authenticated only by a trusted Gmail DMARC pass
    /// for that domain, or by a trusted DKIM pass aligned to it.
    #[test]
    fn t1110_auth_trusted_dmarc_pass_authenticates(
        domain in "[a-z][a-z0-9-]{0,20}\\.[a-z]{2,6}",
    ) {
        let h = RawHeaders(vec![(
            "Authentication-Results".to_owned(),
            format!("mx.google.com; dmarc=pass header.from={domain}"),
        )]);
        prop_assert!(assess_auth(&h, &domain).from_authenticated);
    }

    /// Property 4: a trusted `dkim=pass` aligned to the `From` domain
    /// authenticates it even with no DMARC result.
    #[test]
    fn t1110_auth_trusted_dkim_aligned_authenticates(
        domain in "[a-z][a-z0-9-]{0,20}\\.[a-z]{2,6}",
    ) {
        let h = RawHeaders(vec![
            (
                "Authentication-Results".to_owned(),
                format!("mx.google.com; dkim=pass header.d={domain}"),
            ),
            (
                "DKIM-Signature".to_owned(),
                format!("v=1; a=rsa-sha256; d={domain}; s=sel; h=from:to:subject; b=AbCdEf12"),
            ),
        ]);
        prop_assert!(assess_auth(&h, &domain).from_authenticated);
    }
}

// ---------------------------------------------------------------------------
// Deterministic fixtures for the T-406 contract edges.
// ---------------------------------------------------------------------------

/// Property 4: an Authentication-Results from any authserv-id other than the
/// trusted one is inert, including near-miss ids.
#[test]
fn t1110_auth_untrusted_authserv_fixtures() {
    let near_misses = [
        "evil.example",
        "mx.google.com.evil",
        "mx.google.com.",
        "MX.GOOGLE.COMX",
    ];
    for authserv in near_misses {
        let h = RawHeaders(vec![
            (
                "Authentication-Results".to_owned(),
                format!("{authserv}; dkim=pass header.d=example.com; dmarc=pass header.from=example.com"),
            ),
            (
                "DKIM-Signature".to_owned(),
                "v=1; a=rsa-sha256; d=example.com; s=sel; h=from:list-unsubscribe:list-unsubscribe-post; b=AbCdEf12"
                    .to_owned(),
            ),
            ("List-Unsubscribe".to_owned(), "<https://u.example.com/x>".to_owned()),
            ("List-Unsubscribe-Post".to_owned(), ONE_CLICK.to_owned()),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(!a.from_authenticated, "{authserv} must not authenticate");
        assert!(a.unsubscribe.is_none(), "{authserv} must not offer options");
    }
}

/// Property 4: a `dkim=pass` for an unrelated domain does not authenticate the
/// `From` and does not make an uncovered header usable.
#[test]
fn t1110_auth_unrelated_dkim_does_not_authenticate() {
    let h = RawHeaders(vec![
        (
            "Authentication-Results".to_owned(),
            "mx.google.com; dkim=pass header.d=evil.example".to_owned(),
        ),
        (
            "DKIM-Signature".to_owned(),
            "v=1; a=rsa-sha256; d=evil.example; s=sel; h=from:to:subject; b=AbCdEf12".to_owned(),
        ),
        (
            "List-Unsubscribe".to_owned(),
            "<https://u.example.com/x>".to_owned(),
        ),
    ]);
    let a = assess_auth(&h, "example.com");
    assert!(!a.from_authenticated);
    assert!(a.unsubscribe.is_none());
}

/// Property 4: unsubscribe options are offered only when the trusted
/// Authentication-Results reports a `dkim=pass` for a signature that is
/// identified among the `DKIM-Signature` headers and covers
/// `List-Unsubscribe`; a `List-Unsubscribe` present but uncovered is inert.
#[test]
fn t1110_unsubscribe_only_when_covered_by_identified_signature() {
    // Covered: the identified signature lists list-unsubscribe.
    let covered = RawHeaders(vec![
        (
            "Authentication-Results".to_owned(),
            "mx.google.com; dkim=pass header.d=example.com header.s=sel1".to_owned(),
        ),
        (
            "DKIM-Signature".to_owned(),
            "v=1; a=rsa-sha256; d=example.com; s=sel1; h=from:to:subject:list-unsubscribe; b=AbCdEf12"
                .to_owned(),
        ),
        (
            "List-Unsubscribe".to_owned(),
            "<https://u.example.com/x>".to_owned(),
        ),
    ]);
    assert!(assess_auth(&covered, "example.com").unsubscribe.is_some());

    // Present but the signature does not cover it: no options.
    let uncovered = RawHeaders(vec![
        (
            "Authentication-Results".to_owned(),
            "mx.google.com; dkim=pass header.d=example.com header.s=sel1".to_owned(),
        ),
        (
            "DKIM-Signature".to_owned(),
            "v=1; a=rsa-sha256; d=example.com; s=sel1; h=from:to:subject; b=AbCdEf12".to_owned(),
        ),
        (
            "List-Unsubscribe".to_owned(),
            "<https://u.example.com/x>".to_owned(),
        ),
    ]);
    assert!(assess_auth(&uncovered, "example.com").unsubscribe.is_none());
}

/// Property 4: a folded or injected header never leaks a raw CR or LF into the
/// values the adapter derives from it.
#[test]
fn t1110_folded_and_injected_headers_never_leak_crlf() {
    let injected = "Sender\r\nX-Evil: 1 <a@example.com>";
    if let Some(p) = parse_from(injected) {
        assert!(!p.display.contains(['\r', '\n']));
        assert!(!p.address.contains(['\r', '\n']));
    }
    let text = decode_header_text("invoice\r\nX-Evil: 1");
    assert!(!text.contains(['\r', '\n']));

    let h = RawHeaders(vec![
        ("List-Id".to_owned(), "List\r\n .Example.COM".to_owned()),
        ("Feedback-ID".to_owned(), "1\r\ncampaign".to_owned()),
    ]);
    let from = ParsedFrom {
        display: String::new(),
        address: "a@example.com".to_owned(),
    };
    let facts = build_header_facts(&h, &from);
    for field in [facts.list_id.as_deref(), facts.feedback_id.as_deref()]
        .into_iter()
        .flatten()
    {
        assert!(
            !field.contains(['\r', '\n']),
            "header fact leaked CR/LF: {field:?}"
        );
    }
}
