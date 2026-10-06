//! The caller check for internal routes (S7 5.12 API-INT-1, ASVS V16.3.2).
//!
//! Cloud Tasks calls every internal route with a Google-signed OIDC ID token
//! in `Authorization: Bearer <token>`. The rules are the same in every
//! service: the token must verify against Google's keys for the route's
//! audience and its `email` must equal the one Cloud Tasks service account.
//! A missing, malformed or wrong token is a `401`; nothing else is inspected
//! before this check passes.

use obs::Sensitive;
use ports::{CallerAuthError, CallerVerifier};

/// The audience and caller a service's internal routes require. Loaded from
/// environment (`UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`). `Debug` is
/// hand-written: the allowed caller address must not reach a log (S5).
#[derive(Clone, PartialEq, Eq)]
pub struct InternalAuthConfig {
    /// The expected `aud` claim of the ID token.
    pub audience: String,
    /// The only `email` allowed to call the route.
    pub allowed_caller_email: String,
}

#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for InternalAuthConfig {
    // The allowed caller address is deliberately omitted (S5).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InternalAuthConfig")
            .field("audience", &self.audience)
            .finish()
    }
}

/// Why an internal caller was refused. Every variant maps to `401`.
#[derive(Debug, thiserror::Error)]
pub enum InternalAuthError {
    /// No `Authorization` header, or no bearer token in it.
    #[error("missing")]
    Missing,
    /// The token did not verify for any reason.
    #[error("invalid")]
    Invalid(CallerAuthError),
    /// The token verified but belongs to the wrong caller.
    #[error("caller")]
    WrongCaller,
}

/// Verify the `Authorization` header of an internal request. The bearer token
/// is never logged: only the outcome of the check is.
pub async fn verify_internal_caller(
    verifier: &dyn CallerVerifier,
    cfg: &InternalAuthConfig,
    authorization_header: Option<&str>,
) -> Result<(), InternalAuthError> {
    let header = authorization_header.ok_or(InternalAuthError::Missing)?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or(InternalAuthError::Missing)?;
    if token.is_empty() {
        return Err(InternalAuthError::Missing);
    }
    let caller = verifier
        .verify(&Sensitive::new(token.to_owned()), &cfg.audience)
        .await
        .map_err(InternalAuthError::Invalid)?;
    if !caller.email_verified || caller.email != cfg.allowed_caller_email {
        return Err(InternalAuthError::WrongCaller);
    }
    Ok(())
}
