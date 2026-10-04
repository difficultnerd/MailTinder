# T-206: fake-google, part 3: OAuth and OIDC identity fake

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 550 lines of code plus tests | T-205a |

**Read only these spec sections:** S10 4.2 "Identity" bullet, 7.2 rows "OAuth and OIDC", "Google sign-in", "Step-up" (`docs/specs/S10-test-strategy.md`); S6 4 "Sign-in", "First sign-in", "Re-authentication" bullets (`docs/specs/S6-security.md`); S7 3.4 first paragraph and 3.6 (`docs/specs/S7-api-contract.md`); `docs/backlog/T-205a-fake-google-gmail.md` (server, token registry, control API style). Nothing else is needed.

## Goal

`fake-google` also plays Google's identity platform: the authorisation endpoint (answering immediately as a scripted user), the token endpoint (authorisation code with PKCE, refresh, `invalid_grant`), the revoke endpoint, a JWKS URL and the OpenID discovery document, issuing RS256 ID tokens and access tokens that the Gmail and Drive routes accept. Scenario switches produce every bad token S10 7.2 needs. It can also mint Google-style OIDC tokens for service callers (Cloud Tasks and Cloud Scheduler to `unsub` and `worker`, T-701).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/fake-google/src/oauth.rs` | authorise, token, revoke routes |
| Create | `backend/crates/fake-google/src/oidc.rs` | ID token building and signing, JWKS, discovery |
| Create | `backend/crates/fake-google/src/scenario.rs` | `NextLogin`, `TokenScenario` |
| Change | `backend/crates/fake-google/src/control.rs` | identity control routes |
| Change | `backend/crates/fake-google/src/tokens.rs` | refresh tokens; link access tokens to grants for revoke |
| Create | `backend/crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256.pem` | test-only RSA 2048 private key, generated once (see Algorithm 1) |
| Create | `backend/crates/testkit/fixtures/keys/TEST-ONLY-fake-google-rs256-other.pem` | second key, for "signed by an unknown key" |
| Create | `scripts/gen-test-keys.sh` | the `openssl genpkey` commands used, for reproducibility |
| Change | `.gitleaks.toml` | allowlist the two paths above, with a comment |
| Change | `.pre-commit-config.yaml` | `exclude` for `detect-private-key` matching `TEST-ONLY-.*\.pem$` |
| Change | `backend/crates/fake-google/Cargo.toml` | add `jsonwebtoken` (ring backend), `sha2`, `base64` |
| Create | `backend/crates/fake-google/tests/oauth_routes.rs` | route and scenario tests |

## Types and signatures

```rust
pub const ISSUER: &str = "https://accounts.google.com";          // what the fake writes in `iss`
pub const KID: &str = "fake-google-1";

#[derive(Clone, Debug, Deserialize)]
pub struct ClientReg { pub client_id: String, pub client_secret: String, pub redirect_uris: Vec<String> }

#[derive(Clone, Debug, Deserialize)]
pub struct NextLogin {
    pub sub: String,                        // e.g. "sub-alice"
    pub email: String,                      // reserved domain only
    pub email_verified: bool,
    pub amr: Option<Vec<String>>,           // None: claim absent (Google's usual case)
    pub auth_age_s: i64,                    // auth_time = now - auth_age_s; default 0
    pub outcome: LoginOutcome,              // approve or deny
}
#[derive(Clone, Copy, Debug, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum LoginOutcome { Approve, AccessDenied }

/// One-shot switches applied to the next ID token the token endpoint issues.
#[derive(Clone, Copy, Debug, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum TokenScenario {
    WrongAud, WrongIss, Expired, NotYetValid, ReplayPreviousNonce, MissingNonce, MissingAuthTime,
    SignedByUnknownKey, AlgNone, AlgHs256, NoKid,
}

impl FakeGoogleHandle {
    pub fn register_client(&self, c: ClientReg);
    pub fn next_login(&self, l: NextLogin);
    pub fn token_scenario(&self, s: TokenScenario);
    pub fn revoke_all_for(&self, sub: &str);                    // simulates the user removing access at Google
    pub fn revocations(&self) -> Vec<String>;                   // token kinds revoked, e.g. "refresh"; never values
    pub fn service_oidc_token(&self, aud: &str, email: &str, ttl: time::Duration) -> String; // for T-701 caller checks
}
```

Routes:

| Method and path | Behaviour |
| --- | --- |
| `GET /.well-known/openid-configuration` | `issuer`, `authorization_endpoint`, `token_endpoint`, `revocation_endpoint`, `jwks_uri` (all pointing at this fake), `id_token_signing_alg_values_supported: ["RS256"]`, `code_challenge_methods_supported: ["S256","plain"]` |
| `GET /oauth2/v3/certs` | JWKS with the main key: `kty RSA`, `alg RS256`, `use sig`, `kid`, `n`, `e` |
| `GET /o/oauth2/v2/auth` | checks below; answers `302` to `redirect_uri?code=<c>&state=<state>` (or `?error=access_denied&state=`); a bad request answers `400` with an HTML-free text body and never redirects |
| `POST /token` | form body; `grant_type=authorization_code` or `refresh_token`; JSON responses as Google's |
| `POST /revoke` | form `token=<t>`; revokes a refresh token (and every access token from its grant) or a single access token; 200 `{}`; unknown token 400 `{"error":"invalid_token"}` |

Control routes: `POST /__fake/identity/clients`, `POST /__fake/identity/next-login`, `POST /__fake/identity/token-scenario`, `POST /__fake/identity/revoke-all {sub}`, `GET /__fake/identity/revocations`, `POST /__fake/identity/service-token {aud, email, ttl_s}`.

## Algorithm

1. Keys `[DEFAULT]`: run `scripts/gen-test-keys.sh` once (`openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out .../TEST-ONLY-fake-google-rs256.pem`, same for `-other`) and commit the output. Embed with `include_bytes!` and load with `jsonwebtoken::EncodingKey::from_rsa_pem`. Compute `n` and `e` for the JWKS from the same key once at start (parse the PKCS#8 DER with a small DER walk, or ship the matching `n` and `e` as base64url constants generated by the same script). Why committed keys: generating RSA keys in Rust needs the `rsa` crate, which carries an open RustSec advisory that would fail `cargo-audit`; a published test-only key is not a secret. Add the allowlist entries with a comment saying so.
2. Authorise (`GET /o/oauth2/v2/auth`), in order:
   1. `client_id` registered, `redirect_uri` exactly equal to a registered URI (string compare, no prefix match), `response_type=code`, `scope` contains `openid`, `state` and `nonce` present, `code_challenge` present with `code_challenge_method=S256` (a `plain` method or missing challenge gives 400: PKCE is required, S10 7.2).
   2. Take the queued `NextLogin` (none queued gives 400 `no_scripted_login`). `AccessDenied` redirects with `error=access_denied`.
   3. Store a grant: code (32 random hex chars from the fake's own counter-seeded generator), client, redirect URI, challenge, nonce, scopes, login, `auth_time`, `prompt`, `max_age`, created at (Clock). Codes live 10 minutes and are single use.
3. Token, `authorization_code`:
   1. Client authentication: `client_id` and `client_secret` in the form (or HTTP Basic); wrong gives 401 `{"error":"invalid_client"}`.
   2. Code unknown, expired, already used, or issued to another client gives 400 `{"error":"invalid_grant"}`. A second use of a code also revokes every token from that grant (RFC 6749 4.1.2).
   3. `redirect_uri` must equal the authorise one; `code_verifier` must satisfy `base64url_nopad(SHA-256(verifier)) == challenge`; else `invalid_grant`.
   4. Issue an access token (registered with the T-205a registry: mailbox = the login's email, created if absent; scopes = the granted scopes; TTL 3599 s), a refresh token (only on the first grant for this sub and client, or when `prompt` included `consent`), and an ID token.
   5. Response `{"access_token","expires_in":3599,"refresh_token"?,"scope":"<space separated>","token_type":"Bearer","id_token"}`.
4. ID token claims: `iss` = `ISSUER`, `aud` = client ID, `azp` = client ID, `sub`, `email`, `email_verified`, `iat` = now, `exp` = now + 3600, `nonce`, `auth_time` (included when `max_age` or `prompt=login` was requested, else omitted `[ASSUMES]` Google's behaviour), `amr` when scripted. Header `alg RS256`, `kid` = `KID`, `typ JWT`.
5. Apply a queued `TokenScenario` once: `WrongAud` (`aud` = "other-client"), `WrongIss` (`https://evil.example.com`), `Expired` (`exp` = now - 60), `NotYetValid` (`iat` and `nbf` = now + 600), `ReplayPreviousNonce` (the nonce of the previous grant), `MissingNonce`, `MissingAuthTime`, `SignedByUnknownKey` (sign with the other key, same `kid`), `AlgNone` (header `alg none`, empty signature), `AlgHs256` (HS256 with the public key bytes as the secret: the classic key confusion attack), `NoKid`.
6. Token, `refresh_token`: unknown or revoked gives `invalid_grant`; else a new access token (same scopes, same mailbox) and, if `openid` was granted, a new ID token without `nonce` `[ASSUMES]`. The refresh token itself does not rotate (Google behaviour; rotation is a v2 Microsoft scenario).
7. Revoke: as in the route table. `revoke_all_for(sub)` revokes every refresh and access token for that sub (S10 6.3 "revoked refresh token" case).
8. Service OIDC token: claims `iss` = `https://accounts.google.com`, `aud`, `email`, `email_verified: true`, `sub` = a stable hash of the email, `iat`, `exp`; same key and `kid`.
9. All times come from the injected `Clock`; no wall time.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | Fake only. T-502, T-503, T-504 and T-701 prove AU-03 AC2, AU-03 AC3, the S10 7.2 OAuth, sign-in and step-up rows and the ASVS V9 and V10 rows with it |

## Tests that must pass

- `fake_oidc_happy_path_code_pkce_id_token_validates` (integration: decode the ID token with `jsonwebtoken` and the JWKS; claims as scripted).
- `fake_oidc_plain_or_missing_pkce_is_400` (integration).
- `fake_oidc_redirect_uri_must_match_exactly` (integration: trailing slash differs, 400, no redirect).
- `fake_oidc_code_is_single_use_and_reuse_revokes_grant` (integration).
- `fake_oidc_wrong_verifier_is_invalid_grant` (integration).
- `fake_oidc_each_token_scenario_produces_its_defect` (integration, one assertion per `TokenScenario`).
- `fake_oidc_refresh_returns_new_access_token_accepted_by_gmail_routes` (integration with T-205a).
- `fake_oidc_revoked_refresh_is_invalid_grant` (integration).
- `fake_oidc_access_denied_redirects_with_error` (integration).
- `fake_oidc_auth_time_present_with_prompt_login` (integration).
- `fake_oidc_service_token_verifies_against_jwks` (integration).
- `fake_oidc_times_follow_injected_clock` (integration with `VirtualClock`).

## Edge cases and traps

- Never put a real Google client ID or secret anywhere; register `client_id = "fake-client.apps.example.test"` in tests.
- The key files are named `TEST-ONLY-*` and live under `testkit/fixtures/keys/`; never load them from any non-test crate. `fake-google` is test infrastructure only.
- Do not weaken the gitleaks rules globally; allowlist exactly the two paths.
- The authorise route must never redirect on a bad `redirect_uri` (open redirect); answer 400.
- Compare `code_verifier` against the challenge in constant time is not needed in a fake, but compare the full string, not a prefix.
- `expires_in` is seconds as a number; `scope` is one space-separated string.
- The token endpoint takes `application/x-www-form-urlencoded`, not JSON; a JSON body gives 400 `invalid_request`.
- Record no token values in events; record kinds (`access`, `refresh`, `id`) only.
- Do not add a `rsa` crate dependency (advisory, see Algorithm 1).

## Out of scope

- The real `IdentityProvider` adapter and every sign-in rule (T-502 to T-504). Microsoft identity (v2).

## Done when

- The tests above pass and every required check is green (S10 10.1), including Gitleaks and the pre-commit hooks on the key files.
- Definition of done in S10 10.4.
- The route table is copied into `oauth.rs` as a module doc comment.
