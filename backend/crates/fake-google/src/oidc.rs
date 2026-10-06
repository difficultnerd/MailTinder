//! Identity fake for `fake-google` (T-206): `RS256` JWT signing, `JWKS`
//! and the discovery document. Test-only RSA keys (never secrets).

use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};

use super::scenario::TokenScenario;

pub const ISSUER: &str = "https://accounts.google.com";
pub const KID: &str = "fake-google-1";
pub const ID_TOKEN_TTL_S: i64 = 3600;

/// The public modulus of the committed main key (base64url, no padding).
/// Re-extract with `scripts/gen-test-keys.sh` if the key is regenerated.
pub const JWKS_PUBLIC_N: &str = "kRhN6syf45RiA3HhsRjZE15ST4aOUrHWB3Z1ygF6--KXZMQfjPMKz012kHiK3k-T74HydEaHSDCvVRVTWi5R_M1w8mQZItZTiItwsb_QQhfQYSwVABrdpLSwkxc95E6tIIMGTdAmfj75dxk7ZsKXxv7k7IotTWxq_2BiKie3_P5J7TDfSvlolv4dA2wUOw2dYn-9q_6azTRapsXq1rVc5-tjyOFTkFLXEiqfaF9KAAncokGQbc6_lU0v6U0Bqhcsomc26w_xS3cMhJQcj4qnCyjll5Dx6n-XUv5Rp_VtJRJ595iJtIKMTMprBg5Gb30TzqvEqeWuMUnL7uSTXypxvw";
pub const JWKS_PUBLIC_E: &str = "AQAB";

const MAIN_KEY: &[u8] =
    include_bytes!("../../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pem");
const OTHER_KEY: &[u8] =
    include_bytes!("../../../crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256-other.pem");

/// The claims placed in an ID token.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdClaims {
    pub iss: String,
    pub aud: String,
    pub azp: String,
    pub sub: String,
    pub email: String,
    pub email_verified: bool,
    pub iat: i64,
    pub exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amr: Option<Vec<String>>,
}

/// Options for building an ID token.
#[derive(Debug, Clone, Default)]
pub struct IdTokenOptions<'a> {
    pub aud: Option<&'a str>,
    pub iss: Option<&'a str>,
    pub sub: &'a str,
    pub email: &'a str,
    pub email_verified: bool,
    pub nonce: Option<&'a str>,
    pub auth_time: Option<i64>,
    pub amr: Option<Vec<String>>,
    pub now: i64,
    pub exp_offset: i64,
    pub not_before: Option<i64>,
    pub kid: Option<&'a str>,
    /// Which key to sign with.
    pub use_other_key: bool,
}

/// Build an ID token, applying any queued `TokenScenario` (None = normal).
pub fn build_id_token(opts: &IdTokenOptions<'_>, scenario: Option<TokenScenario>) -> String {
    let aud = opts
        .aud
        .unwrap_or("fake-client.apps.example.test")
        .to_owned();
    let iss = opts.iss.unwrap_or(ISSUER).to_owned();
    let now = opts.now;
    let exp = now + opts.exp_offset;
    let nbf = opts.not_before;
    let mut kid: Option<String> = Some(opts.kid.unwrap_or(KID).to_owned());
    let mut alg = Algorithm::RS256;
    let mut secret: Option<Vec<u8>> = None;
    let nonce = opts.nonce.map(str::to_owned);

    let mut claims = IdClaims {
        iss: iss.clone(),
        aud: aud.clone(),
        azp: aud.clone(),
        sub: opts.sub.to_owned(),
        email: opts.email.to_owned(),
        email_verified: opts.email_verified,
        iat: now,
        exp,
        nonce,
        auth_time: opts.auth_time,
        nbf,
        amr: opts.amr.clone(),
    };

    match scenario {
        Some(TokenScenario::WrongAud) => {
            "other-client".clone_into(&mut claims.aud);
            claims.azp = claims.aud.clone();
        }
        Some(TokenScenario::WrongIss) => {
            "https://evil.example.com".clone_into(&mut claims.iss);
        }
        Some(TokenScenario::Expired) => {
            claims.exp = now - 60;
        }
        Some(TokenScenario::NotYetValid) => {
            let f = now + 600;
            claims.iat = f;
            claims.exp = f + opts.exp_offset;
            claims.nbf = Some(f);
        }
        Some(TokenScenario::ReplayPreviousNonce | TokenScenario::SignedByUnknownKey) => {}
        Some(TokenScenario::MissingNonce) => {
            claims.nonce = None;
        }
        Some(TokenScenario::MissingAuthTime) => {
            claims.auth_time = None;
        }
        Some(TokenScenario::AlgNone) => {
            // jsonwebtoken 9 has no Algorithm::None; build an unsigned JWT.
            let header = serde_json::json!({ "alg": "none", "typ": "JWT" });
            let h = base64url_json(&header);
            let p = base64url_json(&serde_json::to_value(&claims).expect("claims"));
            return format!("{h}.{p}.");
        }
        Some(TokenScenario::AlgHs256) => {
            alg = Algorithm::HS256;
            // Key confusion attack: HS256 with the public key bytes as secret.
            secret = Some(MAIN_KEY.to_vec());
        }
        Some(TokenScenario::NoKid) => {
            kid = None;
        }
        None => {}
    }

    let header = Header {
        alg,
        kid,
        ..Default::default()
    };

    let enc_key = match scenario {
        Some(TokenScenario::AlgHs256) => {
            EncodingKey::from_secret(secret.as_deref().unwrap_or_default())
        }
        Some(TokenScenario::SignedByUnknownKey) => {
            EncodingKey::from_rsa_pem(OTHER_KEY).expect("other key")
        }
        _ => EncodingKey::from_rsa_pem(MAIN_KEY).expect("main key"),
    };

    jsonwebtoken::encode(&header, &claims, &enc_key).expect("encode id token")
}

/// base64url(SHA-256-agnostic JSON serialisation) of a value.
fn base64url_json(v: &serde_json::Value) -> String {
    let s = serde_json::to_string(v).expect("serialize");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

/// The JWKS document (main key only).
pub fn jwks() -> serde_json::Value {
    serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "kid": KID,
            "n": JWKS_PUBLIC_N,
            "e": JWKS_PUBLIC_E,
        }]
    })
}

/// The `OpenID` discovery document.
pub fn discovery(base: &str) -> serde_json::Value {
    serde_json::json!({
        "issuer": ISSUER,
        "authorization_endpoint": format!("{base}o/oauth2/v2/auth"),
        "token_endpoint": format!("{base}token"),
        "revocation_endpoint": format!("{base}revoke"),
        "jwks_uri": format!("{base}oauth2/v3/certs"),
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256", "plain"],
    })
}
