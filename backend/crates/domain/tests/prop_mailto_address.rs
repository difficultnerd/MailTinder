//! Property and fuzz-style tests for the strict `mailto:` and address parsers
//! (T-1110, S6 section 6, ASVS V1.3.11, S10 5 "Hostile content").
//!
//! Both parsers take untrusted text. They must always return (never panic),
//! a valid address must round-trip, and neither may ever hand back a value
//! carrying a raw CR, LF or NUL. A header-injection payload in the subject,
//! body or recipient is refused or neutralised.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{EmailAddress, MailtoError, MailtoTarget};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use proptest::prelude::*;

/// Arbitrary hostile text (NUL, controls, bidi marks, CR/LF).
fn hostile_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..200).prop_map(|chars| chars.into_iter().collect())
}

/// Arbitrary bytes, lossily converted so unpaired surrogates cannot hide.
fn hostile_bytes() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<u8>(), 0..200)
        .prop_map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// A syntactically valid address the parser must accept: a dot-atom-free local
/// part and an LDH domain whose last label is not all digits.
fn valid_address() -> impl Strategy<Value = String> {
    (
        "[A-Za-z0-9_+-]{1,30}",
        "[a-z][a-z0-9-]{0,15}[a-z0-9]",
        "[a-z]{2,6}",
    )
        .prop_map(|(local, host, tld)| format!("{local}@{host}.{tld}"))
}

proptest! {
    /// Property 1: neither parser panics for arbitrary Unicode.
    #[test]
    fn t1110_mailto_address_never_panic(input in hostile_text()) {
        let _ = EmailAddress::parse(&input);
        let _ = MailtoTarget::parse(&input);
    }

    /// Property 1 over lossy bytes (unpaired surrogates, NULs, controls).
    #[test]
    fn t1110_mailto_address_lossy_bytes_never_panic(input in hostile_bytes()) {
        let _ = EmailAddress::parse(&input);
        let _ = MailtoTarget::parse(&input);
    }

    /// Property 5: `parse(display(x)) == x` for a valid address.
    #[test]
    fn t1110_address_round_trips(address in valid_address()) {
        let parsed = EmailAddress::parse(&address).expect("generated address is valid");
        let again = EmailAddress::parse(parsed.as_str()).expect("normalised address re-parses");
        prop_assert_eq!(again.as_str(), parsed.as_str());
        prop_assert_eq!(again, parsed);
    }

    /// Property 5: a `mailto:` URI built from a valid target round-trips.
    #[test]
    fn t1110_mailto_round_trips(
        address in valid_address(),
        subject in prop::option::of("[A-Za-z0-9 _.-]{0,40}"),
        body in prop::option::of("[A-Za-z0-9 _.-]{0,40}"),
    ) {
        let target = MailtoTarget::new(&address, subject.as_deref(), body.as_deref())
            .expect("generated target is valid");
        let mut uri = format!("mailto:{}", target.to());
        let mut separator = '?';
        if let Some(s) = target.subject() {
            uri.push(separator);
            separator = '&';
            uri.push_str("subject=");
            uri.push_str(s);
        }
        if let Some(b) = target.body() {
            uri.push(separator);
            uri.push_str("body=");
            uri.push_str(b);
        }
        let parsed = MailtoTarget::parse(&uri).expect("round-trip parse");
        prop_assert_eq!(parsed.to(), target.to());
        prop_assert_eq!(parsed.subject(), target.subject());
        prop_assert_eq!(parsed.body(), target.body());
    }

    /// Property 5: neither parser ever accepts an address or target carrying a
    /// raw CR, LF or NUL.
    #[test]
    fn t1110_parsers_never_accept_raw_control(input in hostile_bytes()) {
        if let Ok(address) = EmailAddress::parse(&input) {
            prop_assert!(!address.as_str().contains(['\r', '\n', '\0']));
        }
        if let Ok(target) = MailtoTarget::parse(&input) {
            prop_assert!(!target.to().contains(['\r', '\n', '\0']));
            for text in [target.subject(), target.body()].into_iter().flatten() {
                prop_assert!(!text.contains(['\r', '\n', '\0']));
            }
        }
    }

    /// Property 5: a percent-encoded hostile payload is decoded, then either
    /// refused or handed back with every control character gone.
    #[test]
    fn t1110_mailto_encoded_payload_has_no_control(payload in hostile_text()) {
        let encoded = utf8_percent_encode(&payload, NON_ALPHANUMERIC).to_string();
        if let Ok(target) = MailtoTarget::parse(&format!("mailto:a@example.com?subject={encoded}")) {
            if let Some(subject) = target.subject() {
                prop_assert!(!subject.contains(['\r', '\n', '\0']));
            }
        }
        if let Ok(target) = MailtoTarget::parse(&format!("mailto:{encoded}@example.com")) {
            prop_assert!(!target.to().contains(['\r', '\n', '\0']));
        }
    }

    /// Property 5: a `mailto:` URI whose scheme is anything but `mailto:` is
    /// never accepted.
    #[test]
    fn t1110_non_mailto_schemes_are_refused(scheme in "[a-zA-Z]{1,12}", rest in "[A-Za-z0-9./:@%_?=&-]{0,40}") {
        if !scheme.eq_ignore_ascii_case("mailto") {
            let uri = format!("{scheme}:{rest}");
            prop_assert!(MailtoTarget::parse(&uri).is_err());
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic hostile fixtures (S10 5 "Hostile content").
// ---------------------------------------------------------------------------

/// Property 5: an address with a CR, LF or NUL, or a mixed-script (non-ASCII)
/// local part, is refused.
#[test]
fn t1110_address_refuses_control_and_mixed_script() {
    let refused = [
        "a\r@example.com",
        "a\n@example.com",
        "a\0@example.com",
        "a@exa\r\nmple.com",
        "a\u{0430}pple@example.com", // Cyrillic 'а' lookalike
        "tаb@example.com",           // mixed script in the local part
    ];
    for input in refused {
        assert!(EmailAddress::parse(input).is_err(), "must refuse {input:?}");
    }
}

/// Property 5: header-injection payloads in the subject, body or recipient are
/// refused (the control check) rather than carried through.
#[test]
fn t1110_mailto_refuses_injection_fixtures() {
    let refused = [
        // A decoded CR/LF in the subject is a control character.
        "mailto:a@example.com?subject=hello%0D%0AX-Evil:%201",
        // A decoded CR/LF in the body is a control character.
        "mailto:a@example.com?body=bye%0ABcc:%20victim@example.com",
        // A raw CR/LF in the recipient path.
        "mailto:a@example.com\r\nX-Evil: 1",
        // A second recipient.
        "mailto:a@example.com,b@example.com",
        // A forbidden header parameter.
        "mailto:a@example.com?cc=victim@example.com",
        "mailto:a@example.com?bcc=victim@example.com",
        // A duplicated field.
        "mailto:a@example.com?subject=one&subject=two",
        // A field with no `=`.
        "mailto:a@example.com?subject",
        // Bad percent encoding.
        "mailto:a@example.com?body=%FF",
    ];
    for input in refused {
        assert!(
            MailtoTarget::parse(input).is_err(),
            "must refuse {input:?}: {:?}",
            MailtoTarget::parse(input)
        );
    }
}

/// A valid target still parses, and a forbidden parameter reports its reason.
#[test]
fn t1110_mailto_valid_target_and_error_kinds() {
    let target = MailtoTarget::parse("mailto:Unsub@Example.COM?subject=stop&body=please")
        .expect("valid mailto");
    assert_eq!(target.to(), "Unsub@example.com");
    assert_eq!(target.subject(), Some("stop"));
    assert_eq!(target.body(), Some("please"));
    assert_eq!(
        MailtoTarget::parse("mailto:a@example.com?cc=x@example.com"),
        Err(MailtoError::FieldNotAllowed)
    );
    assert_eq!(
        MailtoTarget::parse("mailto:a@example.com?subject=x%0D%0Ay"),
        Err(MailtoError::ControlCharacter)
    );
}
