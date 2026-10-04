# T-502a: Google identity adapter (OAuth code flow, ID token validation, refresh, revoke)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 400 lines of code plus tests | T-201a, T-206, T-305, T-306 |

Split from T-502 (the index row). T-502b holds the sign-in routes.

**Read only these spec sections:** S7 section 3.4 first paragraph and bullets "Verified email", "Mailbox identity" (`docs/specs/S7-api-contract.md`); S6 section 4 bullets "Sign-in", "Multi-factor", "Scopes (Gmail)", "Re-authentication" (`docs/specs/S6-security.md`); S2 AU-04 AC5, AU-03 AC7 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 7.2 row "OAuth and OIDC" and 4.2 "Identity" bullet (`docs/specs/S10-test-strategy.md`); ASVS register rows V6.8.2, V9.1.1, V9.1.2, V9.1.3, V9.2.1, V9.2.3, V10.2.1, V10.5.1, V10.5.3, V10.5.4 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-201a-port-traits.md` "identity.rs"; `docs/backlog/T-206-fake-google-oauth-oidc.md` routes table. Nothing else is needed. This file is the S8 contract for the Google identity calls.

## Goal

`adapters-gmail` gets `GoogleIdentity`, the real `IdentityProvider`: it builds the authorisation URL (PKCE S256, `state`, `nonce`, scopes, `prompt`, `max_age`, `login_hint`), exchanges a code, fully validates Google ID tokens against pinned keys and issuer, refreshes access tokens and revokes grants. It also owns the scope constants, so every flow requests only the S6 section 4 scopes. T-502b, T-503, T-504 and `svc-common` use it through the trait.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/src/identity.rs` | `GoogleIdentity`, `GoogleIdentityConfig`, JWKS cache, wire types |
| Create | `backend/crates/adapters-gmail/src/scopes.rs` | `GMAIL_SCOPES`, `STEP_UP_SCOPES` |
| Create | `backend/crates/adapters-gmail/src/pkce.rs` | `new_pkce`, `new_state_or_nonce` |
| Change | `backend/crates/adapters-gmail/src/lib.rs` | `pub mod identity; pub mod scopes; pub mod pkce;` |
| Create | `backend/crates/adapters-gmail/tests/identity_contract.rs` | Against `fake-google` (T-206) |
| Change | `backend/crates/adapters-gmail/Cargo.toml` | `jsonwebtoken = "9"`, `sha2 = "0.10"`, `subtle = "2"` |

## Types and signatures

```rust
// scopes.rs (S6 section 4; AU-04 AC5: nothing else is ever requested)
pub const GMAIL_SCOPES: [&str; 5] = [
    "openid", "email",
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.send",
    "https://www.googleapis.com/auth/drive.appdata",
];
pub const STEP_UP_SCOPES: [&str; 2] = ["openid", "email"];   // step-up asks for no new scopes (S7 API-AUTH-1)

// pkce.rs
pub struct Pkce { pub verifier: Sensitive<String>, pub challenge: String }  // challenge = base64url(SHA-256(verifier))
pub fn new_pkce(rng: &dyn Rng) -> Pkce;                  // verifier = base64url(32 random bytes), 43 chars
pub fn new_state_or_nonce(rng: &dyn Rng) -> Sensitive<String>; // base64url(32 random bytes)

// identity.rs
pub const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
pub const GOOGLE_JWKS_URI: &str = "https://www.googleapis.com/oauth2/v3/certs";
pub const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
pub const CLOCK_SKEW_S: i64 = 60;                 // [DEFAULT]
pub const JWKS_MAX_CACHE_S: i64 = 3600;           // [DEFAULT] also honours a shorter Cache-Control max-age
pub const JWKS_MIN_REFETCH_S: i64 = 300;          // [DEFAULT] unknown kid triggers at most one refetch per 5 minutes
pub const IDENTITY_TIMEOUT_S: u64 = 10;           // [DEFAULT]

pub struct GoogleIdentityConfig {
    pub client_id: String,
    pub client_secret: Sensitive<String>,         // Secrets::GoogleOAuthClientSecret
    pub auth_endpoint: Url, pub token_endpoint: Url, pub revoke_endpoint: Url, pub jwks_uri: Url, // constants in production, fake-google in tests
}
pub struct GoogleIdentity { cfg: GoogleIdentityConfig, egress: Arc<dyn HttpEgress>, clock: Arc<dyn Clock>, jwks: RwLock<JwksCache> }
impl GoogleIdentity { pub fn new(cfg: GoogleIdentityConfig, egress: Arc<dyn HttpEgress>, clock: Arc<dyn Clock>) -> Self; }
#[async_trait] impl IdentityProvider for GoogleIdentity { /* authorize_url, exchange, validate_id_token, refresh, revoke */ }

#[derive(Deserialize)] struct TokenResponse { access_token: String, expires_in: u64, refresh_token: Option<String>,
    scope: Option<String>, id_token: Option<String>, token_type: String }
#[derive(Deserialize)] struct TokenErrorBody { error: String }
#[derive(Deserialize)] struct GoogleClaims { iss: String, aud: serde_json::Value, azp: Option<String>, sub: String,
    email: Option<String>, email_verified: Option<serde_json::Value>, iat: i64, exp: i64, nbf: Option<i64>,
    nonce: Option<String>, auth_time: Option<i64>, amr: Option<Vec<String>> }
```

`AuthRequest`, `TokenSet`, `IdClaims`, `IdError` and `Prompt` are T-201a's. `exchange` takes `redirect_uri` (T-201a).

## Algorithm

### Per-call contract (S8 for Google identity)

All calls go through `HttpEgress::call` (allowlisted hosts `accounts.google.com` is a browser redirect only; the server calls `oauth2.googleapis.com` and `www.googleapis.com`). Timeout `IDENTITY_TIMEOUT_S`. No redirects followed.

| Use | Endpoint | Method | Request | Fields read | Errors |
| --- | --- | --- | --- | --- | --- |
| Authorise (browser) | `GOOGLE_AUTH_ENDPOINT` | `GET` (built URL) | query below | none (server builds only) | none |
| Code exchange | `GOOGLE_TOKEN_ENDPOINT` | `POST` form | `grant_type=authorization_code`, `code`, `code_verifier`, `redirect_uri`, `client_id`, `client_secret` | `access_token`, `expires_in`, `refresh_token`, `scope`, `id_token` | `400 invalid_grant` gives `InvalidGrant`; other 4xx (for example `invalid_client`, `redirect_uri_mismatch`) give `Unavailable` and an `op_log` `identity_config_error`; 5xx and timeouts give `Unavailable` |
| Refresh | `GOOGLE_TOKEN_ENDPOINT` | `POST` form | `grant_type=refresh_token`, `refresh_token`, `client_id`, `client_secret` | `access_token` | same mapping; `invalid_grant` means revoked or expired |
| Revoke | `GOOGLE_REVOKE_ENDPOINT` | `POST` form | `token` | status only | `200` ok; `400 invalid_token` is `Ok` (already gone); other errors `Unavailable` |
| Keys | `GOOGLE_JWKS_URI` | `GET` | none | `keys[].kid`, `kty`, `alg`, `n`, `e`; `Cache-Control: max-age` | fetch failure gives `Unavailable` |

Authorisation URL query (in this order, encoded with `url::Url::query_pairs_mut`): `client_id`, `redirect_uri`, `response_type=code`, `scope` (space-joined `req.scopes`), `state`, `nonce`, `code_challenge`, `code_challenge_method=S256`, `access_type=offline`, `include_granted_scopes=true`, then `prompt` (`login`, `consent` or `select_account`) when set, `max_age` when set, `login_hint` when set.

Rate limits: Google's token endpoint has per-client quotas and returns `429` or `403 rate_limit_exceeded` rarely; map to `Unavailable` and never retry inside the adapter.

### `validate_id_token(raw, nonce)`

1. `jsonwebtoken::decode_header`; `alg` must be `RS256` (`none`, `HS256` and every other value: `InvalidIdToken("alg")`); `kid` required (`InvalidIdToken("signature")`). Ignore `jku`, `x5u`, `jwk` header fields entirely (V9.1.3).
2. Key: from the JWKS cache by `kid`. Cache empty or older than `min(max-age, JWKS_MAX_CACHE_S)` by `Clock`: refetch. Unknown `kid`: refetch once if the last fetch is older than `JWKS_MIN_REFETCH_S`, then look again; still unknown gives `InvalidIdToken("signature")`. Keys come only from `cfg.jwks_uri`.
3. `jsonwebtoken::decode::<GoogleClaims>` with a `Validation` for RS256 that turns off its own `exp` and `nbf` checks (`validate_exp = false`, `validate_nbf = false`) and its `aud` check (done below with exact rules), because the library reads the system clock. Bad signature gives `InvalidIdToken("signature")`.
4. With `now = clock.now()`: `iss` in `GOOGLE_ISSUERS` else `"iss"`; `aud` a string equal to `client_id`, or an array containing only `client_id`, else `"aud"`; if `azp` is present it equals `client_id`, else `"aud"`; `exp > now - CLOCK_SKEW_S` else `"exp"`; `nbf` (if present) `<= now + CLOCK_SKEW_S` else `"exp"`; `iat <= now + CLOCK_SKEW_S` else `"exp"`.
5. `nonce` present and equal to the expected value (constant time) else `"nonce"`. Single use is enforced by the caller clearing `pre_auth` (T-501).
6. Build `IdClaims`: `sub` (must be 1 to 255 characters, else `"signature"`), `email` (empty string when absent), `email_verified` true only for JSON `true` or the string `"true"`, `auth_time` from seconds when present, `amr` (empty when absent), `issued_at`, `expires_at`.

### `exchange(code, verifier, redirect_uri)`

1. POST the form; on success require `id_token` (absent gives `InvalidIdToken("signature")`).
2. `granted_scopes` = `scope` split on spaces (empty when absent).
3. Return `TokenSet`; every token wrapped in `Sensitive`.

### `refresh` and `revoke`

As in the table. `refresh` returns only the new access token; it never stores anything.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-04 AC5 | Only the S6 section 4 scopes are requested (`GMAIL_SCOPES`, `STEP_UP_SCOPES`) |
| AU-03 AC7 | The identity returned is Google `sub`, never the email |
| V6.8.2 | ID token signature checked against Google's keys; unsigned refused |
| V9.1.1 | Signature verified before any claim is used |
| V9.1.2 | Algorithm fixed to RS256; `none` and HS256 refused |
| V9.1.3 | Keys only from the pinned JWKS URL; `jku`, `x5u`, `jwk` ignored |
| V9.2.1 | `exp` and `nbf` checked with the injected clock |
| V9.2.3 | `aud` equals our client ID |
| V10.2.1 | PKCE S256 and `state` on every authorisation request |
| V10.5.1 | `nonce` must match |
| V10.5.3 | Issuer pinned to Google's exact values |
| V10.5.4 | `aud` (and `azp`) equal our client ID |

## Tests that must pass

All against `fake-google` (T-206) with its `TokenScenario` switches, through a test egress that allows only the fake's address.

- `au_04_ac5_authorize_url_requests_only_s8_scopes` (unit)
- `au_04_ac5_step_up_requests_openid_email_only` (unit)
- `au_03_ac7_claims_carry_sub` (contract)
- `asvs_v6_8_2_signed_by_unknown_key_refused` (contract, `SignedByUnknownKey`)
- `asvs_v9_1_1_tampered_payload_refused` (unit: flip a payload byte)
- `asvs_v9_1_2_alg_none_refused`, `asvs_v9_1_2_alg_hs256_refused` (contract, `AlgNone`, `AlgHs256`)
- `asvs_v9_1_3_no_kid_refused_and_jku_ignored` (contract `NoKid`; unit with a `jku` header pointing elsewhere: no request made to it)
- `asvs_v9_2_1_expired_refused`, `asvs_v9_2_1_not_yet_valid_refused` (contract `Expired`, `NotYetValid`, virtual clock)
- `asvs_v9_2_3_wrong_aud_refused` (contract `WrongAud`)
- `asvs_v10_2_1_authorize_url_has_s256_challenge_and_state` (unit)
- `asvs_v10_5_1_missing_nonce_refused`, `asvs_v10_5_1_wrong_nonce_refused` (contract `MissingNonce`, `ReplayPreviousNonce`)
- `asvs_v10_5_3_wrong_iss_refused` (contract `WrongIss`)
- `asvs_v10_5_4_azp_mismatch_refused` (unit)
- `identity_exchange_invalid_grant_maps` (contract: reused code)
- `identity_refresh_after_revoke_is_invalid_grant` (contract: `revoke_all_for`)
- `identity_revoke_unknown_token_is_ok` (contract)
- `identity_jwks_cached_then_refetched_after_max_age` (contract: count JWKS fetches with the virtual clock)
- `identity_email_verified_string_true_accepted` (unit)
- `pkce_challenge_is_sha256_of_verifier` (unit, RFC 7636 appendix B vector)

## Edge cases and traps

- `jsonwebtoken`'s built-in `exp` check uses `SystemTime`; turn it off and check with `Clock`.
- Never accept the algorithm from the token; fix RS256 in the `Validation`.
- Do not fetch keys from any URL in the token header.
- `auth_time` is absent unless `max_age` or `prompt=login` was requested; leave it `None`, never default it to `iat` (S10 7.2 step-up row).
- Do not log tokens, codes, the authorisation URL (it holds `state`, `nonce`, `login_hint`) or error bodies from Google.
- `invalid_grant` is the only signal of a revoked refresh token; keep it distinct from `Unavailable` so `svc-common` can set `needs_sign_in`.
- Use the URL builder for the query; a `login_hint` with `+` or `&` must be encoded.

## Out of scope

- Session handling and the start and callback routes: T-501, T-502b.
- Storing refresh tokens and minting mailbox access tokens: T-503.
- Step-up rules: T-504. Internal OIDC caller checks for Cloud Tasks: T-701.

## Security review checklist

- Issuer, audience, `azp`, algorithm and key source are all pinned; none comes from the token.
- Time checks use the injected clock with a 60-second skew and nothing else.
- Unknown `kid` refetches are rate-limited, so a flood of bad tokens cannot hammer Google.
- No secret (client secret, code, verifier, tokens) can reach a log line, error message or `Debug` output.
- Scopes requested are exactly `GMAIL_SCOPES` or `STEP_UP_SCOPES`.
- Every outbound call goes through `HttpEgress` to the allowlisted Google hosts.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The api's production `Ports` wiring uses `GoogleIdentity` with the pinned endpoint constants.
