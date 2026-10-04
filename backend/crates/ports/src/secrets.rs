//! The `Secrets` port: named secrets held outside the code.

use async_trait::async_trait;
use obs::Sensitive;

/// A named secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretName {
    GoogleOAuthClientSecret,
    JevApiKey,
    EmailLookupHmacKey,
    LogPseudonymHmacKey,
}

impl SecretName {
    pub fn secret_id(self) -> &'static str {
        match self {
            SecretName::GoogleOAuthClientSecret => "google-oauth-client-secret",
            SecretName::JevApiKey => "jev-api-key",
            SecretName::EmailLookupHmacKey => "email-lookup-hmac-key",
            SecretName::LogPseudonymHmacKey => "log-pseudonym-hmac-key",
        }
    }
}

/// A secrets error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret unavailable")]
    Unavailable,
    #[error("secret denied")]
    Denied,
    #[error("secret missing")]
    Missing,
}

/// The secrets boundary.
#[async_trait]
pub trait Secrets: Send + Sync {
    async fn get(&self, name: SecretName) -> Result<Sensitive<Vec<u8>>, SecretError>;
}
