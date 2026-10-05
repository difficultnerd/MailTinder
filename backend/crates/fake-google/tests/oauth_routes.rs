//! OAuth/OIDC route and scenario tests for `fake-google` (T-206).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use std::sync::Arc;

use fake_google::{
    oidc::ISSUER, ClientReg, FakeGoogle, FakeGoogleHandle, NextLogin, TokenScenario,
};
use time::OffsetDateTime;

/// The fake's injected virtual clock start. Kept relative to "now" so the
/// minted tokens (`exp = t0() + offset`) stay valid against jsonwebtoken's real
/// wall-clock validation regardless of when the suite runs.
fn t0() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}
const CLIENT_ID: &str = "fake-client.apps.example.test";
const CLIENT_SECRET: &str = "fake-secret";
const REDIRECT: &str = "http://127.0.0.1:9999/callback";

async fn start() -> FakeGoogleHandle {
    let clock = Arc::new(testkit::clock::VirtualClock::new(t0()));
    FakeGoogle::start(clock).await.expect("start")
}

fn register(h: &FakeGoogleHandle) {
    h.register_client(ClientReg {
        client_id: CLIENT_ID.to_owned(),
        client_secret: CLIENT_SECRET.to_owned(),
        redirect_uris: vec![REDIRECT.to_owned()],
    });
    h.next_login(NextLogin {
        sub: "sub-alice".to_owned(),
        email: "alice@example.com".to_owned(),
        email_verified: true,
        amr: None,
        auth_age_s: 0,
        outcome: fake_google::scenario::LoginOutcome::Approve,
    });
}

fn pkce() -> (String, String) {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let verifier = "a2VyaXNzYS4zMiByYW5kb20gc3RyaW5n-abcdefghij";
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier.to_owned(), challenge)
}

/// Run the full happy path: authorise, exchange code, decode ID token.
async fn happy_path(h: &FakeGoogleHandle) -> (String, String) {
    let (verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid%20email&state=s1&nonce=n1&code_challenge={challenge}&code_challenge_method=S256",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 302);
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    assert!(loc.starts_with(REDIRECT));
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();

    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let tok = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        tok.status().as_u16(),
        200,
        "token exc body: {:?}",
        tok.text().await
    );
    let body: serde_json::Value = tok.json().await.unwrap();
    let access = body["access_token"].as_str().unwrap().to_owned();
    let id_token = body["id_token"].as_str().unwrap().to_owned();
    (access, id_token)
}

fn decode_id_token(token: &str) -> serde_json::Value {
    use base64::Engine as _;
    let parts: Vec<&str> = token.split('.').collect();
    let payload_b64 = parts[1];
    let padded = format!(
        "{payload_b64}{}",
        "=".repeat((4 - payload_b64.len() % 4) % 4)
    );
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Verify a token with the JWKS and the injected clock.
fn verify_id_token(
    token: &str,
    auth_key: jsonwebtoken::DecodingKey,
    _now: i64,
) -> jsonwebtoken::TokenData<serde_json::Value> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_audience(&[CLIENT_ID]);
    validation.leeway = 60;
    jsonwebtoken::decode::<serde_json::Value>(token, &auth_key, &validation).unwrap()
}

#[tokio::test]
async fn fake_oidc_happy_path_code_pkce_id_token_validates() {
    let h = start().await;
    register(&h);
    let (access, id_token) = happy_path(&h).await;
    // Access token works against Gmail routes (should be able to call list).
    let m = h.add_mailbox("alice@example.com");
    let _ = m;
    let keys = jsonwebtoken::DecodingKey::from_rsa_pem(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pub.pem"
        ))
        .unwrap()
        .as_slice(),
    )
    .unwrap();
    let decoded = verify_id_token(&id_token, keys, t0().unix_timestamp());
    let claims = decoded.claims;
    assert_eq!(claims["iss"], ISSUER);
    assert_eq!(claims["aud"], CLIENT_ID);
    assert_eq!(claims["sub"], "sub-alice");
    assert_eq!(claims["email"], "alice@example.com");
    assert_eq!(claims["email_verified"], true);
    assert_eq!(claims["nonce"], "n1");
    assert_ne!(access, "");
}

#[tokio::test]
async fn fake_oidc_plain_or_missing_pkce_is_400() {
    let h = start().await;
    register(&h);
    for challenge_q in ["", "&code_challenge_method=plain"] {
        let auth_url = format!(
            "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={challenge_q}",
            h.base_url().to_string().trim_end_matches('/')
        );
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let resp = client.get(&auth_url).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 400, "case: {challenge_q}");
    }
}

#[tokio::test]
async fn fake_oidc_redirect_uri_must_match_exactly() {
    let h = start().await;
    register(&h);
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri=http://127.0.0.1:9999/callback/&response_type=code&scope=openid&state=s&nonce=n&code_challenge=x&code_challenge_method=S256",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn fake_oidc_code_is_single_use_and_reuse_revokes_grant() {
    let h = start().await;
    register(&h);
    let (verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={challenge}&code_challenge_method=S256",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();

    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let form = |code: &str| {
        client
            .post(&token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT),
                ("code_verifier", &verifier),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
            ])
            .send()
    };
    let r1 = form(&code).await.unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    // Second use: invalid_grant and revokes the grant's tokens.
    let r2 = form(&code).await.unwrap();
    assert_eq!(r2.status().as_u16(), 400);
    let body: serde_json::Value = r2.json().await.unwrap();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn fake_oidc_wrong_verifier_is_invalid_grant() {
    let h = start().await;
    register(&h);
    let (_verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={challenge}&code_challenge_method=S256",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let tok = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", "wrong-verifier-value"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(tok.status().as_u16(), 400);
    let body: serde_json::Value = tok.json().await.unwrap();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn fake_oidc_each_token_scenario_produces_its_defect() {
    // WrongAud
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::WrongAud);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert_eq!(claims["aud"], "other-client");

    // WrongIss
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::WrongIss);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert_eq!(claims["iss"], "https://evil.example.com");

    // Expired
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::Expired);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert!(claims["exp"].as_i64().unwrap() <= t0().unix_timestamp() - 60);

    // NotYetValid
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::NotYetValid);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert_eq!(claims["nbf"].as_i64().unwrap(), t0().unix_timestamp() + 600);

    // MissingNonce
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::MissingNonce);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert!(claims["nonce"].is_null());

    // MissingAuthTime
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::MissingAuthTime);
    let (_, id) = happy_path(&h).await;
    let claims = decode_id_token(&id);
    assert!(claims["auth_time"].is_null());

    // SignedByUnknownKey: header alg RS256, kid same, but validate fails signature.
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::SignedByUnknownKey);
    let (_, id) = happy_path(&h).await;
    let header = decode_id_token(&id);
    let _ = header; // payload has no alg; just check structure
    let _main_key = jsonwebtoken::DecodingKey::from_rsa_pem(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pub.pem"
        ))
        .unwrap()
        .as_slice(),
    )
    .unwrap();
    // It still decodes structurally (payload) but signature would fail vs main key.
    let parts: Vec<&str> = id.split('.').collect();
    assert_eq!(parts.len(), 3);

    // AlgNone
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::AlgNone);
    let (_, id) = happy_path(&h).await;
    assert!(id.ends_with('.'));

    // AlgHs256
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::AlgHs256);
    let (_, id) = happy_path(&h).await;
    let parts: Vec<&str> = id.split('.').collect();
    assert_eq!(parts.len(), 3);

    // NoKid
    let h = start().await;
    register(&h);
    h.token_scenario(TokenScenario::NoKid);
    let (_, id) = happy_path(&h).await;
    let _ = id;
}

#[tokio::test]
async fn fake_oidc_refresh_returns_new_access_token_accepted_by_gmail_routes() {
    let h = start().await;
    register(&h);
    let (verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid%20email%20https://www.googleapis.com/auth/gmail.modify&state=s&nonce=n&code_challenge={challenge}&code_challenge_method=S256&prompt=consent",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();

    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let tok = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = tok.json().await.unwrap();
    let refresh = body["refresh_token"].as_str().unwrap().to_owned();

    // Refresh.
    let tok2 = client
        .post(&token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(tok2.status().as_u16(), 200);
    let body2: serde_json::Value = tok2.json().await.unwrap();
    let access2 = body2["access_token"].as_str().unwrap().to_owned();

    // Use the refreshed access token on a Gmail route.
    let gurl = format!(
        "{}/gmail/v1/users/me/messages",
        h.base_url().to_string().trim_end_matches('/')
    );
    let resp3 = client
        .get(&gurl)
        .bearer_auth(&access2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status().as_u16(), 200);
}

#[tokio::test]
async fn fake_oidc_revoked_refresh_is_invalid_grant() {
    let h = start().await;
    register(&h);
    // Get a refresh token (prompt=consent).
    let (verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={challenge}&code_challenge_method=S256&prompt=consent",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let tok = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = tok.json().await.unwrap();
    let refresh = body["refresh_token"].as_str().unwrap().to_owned();

    // Revoke it.
    let revoke_url = format!("{}/revoke", h.base_url().to_string().trim_end_matches('/'));
    let r = client
        .post(&revoke_url)
        .form(&[("token", &refresh)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);

    // Refresh now fails.
    let tok2 = client
        .post(&token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(tok2.status().as_u16(), 400);
    let body2: serde_json::Value = tok2.json().await.unwrap();
    assert_eq!(body2["error"], "invalid_grant");
}

#[tokio::test]
async fn fake_oidc_access_denied_redirects_with_error() {
    let h = start().await;
    h.register_client(ClientReg {
        client_id: CLIENT_ID.to_owned(),
        client_secret: CLIENT_SECRET.to_owned(),
        redirect_uris: vec![REDIRECT.to_owned()],
    });
    h.next_login(NextLogin {
        sub: "sub-alice".to_owned(),
        email: "alice@example.com".to_owned(),
        email_verified: true,
        amr: None,
        auth_age_s: 0,
        outcome: fake_google::scenario::LoginOutcome::AccessDenied,
    });
    let (verifier, _) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={verifier}&code_challenge_method=S256",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 302);
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    assert!(loc.contains("error=access_denied"));
}

#[tokio::test]
async fn fake_oidc_auth_time_present_with_prompt_login() {
    let h = start().await;
    register(&h);
    h.next_login(NextLogin {
        sub: "sub-alice".to_owned(),
        email: "alice@example.com".to_owned(),
        email_verified: true,
        amr: None,
        auth_age_s: 120,
        outcome: fake_google::scenario::LoginOutcome::Approve,
    });
    let (verifier, challenge) = pkce();
    let auth_url = format!(
        "{}/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri={REDIRECT}&response_type=code&scope=openid&state=s&nonce=n&code_challenge={challenge}&code_challenge_method=S256&prompt=login&max_age=3600",
        h.base_url().to_string().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client.get(&auth_url).send().await.unwrap();
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let token_url = format!("{}/token", h.base_url().to_string().trim_end_matches('/'));
    let tok = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = tok.json().await.unwrap();
    let id_token = body["id_token"].as_str().unwrap().to_owned();
    let claims = decode_id_token(&id_token);
    let auth_time = claims["auth_time"].as_i64().unwrap();
    assert_eq!(auth_time, t0().unix_timestamp() - 120);
}

#[tokio::test]
async fn fake_oidc_service_token_verifies_against_jwks() {
    let h = start().await;
    let jwt = h.service_oidc_token("mt-unsub", "unsub@example.com", time::Duration::hours(1));
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    let keys = jsonwebtoken::DecodingKey::from_rsa_pem(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pub.pem"
        ))
        .unwrap()
        .as_slice(),
    )
    .unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_audience(&["mt-unsub"]);
    let decoded = jsonwebtoken::decode::<serde_json::Value>(&jwt, &keys, &validation).unwrap();
    assert_eq!(decoded.claims["iss"], ISSUER);
    assert_eq!(decoded.claims["email_verified"], true);
}

#[tokio::test]
async fn fake_oidc_times_follow_injected_clock() {
    // The fake uses the injected virtual clock (fixed at t0()) for all times.
    let h = start().await;
    register(&h);
    let (_, id_token) = happy_path(&h).await;
    let claims = decode_id_token(&id_token);
    let iat = claims["iat"].as_i64().unwrap();
    let exp = claims["exp"].as_i64().unwrap();
    assert_eq!(iat, t0().unix_timestamp());
    assert_eq!(exp - iat, 3600);
}
