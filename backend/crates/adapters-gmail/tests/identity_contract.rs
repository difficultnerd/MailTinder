//! Contract tests for the Google identity adapter against `fake-google`
//! (T-502a). Every call goes through the trait; the fake's `TokenScenario`
//! switches produce each bad token S10 7.2 needs.
//!
//! The browser authorisation step is played by the test itself (a real `GET`),
//! because the adapter only builds that URL; the token, revoke and JWKS calls
//! go through a test egress that allows only the fake's address.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines
)]

use std::sync::Arc;

use adapters_gmail::identity::{GoogleIdentity, GoogleIdentityConfig};
use adapters_gmail::pkce::{challenge_for, new_pkce, new_state_or_nonce};
use adapters_gmail::scopes::{GMAIL_SCOPES, STEP_UP_SCOPES};
use fake_google::oidc::KID;
use fake_google::scenario::{LoginOutcome, NextLogin, TokenScenario};
use fake_google::{ClientReg, FakeGoogle, FakeGoogleHandle};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use obs::Sensitive;
use ports::{AuthRequest, Clock, IdClaims, IdError, IdentityProvider, TokenSet};
use testkit::clock::VirtualClock;
use testkit::egress::FakeHttpEgress;
use testkit::SeededRng;
use time::Duration as TimeDuration;
use url::Url;

const CLIENT_ID: &str = "fake-client.apps.example.test";
const CLIENT_SECRET: &str = "test-client-secret";
const REDIRECT: &str = "http://127.0.0.1:8123/callback";
const NONCE: &str = "nonce-under-test";

/// The test-only RSA key `fake-google` signs its ID tokens with.
const MAIN_KEY: &[u8] =
    include_bytes!("../../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pem");

/// A fake-google plus an adapter pointed at it.
struct Harness {
    handle: FakeGoogleHandle,
    identity: GoogleIdentity,
    egress: Arc<FakeHttpEgress>,
    clock: Arc<VirtualClock>,
}

fn redirect() -> Url {
    Url::parse(REDIRECT).expect("redirect url")
}

/// Format any error for a `Result<(), String>` test.
fn s<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}

async fn harness() -> Harness {
    let clock = Arc::new(VirtualClock::new(testkit::T0));
    let handle = FakeGoogle::start(clock.clone()).await.expect("start");
    handle.register_client(ClientReg {
        client_id: CLIENT_ID.to_owned(),
        client_secret: CLIENT_SECRET.to_owned(),
        redirect_uris: vec![REDIRECT.to_owned()],
    });
    let base = handle.base_url();
    let cfg = GoogleIdentityConfig {
        client_id: CLIENT_ID.to_owned(),
        client_secret: Sensitive::new(CLIENT_SECRET.to_owned()),
        auth_endpoint: base.join("o/oauth2/v2/auth").expect("auth"),
        token_endpoint: base.join("token").expect("token"),
        revoke_endpoint: base.join("revoke").expect("revoke"),
        jwks_uri: base.join("oauth2/v3/certs").expect("jwks"),
    };
    let egress = Arc::new(FakeHttpEgress::new());
    egress.forward("127.0.0.1", handle.addr);
    let identity = GoogleIdentity::new(cfg, egress.clone(), clock.clone());
    Harness {
        handle,
        identity,
        egress,
        clock,
    }
}

/// The default scripted login.
fn login() -> NextLogin {
    NextLogin {
        sub: "sub-alice".to_owned(),
        email: "alice@reserved.example".to_owned(),
        email_verified: true,
        amr: None,
        auth_age_s: 0,
        outcome: LoginOutcome::Approve,
    }
}

fn auth_request(pkce: &adapters_gmail::pkce::Pkce, nonce: &str) -> AuthRequest {
    AuthRequest {
        state: new_state_or_nonce(&SeededRng::new(12)).expose().clone(),
        nonce: nonce.to_owned(),
        code_challenge: pkce.challenge.clone(),
        redirect_uri: redirect(),
        scopes: GMAIL_SCOPES.to_vec(),
        prompt: None,
        max_age_s: None,
        login_hint: None,
    }
}

/// Play the browser: GET the authorisation URL and return the `code` from the
/// redirect `Location`.
async fn browser_code(url: &Url) -> Result<String, IdError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| IdError::Unavailable)?;
    let resp = client
        .get(url.clone())
        .send()
        .await
        .map_err(|_| IdError::Unavailable)?;
    if resp.status().as_u16() != 302 {
        return Err(IdError::Unavailable);
    }
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or(IdError::Unavailable)?;
    let parsed = Url::parse(location).map_err(|_| IdError::Unavailable)?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .ok_or(IdError::Unavailable)
}

/// Run authorise + exchange and return the token set.
async fn issue(
    h: &Harness,
    scenario: Option<TokenScenario>,
    nonce: &str,
) -> Result<TokenSet, IdError> {
    h.handle.next_login(login());
    if let Some(sc) = scenario {
        h.handle.token_scenario(sc);
    }
    let pkce = new_pkce(&SeededRng::new(11));
    let req = auth_request(&pkce, nonce);
    let url = h.identity.authorize_url(&req);
    let code = browser_code(&url).await?;
    h.identity
        .exchange(&code, &pkce.verifier, &redirect())
        .await
}

/// Mint an ID token with the fake's own key, for cases the fake cannot script
/// (a `jku` header, a wrong `azp`, a string `email_verified`).
fn craft(h: &Harness, jku: Option<&str>, overrides: &[(&str, serde_json::Value)]) -> String {
    let now = h.clock.now().unix_timestamp();
    let mut claims = serde_json::json!({
        "iss": "https://accounts.google.com",
        "aud": CLIENT_ID,
        "azp": CLIENT_ID,
        "sub": "sub-alice",
        "email": "alice@reserved.example",
        "email_verified": true,
        "iat": now,
        "exp": now + 3600,
        "nonce": NONCE,
    });
    for (key, value) in overrides {
        claims[*key] = value.clone();
    }
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.to_owned());
    if let Some(uri) = jku {
        header.jku = Some(uri.to_owned());
    }
    let key = EncodingKey::from_rsa_pem(MAIN_KEY).expect("main key");
    jsonwebtoken::encode(&header, &claims, &key).expect("encode")
}

fn query_param(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// The number of JWKS fetches the adapter made.
fn certs_fetches(h: &Harness) -> usize {
    h.egress
        .records()
        .into_iter()
        .filter(|r| r.path == "/oauth2/v3/certs")
        .count()
}

// --- AU-04 AC5: only the S6 section 4 scopes are ever requested ---------------

#[tokio::test]
async fn au_04_ac5_authorize_url_requests_only_s8_scopes() -> Result<(), String> {
    let h = harness().await;
    let pkce = new_pkce(&SeededRng::new(11));
    let mut req = auth_request(&pkce, NONCE);
    req.login_hint = Some(Sensitive::new("a+b&c@reserved.example".to_owned()));
    let url = h.identity.authorize_url(&req);
    let scope = query_param(&url, "scope").ok_or("no scope")?;
    assert_eq!(scope, GMAIL_SCOPES.join(" "));
    assert_eq!(scope.split(' ').count(), 5);
    for extra in ["gmail.readonly", "calendar", "userinfo.profile"] {
        assert!(!scope.contains(extra), "unexpected scope: {extra}");
    }
    // A `login_hint` with `+` and `&` is encoded, then read back intact.
    assert_eq!(
        query_param(&url, "login_hint").as_deref(),
        Some("a+b&c@reserved.example")
    );
    Ok(())
}

#[tokio::test]
async fn au_04_ac5_step_up_requests_openid_email_only() -> Result<(), String> {
    let h = harness().await;
    let pkce = new_pkce(&SeededRng::new(11));
    let mut req = auth_request(&pkce, NONCE);
    req.scopes = STEP_UP_SCOPES.to_vec();
    let url = h.identity.authorize_url(&req);
    assert_eq!(query_param(&url, "scope").as_deref(), Some("openid email"));
    Ok(())
}

// --- AU-03 AC7: the identity is `sub`, never the email -----------------------

#[tokio::test]
async fn au_03_ac7_claims_carry_sub() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, None, NONCE).await.map_err(s)?;
    let claims = h
        .identity
        .validate_id_token(&token.id_token, NONCE)
        .await
        .map_err(s)?;
    assert_eq!(claims.sub, "sub-alice");
    assert!(!claims.sub.contains('@'));
    Ok(())
}

// --- ASVS V6.8.2 / V9.1.1: signature is checked before any claim is used ------

#[tokio::test]
async fn asvs_v6_8_2_signed_by_unknown_key_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::SignedByUnknownKey), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("signature")));
    Ok(())
}

#[tokio::test]
async fn asvs_v9_1_1_tampered_payload_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, None, NONCE).await.map_err(s)?;
    let raw = token.id_token.expose().clone();
    let mut parts: Vec<String> = raw.split('.').map(str::to_owned).collect();
    assert_eq!(parts.len(), 3);
    let mut bytes = parts[1].clone().into_bytes();
    bytes[5] = if bytes[5] == b'A' { b'B' } else { b'A' };
    parts[1] = String::from_utf8(bytes).map_err(s)?;
    let tampered = Sensitive::new(parts.join("."));
    let res = h.identity.validate_id_token(&tampered, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("signature")));
    Ok(())
}

// --- ASVS V9.1.2: algorithm fixed to RS256 -----------------------------------

#[tokio::test]
async fn asvs_v9_1_2_alg_none_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::AlgNone), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("alg")));
    Ok(())
}

#[tokio::test]
async fn asvs_v9_1_2_alg_hs256_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::AlgHs256), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("alg")));
    Ok(())
}

// --- ASVS V9.1.3: keys only from the pinned JWKS; `jku` ignored --------------

#[tokio::test]
async fn asvs_v9_1_3_no_kid_refused_and_jku_ignored() -> Result<(), String> {
    let h = harness().await;
    // No `kid`: refused before any key is looked up.
    let token = issue(&h, Some(TokenScenario::NoKid), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("signature")));

    // A `jku` pointing elsewhere is ignored: the token still validates against
    // the pinned JWKS and no request leaves the fake's host.
    let crafted = craft(&h, Some("http://evil.example.test/keys"), &[]);
    let claims = h
        .identity
        .validate_id_token(&Sensitive::new(crafted), NONCE)
        .await
        .map_err(s)?;
    assert_eq!(claims.sub, "sub-alice");
    for record in h.egress.records() {
        assert_eq!(record.host, "127.0.0.1", "unexpected host reached");
    }
    assert!(h.egress.violations().is_empty(), "a host was refused");
    Ok(())
}

// --- ASVS V9.2.1: `exp` and `nbf` checked with the injected clock ------------

#[tokio::test]
async fn asvs_v9_2_1_expired_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::Expired), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("exp")));
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_1_not_yet_valid_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::NotYetValid), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("exp")));
    Ok(())
}

// --- ASVS V9.2.3 / V10.5.4: `aud` and `azp` equal our client id --------------

#[tokio::test]
async fn asvs_v9_2_3_wrong_aud_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::WrongAud), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("aud")));
    Ok(())
}

#[tokio::test]
async fn asvs_v10_5_4_azp_mismatch_refused() -> Result<(), String> {
    let h = harness().await;
    let crafted = craft(&h, None, &[("azp", serde_json::json!("other-client"))]);
    let res = h
        .identity
        .validate_id_token(&Sensitive::new(crafted), NONCE)
        .await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("aud")));
    Ok(())
}

// --- ASVS V10.2.1: PKCE S256 and `state` on every authorisation request ------

#[tokio::test]
async fn asvs_v10_2_1_authorize_url_has_s256_challenge_and_state() -> Result<(), String> {
    let h = harness().await;
    let pkce = new_pkce(&SeededRng::new(11));
    let req = auth_request(&pkce, NONCE);
    let url = h.identity.authorize_url(&req);
    assert_eq!(
        query_param(&url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        query_param(&url, "code_challenge").as_deref(),
        Some(challenge_for(pkce.verifier.expose()).as_str())
    );
    assert_eq!(
        query_param(&url, "state").as_deref(),
        Some(req.state.as_str())
    );
    assert_eq!(query_param(&url, "nonce").as_deref(), Some(NONCE));
    Ok(())
}

// --- ASVS V10.5.1: `nonce` must match ----------------------------------------

#[tokio::test]
async fn asvs_v10_5_1_missing_nonce_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::MissingNonce), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("nonce")));
    Ok(())
}

#[tokio::test]
async fn asvs_v10_5_1_wrong_nonce_refused() -> Result<(), String> {
    let h = harness().await;
    // First grant records `nonce-one` as the previous nonce.
    let _first = issue(&h, None, "nonce-one").await.map_err(s)?;
    // The replay scenario reuses `nonce-one` while we expect `nonce-two`.
    let token = issue(&h, Some(TokenScenario::ReplayPreviousNonce), "nonce-two")
        .await
        .map_err(s)?;
    let res = h
        .identity
        .validate_id_token(&token.id_token, "nonce-two")
        .await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("nonce")));
    Ok(())
}

// --- ASVS V10.5.3: issuer pinned to Google's exact values --------------------

#[tokio::test]
async fn asvs_v10_5_3_wrong_iss_refused() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, Some(TokenScenario::WrongIss), NONCE)
        .await
        .map_err(s)?;
    let res = h.identity.validate_id_token(&token.id_token, NONCE).await;
    assert_eq!(res.err(), Some(IdError::InvalidIdToken("iss")));
    Ok(())
}

// --- exchange, refresh and revoke --------------------------------------------

#[tokio::test]
async fn identity_exchange_invalid_grant_maps() -> Result<(), String> {
    let h = harness().await;
    h.handle.next_login(login());
    let pkce = new_pkce(&SeededRng::new(11));
    let req = auth_request(&pkce, NONCE);
    let url = h.identity.authorize_url(&req);
    let code = browser_code(&url).await.map_err(s)?;
    h.identity
        .exchange(&code, &pkce.verifier, &redirect())
        .await
        .map_err(s)?;
    let second = h
        .identity
        .exchange(&code, &pkce.verifier, &redirect())
        .await;
    assert_eq!(second.err(), Some(IdError::InvalidGrant));
    Ok(())
}

#[tokio::test]
async fn identity_refresh_after_revoke_is_invalid_grant() -> Result<(), String> {
    let h = harness().await;
    let token = issue(&h, None, NONCE).await.map_err(s)?;
    let refresh = token.refresh_token.clone().ok_or("no refresh token")?;
    h.handle.revoke_all_for("sub-alice");
    let res = h.identity.refresh(&refresh).await;
    assert_eq!(res.err(), Some(IdError::InvalidGrant));
    Ok(())
}

#[tokio::test]
async fn identity_revoke_unknown_token_is_ok() -> Result<(), String> {
    let h = harness().await;
    h.identity
        .revoke(&Sensitive::new("not-a-real-token".to_owned()))
        .await
        .map_err(s)?;
    Ok(())
}

// --- JWKS cache ---------------------------------------------------------------

#[tokio::test]
async fn identity_jwks_cached_then_refetched_after_max_age() -> Result<(), String> {
    let h = harness().await;
    let first = issue(&h, None, NONCE).await.map_err(s)?;
    let _ = h
        .identity
        .validate_id_token(&first.id_token, NONCE)
        .await
        .map_err(s)?;
    assert_eq!(certs_fetches(&h), 1);
    // The second validation reuses the cached keys.
    let _ = h
        .identity
        .validate_id_token(&first.id_token, NONCE)
        .await
        .map_err(s)?;
    assert_eq!(certs_fetches(&h), 1);
    // Past `JWKS_MAX_CACHE_S` the next validation refetches.
    h.clock.advance(TimeDuration::seconds(3601));
    let second = issue(&h, None, NONCE).await.map_err(s)?;
    let claims: IdClaims = h
        .identity
        .validate_id_token(&second.id_token, NONCE)
        .await
        .map_err(s)?;
    assert_eq!(claims.sub, "sub-alice");
    assert_eq!(certs_fetches(&h), 2);
    Ok(())
}

// --- claim coercion -----------------------------------------------------------

#[tokio::test]
async fn identity_email_verified_string_true_accepted() -> Result<(), String> {
    let h = harness().await;
    let crafted = craft(&h, None, &[("email_verified", serde_json::json!("true"))]);
    let claims = h
        .identity
        .validate_id_token(&Sensitive::new(crafted), NONCE)
        .await
        .map_err(s)?;
    assert!(claims.email_verified);
    Ok(())
}
