//! The `CallerVerifier` port: who may call an internal service route.
//!
//! Internal routes (S7 5.12 API-INT-1) are called by Cloud Tasks with a
//! Google-signed OIDC ID token. This port verifies that token; the check that
//! turns a verified caller into a refusal lives in
//! `svc-common::internal_auth`, so every service applies the same rules
//! (T-701, ASVS V16.3.2).

use async_trait::async_trait;
use obs::Sensitive;

/// The verified caller of an internal route. Debug prints only whether the
/// email was verified, so a service-account address cannot reach a log
/// through this struct (S5).
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedCaller {
    /// The `email` claim of the verified ID token.
    pub email: String,
    /// Whether Google marked the email verified.
    pub email_verified: bool,
}

#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for VerifiedCaller {
    // The email is deliberately omitted (S5).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedCaller")
            .field("email_verified", &self.email_verified)
            .finish()
    }
}

/// A reason an OIDC ID token was refused. Every variant maps to `401`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallerAuthError {
    #[error("malformed")]
    Malformed,
    #[error("signature")]
    BadSignature,
    #[error("issuer")]
    WrongIssuer,
    #[error("audience")]
    WrongAudience,
    #[error("expired")]
    Expired,
    #[error("keys unavailable")]
    KeysUnavailable,
}

/// Verifies a Google-signed OIDC ID token: signature against Google's keys,
/// `iss`, `aud` equal to `audience`, `exp`, returning the verified `email`.
#[async_trait]
pub trait CallerVerifier: Send + Sync {
    async fn verify(
        &self,
        bearer: &Sensitive<String>,
        audience: &str,
    ) -> Result<VerifiedCaller, CallerAuthError>;
}
