//! The `IdentityProvider` port: OAuth/OIDC sign-in.

use std::fmt;

use async_trait::async_trait;
use obs::Sensitive;
use time::OffsetDateTime;
use url::Url;

/// An OAuth prompt value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    Login,
    Consent,
    SelectAccount,
}

/// An authorization request. Debug prints only non-sensitive fields.
pub struct AuthRequest {
    pub state: String,
    pub nonce: String,
    /// base64url(SHA-256(verifier)), method S256 only.
    pub code_challenge: String,
    pub redirect_uri: Url,
    pub scopes: Vec<&'static str>,
    pub prompt: Option<Prompt>,
    pub max_age_s: Option<u32>,
    pub login_hint: Option<Sensitive<String>>,
}

impl fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthRequest")
            .field("state", &self.state)
            .field("nonce", &self.nonce)
            .field("code_challenge", &self.code_challenge)
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .field("prompt", &self.prompt)
            .field("max_age_s", &self.max_age_s)
            .field("login_hint", &self.login_hint)
            .finish()
    }
}

/// A token set from the provider. Debug prints field names only.
pub struct TokenSet {
    pub access_token: Sensitive<String>,
    pub refresh_token: Option<Sensitive<String>>,
    /// Raw JWT, validated with `validate_id_token`.
    pub id_token: Sensitive<String>,
    pub expires_in_s: u64,
    pub granted_scopes: Vec<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &self.refresh_token.is_some())
            .field("id_token", &"[redacted]")
            .field("expires_in_s", &self.expires_in_s)
            .field("granted_scopes", &self.granted_scopes)
            .finish()
    }
}

/// Claims from a validated ID token. Debug prints only non-sensitive fields.
pub struct IdClaims {
    pub sub: String,
    pub email: Sensitive<String>,
    pub email_verified: bool,
    pub auth_time: Option<OffsetDateTime>,
    pub amr: Vec<String>,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

impl fmt::Debug for IdClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdClaims")
            .field("sub", &self.sub)
            .field("email", &"[redacted]")
            .field("email_verified", &self.email_verified)
            .field("auth_time", &self.auth_time)
            .field("amr", &self.amr)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// An identity provider error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("invalid grant")]
    InvalidGrant,
    #[error("invalid id token: {0}")]
    InvalidIdToken(&'static str),
    #[error("access denied")]
    AccessDenied,
    #[error("identity provider unavailable")]
    Unavailable,
}

/// The OAuth/OIDC boundary.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    fn authorize_url(&self, req: &AuthRequest) -> Url;
    async fn exchange(
        &self,
        code: &str,
        verifier: &Sensitive<String>,
        redirect_uri: &Url,
    ) -> Result<TokenSet, IdError>;
    async fn validate_id_token(
        &self,
        raw: &Sensitive<String>,
        nonce: &str,
    ) -> Result<IdClaims, IdError>;
    /// Returns a fresh access token.
    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError>;
    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError>;
}
