//! Property and fuzz-style tests for the `List-Unsubscribe` URI parser
//! (T-1110, S6 section 6, ASVS V1.2.2).
//!
//! Whatever a hostile header contains, the parser must *return* (never panic)
//! and must hand back only `https:` targets with a host and no userinfo, or
//! `mailto:` targets the project's own validator accepts. Every other scheme
//! (plain `http`, `ftp`, `javascript`, `file`, `data`, …) and any userinfo are
//! dropped, never upgraded. The SSRF refusal of private, loopback, link-local
//! and metadata *hosts* happens later, at the egress boundary (T-306): see
//! `backend/crates/egress/tests/prop_target.rs` for the encoding properties.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapters_gmail::list_unsubscribe::{parse_list_unsubscribe, LuUris};
use domain::MailtoTarget;
use proptest::prelude::*;

/// The parser may only ever hand back a safe `https` target or a `mailto`
/// target the project's own validator accepts (S6 6).
fn assert_targets_valid(got: &LuUris) {
    if let Some(url) = &got.https {
        assert_eq!(url.scheme(), "https", "accepted a non-https scheme");
        assert!(url.host_str().is_some(), "accepted a hostless url");
        assert!(url.username().is_empty(), "accepted userinfo");
        assert!(url.password().is_none(), "accepted a password");
    }
    if let Some(mailto) = &got.mailto {
        let round_trip = format!("mailto:{}", mailto.to());
        assert!(
            MailtoTarget::parse(&round_trip).is_ok(),
            "accepted a mailto the project validator rejects: {round_trip}"
        );
        assert_eq!(
            mailto.to().matches('@').count(),
            1,
            "accepted a multi-recipient"
        );
    }
}

/// Arbitrary hostile text: any scalar value (NUL, controls, bidirectional and
/// zero-width marks) up to a few hundred characters.
fn hostile_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..600).prop_map(|chars| chars.into_iter().collect())
}

/// Arbitrary bytes, converted lossily so unpaired surrogates cannot hide.
fn hostile_bytes() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<u8>(), 0..600)
        .prop_map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

proptest! {
    /// Property 1: any Unicode input gives a result, never a panic, and the
    /// result obeys property 2.
    #[test]
    fn t1110_list_unsubscribe_never_panics(value in hostile_text()) {
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }

    /// Property 1 over the proptest arbitrary `String`.
    #[test]
    fn t1110_list_unsubscribe_any_string(value in any::<String>()) {
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }

    /// Property 1 over lossy bytes (unpaired surrogates, lone continuation
    /// bytes, NULs): still no panic, still safe targets.
    #[test]
    fn t1110_list_unsubscribe_lossy_bytes(value in hostile_bytes()) {
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }

    /// Property 2: a candidate wrapped in `<...>` is accepted only when it is
    /// an `https` URL with a host and no userinfo, or a validated `mailto:`.
    #[test]
    fn t1110_list_unsubscribe_bracketed_candidate_never_unsafe(
        scheme in prop_oneof![Just("https"), Just("http"), Just("ftp"), Just("javascript"),
                              Just("file"), Just("data"), Just("HTTPS"), Just("")],
        rest in "[A-Za-z0-9./:@%_?=&-]{0,80}",
    ) {
        let value = format!("<{scheme}:{rest}>");
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }

    /// Property 2: an `https` candidate is handed back only with a host and no
    /// userinfo; whatever the generated host or path.
    #[test]
    fn t1110_list_unsubscribe_https_is_bare(
        host in "[A-Za-z0-9.-]{0,60}",
        userinfo in prop::option::of("[A-Za-z0-9._%:@-]{1,20}"),
        path in "[A-Za-z0-9./?=&%_~-]{0,60}",
    ) {
        let value = match &userinfo {
            Some(u) => format!("<https://{u}@{host}/{path}>"),
            None => format!("<https://{host}/{path}>"),
        };
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }

    /// Property 2: a `mailto:` candidate survives only when the mailto parser
    /// accepts it; a forbidden parameter or a second recipient is dropped.
    #[test]
    fn t1110_list_unsubscribe_mailto_matches_validator(
        local in "[A-Za-z0-9._%+-]{0,40}",
        domain in "[A-Za-z0-9.-]{0,40}",
        query in "[A-Za-z0-9./:@%_=+&?-]{0,60}",
    ) {
        let value = format!("<mailto:{local}@{domain}?{query}>");
        let got = parse_list_unsubscribe(&value);
        assert_targets_valid(&got);
    }
}

// ---------------------------------------------------------------------------
// Deterministic hostile fixtures (S10 5 "Hostile content").
// ---------------------------------------------------------------------------

/// Property 2 named cases: the schemes and userinfo the task lists are never
/// accepted, and the accepted kinds parse cleanly.
#[test]
fn t1110_list_unsubscribe_drops_forbidden_schemes_and_userinfo() {
    let dropped = [
        "<http://u.example.com/x>",
        "<javascript:alert(1)>",
        "<file:///etc/passwd>",
        "<data:text/plain,unsubscribe>",
        "<ftp://u.example.com/x>",
        "<HTTP://u.example.com/x>", // the scheme is not https
        "<https://user:pw@u.example.com/x>",
        "<https://user@u.example.com/x>",
    ];
    for value in dropped {
        let got = parse_list_unsubscribe(value);
        assert!(
            got.https.is_none() && got.mailto.is_none(),
            "must drop {value}: {got:?}"
        );
    }
    // The scheme is case-insensitive (RFC 3986), so an upper-case `HTTPS` is
    // the same target as `https` and is kept.
    let upper = parse_list_unsubscribe("<HTTPS://u.example.com/x>");
    assert_eq!(
        upper.https.as_ref().map(url::Url::scheme),
        Some("https"),
        "a case-varied https target is kept"
    );
    let kept = parse_list_unsubscribe("<https://u.example.com/x>");
    assert_eq!(
        kept.https.as_ref().map(url::Url::scheme),
        Some("https"),
        "a bare https target is kept"
    );
}

/// Property 1: a multi-megabyte header block is bounded and never panics.
#[test]
fn t1110_list_unsubscribe_huge_input() {
    let huge = "<https://u.example.com/%s>".replace("%s", &"a".repeat(1024 * 1024));
    assert!(huge.len() > 1024 * 1024);
    let got = parse_list_unsubscribe(&huge);
    // The one URI is over the per-URI cap, so it is dropped, not kept whole.
    assert!(got.https.is_none());
    // A one-megabyte run of commas and angle brackets is handled too.
    let noise = ",<".repeat(500_000);
    let _ = parse_list_unsubscribe(&noise);
}
