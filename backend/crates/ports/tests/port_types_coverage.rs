//! Coverage for the redacting Debug impls and small helpers in the port types.

use obs::Sensitive;
use ports::keys::WrappedKey;
use ports::{AuthRequest, ClassifierInput, IdClaims, Prompt, SecretName, TokenSet};
use time::OffsetDateTime;
use url::Url;

#[test]
fn classifier_input_debug_redacts() {
    let input = ClassifierInput {
        from_display: "S".into(),
        from_domain: "example.com".into(),
        list_id: None,
        has_list_unsubscribe: true,
        has_list_unsubscribe_post: false,
        precedence: None,
        auto_submitted: None,
        esp_header_names: vec![],
        auth_summary: String::new(),
        subject: "subj".into(),
        text: "body".into(),
        input_version: "v1",
    };
    assert_eq!(format!("{input:?}"), "ClassifierInput { .. }");
}

#[test]
fn wrapped_key_deserialize_base64url() {
    let wk: WrappedKey = serde_json::from_str("\"AAEC\"").unwrap_or_else(|_| panic!("parse"));
    assert_eq!(wk.0, vec![0, 1, 2]);
    // Invalid base64url fails.
    assert!(serde_json::from_str::<WrappedKey>("\"!!!\"").is_err());
}

#[test]
fn secret_name_secret_id() {
    assert_eq!(
        SecretName::GoogleOAuthClientSecret.secret_id(),
        "google-oauth-client-secret"
    );
    assert_eq!(SecretName::JevApiKey.secret_id(), "jev-api-key");
    assert_eq!(
        SecretName::EmailLookupHmacKey.secret_id(),
        "email-lookup-hmac-key"
    );
    assert_eq!(
        SecretName::LogPseudonymHmacKey.secret_id(),
        "log-pseudonym-hmac-key"
    );
}

#[test]
fn auth_request_debug_redacts() {
    let req = AuthRequest {
        state: "st".into(),
        nonce: "n".into(),
        code_challenge: "cc".into(),
        redirect_uri: Url::parse("https://mailtinder.app/cb").unwrap_or_else(|_| panic!("url")),
        scopes: vec!["openid"],
        prompt: Some(Prompt::Consent),
        max_age_s: Some(60),
        login_hint: Some(Sensitive::new("user@example.com".into())),
    };
    let s = format!("{req:?}");
    assert!(s.contains("AuthRequest"));
    assert!(!s.contains("user@example.com"));
}

#[test]
fn token_set_debug_redacts() {
    let ts = TokenSet {
        access_token: Sensitive::new("at".into()),
        refresh_token: Some(Sensitive::new("rt".into())),
        id_token: Sensitive::new("it".into()),
        expires_in_s: 3600,
        granted_scopes: vec!["openid".into()],
    };
    let s = format!("{ts:?}");
    assert!(s.contains("TokenSet"));
    assert!(!s.contains("at") || !s.contains("rt"));
}

#[test]
fn id_claims_debug_redacts_email() {
    let claims = IdClaims {
        sub: "sub-1".into(),
        email: Sensitive::new("user@example.com".into()),
        email_verified: true,
        auth_time: None,
        amr: vec!["pwd".into()],
        issued_at: OffsetDateTime::UNIX_EPOCH,
        expires_at: OffsetDateTime::UNIX_EPOCH,
    };
    let s = format!("{claims:?}");
    assert!(s.contains("IdClaims"));
    assert!(!s.contains("user@example.com"));
}
