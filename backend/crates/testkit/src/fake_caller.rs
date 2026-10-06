//! A fake `CallerVerifier` that accepts tokens the test minted (T-701).
//!
//! A token is an opaque string bound to an audience and a caller; anything the
//! test did not mint is refused. No real signature is involved.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use obs::Sensitive;
use ports::{CallerAuthError, CallerVerifier, VerifiedCaller};

/// A scripted internal-caller verifier.
pub struct FakeCallerVerifier {
    tokens: Mutex<Vec<(String, String, VerifiedCaller)>>,
    next: AtomicU64,
}

impl FakeCallerVerifier {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
        }
    }

    /// Mint a token for `(audience, email)` with `email_verified` true.
    pub fn mint(&self, audience: &str, email: &str) -> String {
        self.mint_with(audience, email, true)
    }

    /// Mint a token whose `email` is not verified.
    pub fn mint_unverified(&self, audience: &str, email: &str) -> String {
        self.mint_with(audience, email, false)
    }

    fn mint_with(&self, audience: &str, email: &str, email_verified: bool) -> String {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let token = format!("fake-oidc-{n}");
        self.tokens
            .lock()
            .unwrap_or_else(|_| panic!("caller poisoned"))
            .push((
                token.clone(),
                audience.to_owned(),
                VerifiedCaller {
                    email: email.to_owned(),
                    email_verified,
                },
            ));
        token
    }
}

impl Default for FakeCallerVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CallerVerifier for FakeCallerVerifier {
    async fn verify(
        &self,
        bearer: &Sensitive<String>,
        audience: &str,
    ) -> Result<VerifiedCaller, CallerAuthError> {
        if bearer.expose().is_empty() {
            return Err(CallerAuthError::Malformed);
        }
        let tokens = self
            .tokens
            .lock()
            .unwrap_or_else(|_| panic!("caller poisoned"));
        match tokens.iter().find(|(t, _, _)| t == bearer.expose()) {
            None => Err(CallerAuthError::BadSignature),
            Some((_, token_audience, caller)) => {
                if token_audience == audience {
                    Ok(caller.clone())
                } else {
                    Err(CallerAuthError::WrongAudience)
                }
            }
        }
    }
}
