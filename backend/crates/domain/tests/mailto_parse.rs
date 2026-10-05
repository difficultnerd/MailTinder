//! Parser table and property tests for `EmailAddress` and `MailtoTarget::parse`
//! (T-404).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

use domain::{
    EmailAddress, MailtoError, MailtoTarget, MAILTO_BODY_MAX_CHARS, MAILTO_SUBJECT_MAX_CHARS,
};
use proptest::prelude::*;

/// ASVS V1.3.3: length limits are applied before the send call.
#[test]
fn asvs_v1_3_3_mailto_length_limits() {
    let subject_ok = "a".repeat(MAILTO_SUBJECT_MAX_CHARS);
    let subject_over = "a".repeat(MAILTO_SUBJECT_MAX_CHARS + 1);
    let body_ok = "a".repeat(MAILTO_BODY_MAX_CHARS);
    let body_over = "a".repeat(MAILTO_BODY_MAX_CHARS + 1);

    assert!(MailtoTarget::parse(&format!("mailto:a@b.com?subject={subject_ok}")).is_ok());
    assert_eq!(
        MailtoTarget::parse(&format!("mailto:a@b.com?subject={subject_over}")),
        Err(MailtoError::TooLong)
    );
    assert!(MailtoTarget::parse(&format!("mailto:a@b.com?body={body_ok}")).is_ok());
    assert_eq!(
        MailtoTarget::parse(&format!("mailto:a@b.com?body={body_over}")),
        Err(MailtoError::TooLong)
    );

    let huge = format!("mailto:a@b.com?body={}", "a".repeat(2100));
    assert_eq!(MailtoTarget::parse(&huge), Err(MailtoError::TooLong));
}

/// ASVS V1.3.11: CR, LF and NUL are refused in every component, after decoding.
#[test]
fn asvs_v1_3_11_mailto_rejects_cr_lf_in_any_field() {
    // In the path the decoded control makes the address invalid.
    for uri in [
        "mailto:a%0D@b.com",
        "mailto:a%0A@b.com",
        "mailto:a%00@b.com",
    ] {
        assert!(MailtoTarget::parse(uri).is_err(), "{uri} must be refused");
    }
    // In subject and body the mailto parser refuses it directly.
    for uri in [
        "mailto:a@b.com?subject=hi%0D%0ABcc:x@y.com",
        "mailto:a@b.com?subject=%0A",
        "mailto:a@b.com?body=hi%0A",
        "mailto:a@b.com?body=%00",
        "mailto:a@b.com?subject=%09",
    ] {
        assert_eq!(
            MailtoTarget::parse(uri),
            Err(MailtoError::ControlCharacter),
            "{uri} must be refused"
        );
    }
}

/// ASVS V1.3.11: only `subject` and `body` are allowed, once each.
#[test]
fn asvs_v1_3_11_mailto_rejects_cc_bcc_to_and_unknown_fields() {
    for uri in [
        "mailto:a@b.com?cc=x@y.com",
        "mailto:a@b.com?Cc=x@y.com",
        "mailto:a@b.com?bcc=x@y.com",
        "mailto:a@b.com?to=x@y.com",
        "mailto:a@b.com?in-reply-to=z",
        "mailto:a@b.com?x=1",
        "mailto:a@b.com?subject",
    ] {
        assert_eq!(
            MailtoTarget::parse(uri),
            Err(MailtoError::FieldNotAllowed),
            "{uri} must be refused"
        );
    }
    assert_eq!(
        MailtoTarget::parse("mailto:a@b.com?subject=a&subject=b"),
        Err(MailtoError::DuplicateField)
    );
    assert_eq!(
        MailtoTarget::parse("mailto:a@b.com?body=a&body=b"),
        Err(MailtoError::DuplicateField)
    );
}

/// ASVS V1.3.11: a `mailto:` target names exactly one recipient.
#[test]
fn asvs_v1_3_11_mailto_rejects_multiple_recipients() {
    for uri in [
        "mailto:a@example.com,b@example.com",
        "mailto:a@example.com%2Cb@example.com",
    ] {
        assert_eq!(
            MailtoTarget::parse(uri),
            Err(MailtoError::MultipleRecipients),
            "{uri} must be refused"
        );
    }
    // A `to=` field may not supply the recipient either: the path is empty, so
    // the target has no address at all.
    assert_eq!(
        MailtoTarget::parse("mailto:?to=a@example.com"),
        Err(MailtoError::Address)
    );
}

// ASVS V1.3.11: the parser never panics, whatever it is handed.
proptest! {
    #[test]
    fn asvs_v1_3_11_mailto_parse_never_panics(uri in "\\PC*") {
        let _ = MailtoTarget::parse(&uri);
    }
}

/// RFC 6068: `+` is a literal plus, not a space.
#[test]
fn mailto_plus_is_literal_not_space() {
    let target = MailtoTarget::parse("mailto:a@b.com?subject=a+b&body=c%2Bd").expect("valid");
    assert_eq!(target.subject(), Some("a+b"));
    assert_eq!(target.body(), Some("c+d"));
}

/// The List-Unsubscribe angle brackets are optional.
#[test]
fn mailto_angle_brackets_stripped() {
    let target = MailtoTarget::parse("<mailto:Unsub@Example.com?subject=x>").expect("valid");
    assert_eq!(target.to(), "Unsub@example.com");
    assert_eq!(target.subject(), Some("x"));

    // A lone `<` is not a mailto URI.
    assert_eq!(
        MailtoTarget::parse("<mailto:a@b.com"),
        Err(MailtoError::NotMailto)
    );
}

/// S7 API-ADM-2: the local part is kept as given, the domain lower-cased.
#[test]
fn email_address_domain_lowercased_local_kept() {
    let address = EmailAddress::parse("User.Name@Example.COM").expect("valid");
    assert_eq!(address.as_str(), "User.Name@example.com");
    assert_eq!(address.domain(), "example.com");
    assert_eq!(address.lookup_form(), "user.name@example.com");
}

/// IP literals and quoted local parts are refused.
#[test]
fn email_address_rejects_ip_literal_and_quoted_local() {
    for bad in [
        "a@[127.0.0.1]",
        "[127.0.0.1]",
        "\"a b\"@example.com",
        "a@1.2.3.4",
    ] {
        assert!(EmailAddress::parse(bad).is_err(), "{bad:?} must be refused");
    }
}

/// A stray `{:?}` never prints the address.
#[test]
fn email_address_debug_is_redacted() {
    let address = EmailAddress::parse("secret@example.com").expect("valid");
    assert_eq!(format!("{address:?}"), "[address]");
    assert!(!format!("{address:?}").contains("secret"));
}
