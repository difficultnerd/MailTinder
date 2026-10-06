//! The real Google `IdentityProvider`: OAuth 2.0 authorisation code flow with
//! PKCE, ID token validation against Google's pinned keys and issuer, refresh
//! and revoke (T-502a).
//!
//! Per-call contract (S8 for Google identity). All calls go through
//! [`HttpEgress`] with a fixed timeout; no redirects are followed. `accounts.
//! google.com` is a browser redirect only (the server just builds the URL); the
//! server itself calls `oauth2.googleapis.com` and `www.googleapis.com`.
//!
//! | Use | Endpoint | Method | Fields read | Errors |
//! | --- | --- | --- | --- | --- |
//! | Authorise (browser) | `GOOGLE_AUTH_ENDPOINT` | `GET` (built URL) | none | none |
//! | Code exchange | `GOOGLE_TOKEN_ENDPOINT` | `POST` form | `access_token`, `expires_in`, `refresh_token`, `scope`, `id_token` | `400 invalid_grant` -> `InvalidGrant`; other 4xx -> `Unavailable` (config error logged); 5xx/timeout -> `Unavailable` |
//! | Refresh | `GOOGLE_TOKEN_ENDPOINT` | `POST` form | `access_token` | same mapping |
//! | Revoke | `GOOGLE_REVOKE_ENDPOINT` | `POST` form | status only | `400 invalid_token` is `Ok` |
//! | Keys | `GOOGLE_JWKS_URI` | `GET` | `keys[].kid`, `kty`, `alg`, `n`, `e`; `Cache-Control: max-age` | fetch failure -> `Unavailable` |
//!
//! Tokens, codes, verifiers, the authorisation URL and Google error bodies
//! never reach a log line.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use obs::{op_log, OpLog, Sensitive};
use ports::{
    AuthRequest, Clock, EgressRequest, EgressResponse, HttpEgress, HttpMethod, IdClaims, IdError,
    IdentityProvider, Prompt, TokenSet,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use url::{form_urlencoded, Url};

/// The browser authorisation endpoint. The server only builds this URL.
pub const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// The code-exchange and refresh endpoint.
pub const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// The revoke endpoint.
pub const GOOGLE_REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
/// The pinned JWKS URL; keys come from here and nowhere else (V9.1.3).
pub const GOOGLE_JWKS_URI: &str = "https://www.googleapis.com/oauth2/v3/certs";
/// The only accepted issuers (V10.5.3).
pub const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
/// The clock skew applied to `exp`, `nbf` and `iat` `[DEFAULT]`.
pub const CLOCK_SKEW_S: i64 = 60;
/// The longest a JWKS document is cached `[DEFAULT]`.
pub const JWKS_MAX_CACHE_S: i64 = 3600;
/// The minimum interval between unknown-`kid` refetches `[DEFAULT]`.
pub const JWKS_MIN_REFETCH_S: i64 = 300;
/// The per-call timeout `[DEFAULT]`.
pub const IDENTITY_TIMEOUT_S: u64 = 10;

/// How to build the adapter: client credentials plus the four endpoints. The
/// constants above are production values; tests point at `fake-google`.
pub struct GoogleIdentityConfig {
    /// Our OAuth client id; the expected `aud` and `azp` (V9.2.3, V10.5.4).
    pub client_id: String,
    /// The client secret, from `Secrets::GoogleOAuthClientSecret`.
    pub client_secret: Sensitive<String>,
    /// The authorisation endpoint (browser only).
    pub auth_endpoint: Url,
    /// The token endpoint (code exchange and refresh).
    pub token_endpoint: Url,
    /// The revoke endpoint.
    pub revoke_endpoint: Url,
    /// The pinned JWKS URL.
    pub jwks_uri: Url,
}

/// The cached JWKS keys and when they were fetched.
struct JwksCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<OffsetDateTime>,
    max_age_s: i64,
}

/// The real Google identity adapter. `Clone`-free; callers hold it in an `Arc`.
pub struct GoogleIdentity {
    cfg: GoogleIdentityConfig,
    egress: Arc<dyn HttpEgress>,
    clock: Arc<dyn Clock>,
    jwks: RwLock<JwksCache>,
}

impl GoogleIdentity {
    /// Build the adapter. Endpoints are the constants above in production.
    pub fn new(
        cfg: GoogleIdentityConfig,
        egress: Arc<dyn HttpEgress>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            cfg,
            egress,
            clock,
            jwks: RwLock::new(JwksCache {
                keys: HashMap::new(),
                fetched_at: None,
                max_age_s: JWKS_MAX_CACHE_S,
            }),
        }
    }

    fn cache_read(&self) -> RwLockReadGuard<'_, JwksCache> {
        self.jwks.read().unwrap_or_else(|e| e.into_inner())
    }

    fn cache_write(&self) -> RwLockWriteGuard<'_, JwksCache> {
        self.jwks.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The signing key for `kid`, refetching the pinned JWKS only as the cache
    /// policy allows (unknown `kid` refetches are rate limited, V9.1.3).
    async fn key_for(&self, kid: &str) -> Result<DecodingKey, IdError> {
        let now = self.clock.now();
        {
            let cache = self.cache_read();
            if let Some(key) = cache.keys.get(kid) {
                if !is_stale(cache.fetched_at, cache.max_age_s, now) {
                    return Ok(key.clone());
                }
            }
        }
        let (has_key, last_fetch, max_age) = {
            let cache = self.cache_read();
            (
                cache.keys.contains_key(kid),
                cache.fetched_at,
                cache.max_age_s,
            )
        };
        let stale = is_stale(last_fetch, max_age, now);
        let may_refetch =
            last_fetch.map_or(true, |t| (now - t).whole_seconds() >= JWKS_MIN_REFETCH_S);
        let need_fetch = if has_key { stale } else { may_refetch };
        if !need_fetch {
            return Err(IdError::InvalidIdToken("signature"));
        }
        let (keys, max_age) = self.fetch_jwks().await?;
        {
            let mut cache = self.cache_write();
            cache.keys = keys;
            cache.fetched_at = Some(now);
            cache.max_age_s = max_age;
        }
        let cache = self.cache_read();
        cache
            .keys
            .get(kid)
            .cloned()
            .ok_or(IdError::InvalidIdToken("signature"))
    }

    /// Fetch and parse the pinned JWKS document, honouring a shorter
    /// `Cache-Control: max-age`.
    async fn fetch_jwks(&self) -> Result<(HashMap<String, DecodingKey>, i64), IdError> {
        let req = EgressRequest {
            method: HttpMethod::Get,
            url: self.cfg.jwks_uri.clone(),
            headers: vec![(
                "Accept".to_owned(),
                Sensitive::new("application/json".to_owned()),
            )],
            body: None,
            timeout: Duration::from_secs(IDENTITY_TIMEOUT_S),
        };
        let resp = self
            .egress
            .call(req)
            .await
            .map_err(|_| IdError::Unavailable)?;
        if resp.status != 200 {
            return Err(IdError::Unavailable);
        }
        let doc: JwksDocument =
            serde_json::from_slice(&resp.body).map_err(|_| IdError::Unavailable)?;
        let max_age = cache_control_max_age(&resp.headers)
            .map_or(JWKS_MAX_CACHE_S, |m| m.clamp(1, JWKS_MAX_CACHE_S));
        let mut keys = HashMap::new();
        for key in doc.keys {
            if key.kty != "RSA" {
                continue;
            }
            if key.alg.as_deref().is_some_and(|a| a != "RS256") {
                continue;
            }
            if let Ok(decoding) = DecodingKey::from_rsa_components(&key.n, &key.e) {
                keys.insert(key.kid, decoding);
            }
        }
        Ok((keys, max_age))
    }

    /// POST a form body to `url` and return the raw response.
    async fn post_form(&self, url: Url, body: String) -> Result<EgressResponse, IdError> {
        let req = EgressRequest {
            method: HttpMethod::Post,
            url,
            headers: vec![
                (
                    "Content-Type".to_owned(),
                    Sensitive::new("application/x-www-form-urlencoded".to_owned()),
                ),
                (
                    "Accept".to_owned(),
                    Sensitive::new("application/json".to_owned()),
                ),
            ],
            body: Some(body.into_bytes()),
            timeout: Duration::from_secs(IDENTITY_TIMEOUT_S),
        };
        self.egress
            .call(req)
            .await
            .map_err(|_| IdError::Unavailable)
    }
}

#[async_trait]
impl IdentityProvider for GoogleIdentity {
    fn authorize_url(&self, req: &AuthRequest) -> Url {
        let mut url = self.cfg.auth_endpoint.clone();
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("client_id", &self.cfg.client_id);
            pairs.append_pair("redirect_uri", req.redirect_uri.as_str());
            pairs.append_pair("response_type", "code");
            pairs.append_pair("scope", &req.scopes.join(" "));
            pairs.append_pair("state", &req.state);
            pairs.append_pair("nonce", &req.nonce);
            pairs.append_pair("code_challenge", &req.code_challenge);
            pairs.append_pair("code_challenge_method", "S256");
            pairs.append_pair("access_type", "offline");
            pairs.append_pair("include_granted_scopes", "true");
            if let Some(prompt) = req.prompt {
                pairs.append_pair(
                    "prompt",
                    match prompt {
                        Prompt::Login => "login",
                        Prompt::Consent => "consent",
                        Prompt::SelectAccount => "select_account",
                    },
                );
            }
            if let Some(max_age) = req.max_age_s {
                pairs.append_pair("max_age", &max_age.to_string());
            }
            if let Some(hint) = &req.login_hint {
                pairs.append_pair("login_hint", hint.expose());
            }
        }
        url
    }

    async fn exchange(
        &self,
        code: &str,
        verifier: &Sensitive<String>,
        redirect_uri: &Url,
    ) -> Result<TokenSet, IdError> {
        let body = form_body(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier.expose()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", &self.cfg.client_id),
            ("client_secret", self.cfg.client_secret.expose()),
        ]);
        let resp = self
            .post_form(self.cfg.token_endpoint.clone(), body)
            .await?;
        let status = resp.status;
        if status == 400 {
            if token_error(&resp.body).as_deref() == Some("invalid_grant") {
                return Err(IdError::InvalidGrant);
            }
            log_config_error(status);
            return Err(IdError::Unavailable);
        }
        if !(200..300).contains(&status) {
            return Err(IdError::Unavailable);
        }
        let tr: TokenResponse =
            serde_json::from_slice(&resp.body).map_err(|_| IdError::Unavailable)?;
        let id_token = tr.id_token.ok_or(IdError::InvalidIdToken("signature"))?;
        let granted_scopes = tr
            .scope
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(TokenSet {
            access_token: Sensitive::new(tr.access_token),
            refresh_token: tr.refresh_token.map(Sensitive::new),
            id_token: Sensitive::new(id_token),
            expires_in_s: tr.expires_in,
            granted_scopes,
        })
    }

    async fn validate_id_token(
        &self,
        raw: &Sensitive<String>,
        nonce: &str,
    ) -> Result<IdClaims, IdError> {
        let token = raw.expose();
        // 1. The algorithm is fixed to RS256 whatever the token claims.
        let alg = header_alg(token);
        if alg.as_deref() != Some("RS256") {
            return Err(if alg.is_none() {
                IdError::InvalidIdToken("signature")
            } else {
                IdError::InvalidIdToken("alg")
            });
        }
        let header =
            jsonwebtoken::decode_header(token).map_err(|_| IdError::InvalidIdToken("signature"))?;
        let kid = header.kid.ok_or(IdError::InvalidIdToken("signature"))?;
        // 2. Key from the pinned JWKS only.
        let key = self.key_for(&kid).await?;
        // 3. Signature, with jsonwebtoken's own clock-based checks off.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        let data = jsonwebtoken::decode::<GoogleClaims>(token, &key, &validation)
            .map_err(|_| IdError::InvalidIdToken("signature"))?;
        let claims = data.claims;
        // 4. Claims checked against the injected clock.
        let now = self.clock.now();
        let now_s = now.unix_timestamp();
        if !GOOGLE_ISSUERS.contains(&claims.iss.as_str()) {
            return Err(IdError::InvalidIdToken("iss"));
        }
        if !aud_matches(&claims.aud, &self.cfg.client_id) {
            return Err(IdError::InvalidIdToken("aud"));
        }
        if let Some(azp) = &claims.azp {
            if azp != &self.cfg.client_id {
                return Err(IdError::InvalidIdToken("aud"));
            }
        }
        if claims.exp <= now_s - CLOCK_SKEW_S {
            return Err(IdError::InvalidIdToken("exp"));
        }
        if claims.nbf.is_some_and(|nbf| nbf > now_s + CLOCK_SKEW_S) {
            return Err(IdError::InvalidIdToken("exp"));
        }
        if claims.iat > now_s + CLOCK_SKEW_S {
            return Err(IdError::InvalidIdToken("exp"));
        }
        // 5. `nonce` must match, compared in constant time.
        match &claims.nonce {
            Some(expected) if ct_eq(expected, nonce) => {}
            _ => return Err(IdError::InvalidIdToken("nonce")),
        }
        // 6. Build the identity: `sub`, never the email (AU-03 AC7).
        if claims.sub.is_empty() || claims.sub.chars().count() > 255 {
            return Err(IdError::InvalidIdToken("signature"));
        }
        let issued_at = unix(claims.iat)?;
        let expires_at = unix(claims.exp)?;
        let auth_time = match claims.auth_time {
            Some(t) => Some(unix(t)?),
            None => None,
        };
        Ok(IdClaims {
            sub: claims.sub,
            email: Sensitive::new(claims.email.unwrap_or_default()),
            email_verified: email_verified(claims.email_verified.as_ref()),
            auth_time,
            amr: claims.amr.unwrap_or_default(),
            issued_at,
            expires_at,
        })
    }

    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError> {
        let body = form_body(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.expose()),
            ("client_id", &self.cfg.client_id),
            ("client_secret", self.cfg.client_secret.expose()),
        ]);
        let resp = self
            .post_form(self.cfg.token_endpoint.clone(), body)
            .await?;
        let status = resp.status;
        if status == 400 {
            if token_error(&resp.body).as_deref() == Some("invalid_grant") {
                return Err(IdError::InvalidGrant);
            }
            log_config_error(status);
            return Err(IdError::Unavailable);
        }
        if !(200..300).contains(&status) {
            return Err(IdError::Unavailable);
        }
        let tr: TokenResponse =
            serde_json::from_slice(&resp.body).map_err(|_| IdError::Unavailable)?;
        Ok(Sensitive::new(tr.access_token))
    }

    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError> {
        let body = form_body(&[("token", token.expose())]);
        let resp = self
            .post_form(self.cfg.revoke_endpoint.clone(), body)
            .await?;
        match resp.status {
            200..=299 => Ok(()),
            400 => {
                // `invalid_token` means the grant is already gone; that is success.
                if token_error(&resp.body).as_deref() == Some("invalid_token") {
                    Ok(())
                } else {
                    Err(IdError::Unavailable)
                }
            }
            _ => Err(IdError::Unavailable),
        }
    }
}

/// The token endpoint response.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
    scope: Option<String>,
    id_token: Option<String>,
    #[allow(dead_code)] // read by serde; the adapter does not use it
    token_type: String,
}

/// The `error` field of a Google error body.
#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
}

/// The subset of Google's ID token claims this adapter reads.
#[derive(Deserialize)]
struct GoogleClaims {
    iss: String,
    aud: serde_json::Value,
    azp: Option<String>,
    sub: String,
    email: Option<String>,
    email_verified: Option<serde_json::Value>,
    iat: i64,
    exp: i64,
    nbf: Option<i64>,
    nonce: Option<String>,
    auth_time: Option<i64>,
    amr: Option<Vec<String>>,
}

/// The JWKS document.
#[derive(Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

/// One JWK entry.
#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    alg: Option<String>,
    n: String,
    e: String,
}

/// True when the cache is empty or older than `min(max-age, JWKS_MAX_CACHE_S)`.
fn is_stale(fetched_at: Option<OffsetDateTime>, max_age_s: i64, now: OffsetDateTime) -> bool {
    match fetched_at {
        None => true,
        Some(t) => (now - t).whole_seconds() >= max_age_s.min(JWKS_MAX_CACHE_S),
    }
}

/// The `alg` string from the token's header segment, if it can be read.
fn header_alg(token: &str) -> Option<String> {
    let segment = token.split('.').next()?;
    let decoded = URL_SAFE_NO_PAD.decode(segment).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value.get("alg").and_then(|a| a.as_str()).map(str::to_owned)
}

/// True when `aud` is our client id, alone.
fn aud_matches(aud: &serde_json::Value, client_id: &str) -> bool {
    match aud {
        serde_json::Value::String(s) => s == client_id,
        serde_json::Value::Array(items) => items.len() == 1 && items[0].as_str() == Some(client_id),
        _ => false,
    }
}

/// Google reports verification as `true` or the string `"true"`.
fn email_verified(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s == "true",
        _ => false,
    }
}

/// A Unix second as a `OffsetDateTime`, or an `exp` failure when out of range.
fn unix(seconds: i64) -> Result<OffsetDateTime, IdError> {
    OffsetDateTime::from_unix_timestamp(seconds).map_err(|_| IdError::InvalidIdToken("exp"))
}

/// A constant-time comparison of two strings (V10.5.1).
fn ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// URL-encode a form body.
fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

/// The `error` string from a Google error body, if parsable.
fn token_error(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<TokenErrorBody>(body)
        .ok()
        .map(|e| e.error)
}

/// The `max-age` from a `Cache-Control` header, if present and numeric.
fn cache_control_max_age(headers: &[(String, String)]) -> Option<i64> {
    let value = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
        .map(|(_, v)| v.as_str())?;
    for part in value.split(',') {
        if let Some(rest) = part.trim().strip_prefix("max-age=") {
            return rest.trim().parse::<i64>().ok();
        }
    }
    None
}

/// Log a configuration-level failure (for example `invalid_client`). No error
/// body, client id or secret is logged.
fn log_config_error(status: u16) {
    op_log(&OpLog {
        op: "identity_config_error",
        outcome: "failure",
        status: Some(status),
        latency_ms: None,
    });
}
