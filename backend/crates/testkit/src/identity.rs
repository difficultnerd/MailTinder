//! A scripted identity provider fake.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use obs::Sensitive;
use ports::{AuthRequest, IdClaims, IdError, IdentityProvider, Prompt, TokenSet};
use url::Url;

/// A scripted identity provider.
pub struct FakeIdentityProvider {
    exchange: Mutex<VecDeque<Result<TokenSet, IdError>>>,
    claims: Mutex<VecDeque<Result<IdClaims, IdError>>>,
    refresh: Mutex<VecDeque<(String, Result<String, IdError>)>>,
    revoked: Mutex<Vec<String>>,
    authorize_calls: Mutex<usize>,
}

impl FakeIdentityProvider {
    pub fn new() -> Self {
        Self {
            exchange: Mutex::new(VecDeque::new()),
            claims: Mutex::new(VecDeque::new()),
            refresh: Mutex::new(VecDeque::new()),
            revoked: Mutex::new(Vec::new()),
            authorize_calls: Mutex::new(0),
        }
    }

    pub fn script_exchange(&self, r: Result<TokenSet, IdError>) {
        self.exchange
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .push_back(r);
    }

    pub fn script_claims(&self, r: Result<IdClaims, IdError>) {
        self.claims
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .push_back(r);
    }

    pub fn script_refresh(&self, refresh: &str, r: Result<String, IdError>) {
        self.refresh
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .push_back((refresh.to_owned(), r));
    }

    /// Raw revoked values, test only.
    pub fn revoked(&self) -> Vec<String> {
        self.revoked
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .clone()
    }

    pub fn authorize_requests(&self) -> usize {
        *self
            .authorize_calls
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
    }
}

impl Default for FakeIdentityProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IdentityProvider for FakeIdentityProvider {
    fn authorize_url(&self, req: &AuthRequest) -> Url {
        *self
            .authorize_calls
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned")) += 1;
        let mut url = Url::parse("https://accounts.example.test/o/oauth2/v2/auth")
            .unwrap_or_else(|_| panic!("url"));
        url.query_pairs_mut()
            .append_pair("state", &req.state)
            .append_pair("nonce", &req.nonce)
            .append_pair("code_challenge", &req.code_challenge)
            .append_pair("code_challenge_method", "S256");
        if let Some(p) = req.prompt {
            let v = match p {
                Prompt::Login => "login",
                Prompt::Consent => "consent",
                Prompt::SelectAccount => "select_account",
            };
            url.query_pairs_mut().append_pair("prompt", v);
        }
        if let Some(m) = req.max_age_s {
            url.query_pairs_mut().append_pair("max_age", &m.to_string());
        }
        url
    }

    async fn exchange(
        &self,
        _code: &str,
        _verifier: &Sensitive<String>,
        _redirect_uri: &Url,
    ) -> Result<TokenSet, IdError> {
        self.exchange
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .pop_front()
            .unwrap_or(Err(IdError::Unavailable))
    }

    async fn validate_id_token(
        &self,
        _raw: &Sensitive<String>,
        _nonce: &str,
    ) -> Result<IdClaims, IdError> {
        self.claims
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .pop_front()
            .unwrap_or(Err(IdError::Unavailable))
    }

    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError> {
        let mut q = self
            .refresh
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"));
        if let Some((expected, r)) = q.pop_front() {
            if &expected == refresh.expose() {
                return r.map(Sensitive::new);
            }
        }
        // A revoked value fails.
        let revoked = self
            .revoked
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"));
        if revoked.contains(refresh.expose()) {
            return Err(IdError::InvalidGrant);
        }
        Err(IdError::Unavailable)
    }

    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError> {
        self.revoked
            .lock()
            .unwrap_or_else(|_| panic!("identity poisoned"))
            .push(token.expose().clone());
        Ok(())
    }
}
