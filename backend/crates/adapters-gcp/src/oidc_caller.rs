//! The real OIDC caller check for internal routes (T-701).
//!
//! Cloud Tasks calls internal routes with a Google-signed OIDC ID token. This
//! adapter verifies the RS256 signature against Google's published JWKS (the
//! only key source), the issuer, the audience and the expiry. The JWKS is
//! fetched through [`HttpEgress`] — the `unsub` allowlist pins
//! `www.googleapis.com/oauth2/v3/certs` — and cached for one hour.
//!
//! The bearer token never reaches a log line: every failure is a typed error,
//! and only the check's outcome is logged by the caller.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use obs::Sensitive;
use ports::{
    CallerAuthError, CallerVerifier, Clock, EgressRequest, HttpEgress, HttpMethod, VerifiedCaller,
};
use serde::Deserialize;
use time::OffsetDateTime;
use url::Url;

/// The pinned JWKS URL; keys come from here and nowhere else (V9.1.3).
pub const GOOGLE_JWKS_URI: &str = "https://www.googleapis.com/oauth2/v3/certs";
/// The only accepted issuers (V10.5.3).
pub const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
/// The clock skew applied to `exp` `[DEFAULT]`.
pub const CLOCK_SKEW_S: i64 = 60;
/// How long the JWKS is cached `[DEFAULT]`.
pub const JWKS_MAX_CACHE_S: i64 = 3600;
/// The per-call timeout `[DEFAULT]`.
pub const OIDC_TIMEOUT_S: u64 = 10;

/// The cached JWKS keys and when they were fetched.
struct JwksCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<OffsetDateTime>,
}

/// Verifies Google-signed OIDC ID tokens against Google's pinned keys.
pub struct GoogleCallerVerifier {
    egress: Arc<dyn HttpEgress>,
    clock: Arc<dyn Clock>,
    jwks: RwLock<JwksCache>,
}

impl GoogleCallerVerifier {
    /// Build the verifier. In production the JWKS URL is the constant above.
    pub fn new(egress: Arc<dyn HttpEgress>, clock: Arc<dyn Clock>) -> Self {
        Self {
            egress,
            clock,
            jwks: RwLock::new(JwksCache {
                keys: HashMap::new(),
                fetched_at: None,
            }),
        }
    }

    fn cache_read(&self) -> RwLockReadGuard<'_, JwksCache> {
        self.jwks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cache_write(&self) -> RwLockWriteGuard<'_, JwksCache> {
        self.jwks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The signing key for `kid`, refetching the pinned JWKS when the cache is
    /// empty, stale, or does not hold `kid`.
    async fn key_for(&self, kid: &str) -> Result<DecodingKey, CallerAuthError> {
        let now = self.clock.now();
        {
            let cache = self.cache_read();
            if !is_stale(cache.fetched_at, now) {
                if let Some(key) = cache.keys.get(kid) {
                    return Ok(key.clone());
                }
            }
        }
        let keys = self.fetch_jwks().await?;
        let found = keys.get(kid).cloned();
        {
            let mut cache = self.cache_write();
            cache.keys = keys;
            cache.fetched_at = Some(now);
        }
        found.ok_or(CallerAuthError::BadSignature)
    }

    /// Fetch and parse the pinned JWKS.
    async fn fetch_jwks(&self) -> Result<HashMap<String, DecodingKey>, CallerAuthError> {
        let url = Url::parse(GOOGLE_JWKS_URI).map_err(|_| CallerAuthError::KeysUnavailable)?;
        let req = EgressRequest {
            method: HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(OIDC_TIMEOUT_S),
        };
        let resp = self
            .egress
            .call(req)
            .await
            .map_err(|_| CallerAuthError::KeysUnavailable)?;
        if !(200..300).contains(&resp.status) {
            return Err(CallerAuthError::KeysUnavailable);
        }
        let doc: JwksDocument =
            serde_json::from_slice(&resp.body).map_err(|_| CallerAuthError::KeysUnavailable)?;
        let mut keys = HashMap::new();
        for jwk in doc.keys {
            if jwk.kty != "RSA" || jwk.alg.as_deref().is_some_and(|a| a != "RS256") {
                continue;
            }
            if let (Some(n), Some(e)) = (jwk.n.as_deref(), jwk.e.as_deref()) {
                if let Ok(key) = DecodingKey::from_rsa_components(n, e) {
                    keys.insert(jwk.kid, key);
                }
            }
        }
        if keys.is_empty() {
            return Err(CallerAuthError::KeysUnavailable);
        }
        Ok(keys)
    }
}

#[async_trait]
impl CallerVerifier for GoogleCallerVerifier {
    async fn verify(
        &self,
        bearer: &Sensitive<String>,
        audience: &str,
    ) -> Result<VerifiedCaller, CallerAuthError> {
        let token = bearer.expose();
        let header = jsonwebtoken::decode_header(token).map_err(|_| CallerAuthError::Malformed)?;
        if header.alg != Algorithm::RS256 {
            return Err(CallerAuthError::BadSignature);
        }
        let kid = header.kid.ok_or(CallerAuthError::Malformed)?;
        let key = self.key_for(&kid).await?;

        // Signature only; claims are checked against the injected clock below.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        let data = jsonwebtoken::decode::<CallerClaims>(token, &key, &validation)
            .map_err(|_| CallerAuthError::BadSignature)?;
        let claims = data.claims;

        if !GOOGLE_ISSUERS.contains(&claims.iss.as_str()) {
            return Err(CallerAuthError::WrongIssuer);
        }
        if !claims.aud.matches(audience) {
            return Err(CallerAuthError::WrongAudience);
        }
        let now_s = self.clock.now().unix_timestamp();
        if claims.exp <= now_s - CLOCK_SKEW_S {
            return Err(CallerAuthError::Expired);
        }
        let email = claims.email.ok_or(CallerAuthError::Malformed)?;
        Ok(VerifiedCaller {
            email,
            email_verified: claims.email_verified.unwrap_or(false),
        })
    }
}

/// True when the cache is empty or older than [`JWKS_MAX_CACHE_S`].
fn is_stale(fetched_at: Option<OffsetDateTime>, now: OffsetDateTime) -> bool {
    match fetched_at {
        None => true,
        Some(t) => now - t >= time::Duration::seconds(JWKS_MAX_CACHE_S),
    }
}

/// The `aud` claim: a single string or an array of strings.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn matches(&self, expected: &str) -> bool {
        match self {
            Audience::One(a) => a == expected,
            Audience::Many(list) => list.iter().any(|a| a == expected),
        }
    }
}

/// The ID token claims read by the caller check. No `Debug`: the `email`
/// claim must not reach a log.
#[derive(Deserialize)]
struct CallerClaims {
    iss: String,
    aud: Audience,
    exp: i64,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}
