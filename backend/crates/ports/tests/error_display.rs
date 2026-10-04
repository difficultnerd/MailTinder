//! Coverage for the thiserror `Display` impls on the port error enums.

use ports::{
    ClassifierError, EgressError, IdError, KeyError, MailError, RefusedRange, SchedError,
    SecretError,
};

#[test]
fn egress_error_display() {
    assert_eq!(EgressError::SchemeNotAllowed.to_string(), "scheme not allowed");
    assert_eq!(EgressError::CredentialsInUrl.to_string(), "credentials in url");
    assert_eq!(EgressError::PortNotAllowed.to_string(), "port not allowed");
    assert_eq!(EgressError::IpLiteralHost.to_string(), "ip literal host");
    assert_eq!(EgressError::HostNotAllowed.to_string(), "host not allowed");
    assert_eq!(
        EgressError::AddressRefused(RefusedRange::Private).to_string(),
        "address refused: Private"
    );
    // nosemgrep: mailtinder-no-permanent-delete -- asserting the Display string of an error variant, not a delete operation
    assert_eq!(EgressError::PermanentDeleteRefused.to_string(), "permanent delete refused");
    assert_eq!(EgressError::NotPermitted.to_string(), "not permitted for this service");
    assert_eq!(EgressError::DnsFailed.to_string(), "dns failure");
    assert_eq!(EgressError::Connect.to_string(), "connect failure");
    assert_eq!(EgressError::Tls.to_string(), "tls failure");
    assert_eq!(EgressError::Timeout.to_string(), "timeout");
    assert_eq!(EgressError::ResponseTooLarge.to_string(), "response too large");
}

#[test]
fn key_error_display() {
    assert_eq!(KeyError::Unavailable.to_string(), "key service unavailable");
    assert_eq!(KeyError::OpenFailed.to_string(), "open failed");
    assert_eq!(KeyError::Malformed.to_string(), "malformed ciphertext");
    assert_eq!(KeyError::UnsupportedVersion(2).to_string(), "unsupported scheme version 2");
    assert_eq!(KeyError::Denied.to_string(), "denied");
}

#[test]
fn id_error_display() {
    assert_eq!(IdError::InvalidGrant.to_string(), "invalid grant");
    assert_eq!(IdError::InvalidIdToken("x").to_string(), "invalid id token: x");
    assert_eq!(IdError::AccessDenied.to_string(), "access denied");
    assert_eq!(IdError::Unavailable.to_string(), "identity provider unavailable");
}

#[test]
fn secret_error_display() {
    assert_eq!(SecretError::Unavailable.to_string(), "secret unavailable");
    assert_eq!(SecretError::Denied.to_string(), "secret denied");
    assert_eq!(SecretError::Missing.to_string(), "secret missing");
}

#[test]
fn mail_error_display() {
    assert_eq!(MailError::Unauthorized.to_string(), "unauthorized");
    assert_eq!(MailError::Forbidden.to_string(), "forbidden");
    assert_eq!(MailError::NotFound.to_string(), "not found");
    assert_eq!(MailError::RateLimited { retry_after_s: 3 }.to_string(), "rate limited");
    assert_eq!(MailError::Transient.to_string(), "transient");
    assert_eq!(MailError::Invalid("bad".into()).to_string(), "invalid: bad");
}

#[test]
fn classifier_error_display() {
    assert_eq!(ClassifierError::Timeout.to_string(), "timeout");
    assert_eq!(ClassifierError::Http(500).to_string(), "http 500");
    assert_eq!(ClassifierError::InvalidOutput.to_string(), "invalid output");
    assert_eq!(ClassifierError::Disabled.to_string(), "disabled");
    assert_eq!(ClassifierError::Unavailable.to_string(), "unavailable");
}

#[test]
fn sched_error_display() {
    assert_eq!(SchedError::Unavailable.to_string(), "scheduler unavailable");
    assert_eq!(SchedError::Rejected("x").to_string(), "rejected: x");
}
