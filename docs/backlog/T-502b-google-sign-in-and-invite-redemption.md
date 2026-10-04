# T-502b: Google sign-in and invite redemption (API-AUTH-1, API-AUTH-2)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 550 lines of code plus tests | T-107, T-206, T-501, T-502a, T-503 |

Split from T-502 (the index row). T-502a holds the identity adapter.

**Read only these spec sections:** S7 sections 3.1, 3.4, and 5.2 API-AUTH-1 and API-AUTH-2 (`docs/specs/S7-api-contract.md`); S7 section 6 row "Sign-in start and callback"; S6 section 4 bullets "Sign-in", "Multi-factor", "First sign-in (enrolment)", and section 3 rows T6, T10 (`docs/specs/S6-security.md`); S2 AU-02 AC1, AU-03 AC1 to AC7, AU-01 AC4 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 7.2 rows "OAuth and OIDC", "Google sign-in", "Invites" (`docs/specs/S10-test-strategy.md`); ASVS register rows V3.7.2, V6.3.1, V6.3.3, V6.3.4, V6.4.1, V6.8.1, V7.6.2, V10.1.1, V10.1.2, V10.2.2, V10.5.2, V14.2.1, V16.3.1, V2.3.4 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-501-sessions-cookie-and-csrf.md` "Types and signatures"; `docs/backlog/T-107-state-machines.md` `check_redemption`. Nothing else is needed.

## Goal

The api serves `POST /api/v1/auth/google/start` and `GET /api/v1/auth/google/callback` for the `sign_in` and `join` intents: a returning user signs in with any linked Gmail mailbox (matched by Google `sub`), an invited person joins with the single-use invite token plus a matching verified email, and an uninvited person reaches the `pending_invite_request` state. Both routes are the shared machinery the `link`, `reconnect` (T-601a) and `step_up` (T-504) intents plug into.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/auth/mod.rs` | `pub mod start; pub mod callback; pub mod outcome; pub mod email_key;` |
| Create | `backend/crates/api/src/auth/start.rs` | API-AUTH-1 handler and `begin_oauth` |
| Create | `backend/crates/api/src/auth/callback.rs` | API-AUTH-2 handler, `CallbackContext`, intent dispatch, `sign_in` and `join` |
| Create | `backend/crates/api/src/auth/outcome.rs` | `Outcome` enum and the redirect |
| Create | `backend/crates/api/src/auth/email_key.rs` | `email_lookup_hash` |
| Change | `backend/crates/api/src/lib.rs` | Mount the two routes; add their templates to `ROUTE_TEMPLATES` |
| Create | `backend/crates/api/tests/sign_in.rs` | Service integration tests against `fake-google` |

## Types and signatures

```rust
// auth/outcome.rs: S7 3.4 outcome table; match without `_`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { SignedIn, Joined, Linked, Reconnected, SteppedUp, NotInvited, NotRegistered, InviteInvalid,
                   EmailMismatch, EmailUnverified, MailboxLinkedElsewhere, StepUpWrongAccount, ConsentBlocked, Cancelled, Failed }
impl Outcome {
    pub fn code(self) -> &'static str;       // "signed_in", ..., "failed"
    pub fn keeps_tokens(self) -> bool;       // true only for SignedIn, Joined, Linked, Reconnected
}
/// 302 to "{app_origin}/#/auth/result?outcome=<code>" plus optional Set-Cookie. Nothing else in the URL.
pub fn redirect(config: &ApiConfig, outcome: Outcome, cookie: Option<NewCookie>) -> Response;

// auth/email_key.rs
/// HMAC-SHA-256 under ApiConfig::email_lookup_key of EmailAddress::lookup_form() (T-404).
pub fn email_lookup_hash(key: &Sensitive<Vec<u8>>, email: &EmailAddress) -> EmailLookupHash;

// auth/start.rs
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct StartRequest { pub intent: AuthIntentWire, pub invite_token: Option<String>, pub mailbox_id: Option<Uuid> }
#[derive(Deserialize)] #[serde(rename_all = "snake_case")]
pub enum AuthIntentWire { SignIn, Join, Link, Reconnect, StepUp }
#[derive(Serialize)] pub struct StartResponse { pub authorization_url: String }
pub struct OAuthParams { pub scopes: &'static [&'static str], pub prompt: Option<Prompt>, pub max_age_s: Option<u32>, pub login_hint: Option<Sensitive<String>> }
/// Shared by every intent: new state, nonce and PKCE; seal them into pre_auth on the session; build the URL.
pub async fn begin_oauth(state: &AppState, session: &LoadedSession, intent: AuthIntent, invite_token_hash: Option<Sha256Hash>, p: OAuthParams) -> Result<(StartResponse, Option<NewCookie>), ApiError>;

// auth/callback.rs
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct CallbackQuery { pub code: Option<String>, pub state: String, pub error: Option<String>, pub scope: Option<String>, pub authuser: Option<String>, pub prompt: Option<String>, pub hd: Option<String> }
/// Everything the intent handlers need after state, code and ID token are verified.
pub struct CallbackContext<'a> {
    pub state: &'a AppState, pub session: LoadedSession, pub pre: PreAuthPlain,
    pub tokens: TokenSet, pub claims: IdClaims, pub request_id: RequestId,
}
pub async fn finish_sign_in(ctx: CallbackContext<'_>) -> (Outcome, Option<NewCookie>);
pub async fn finish_join(ctx: CallbackContext<'_>) -> (Outcome, Option<NewCookie>);
// Implemented by later tasks; until they merge, these return (Outcome::Failed, None):
//   T-601a: auth::link::finish_link, auth::link::finish_reconnect
//   T-504:  auth::step_up::finish_step_up
/// Revoke new provider tokens unless the outcome keeps them and only when `sub` is linked to no user.
pub async fn discard_tokens(state: &AppState, tokens: &TokenSet, sub_is_linked: bool);
```

Google adds `scope`, `authuser`, `prompt` and `hd` to the callback query; they are accepted and ignored so `deny_unknown_fields` does not break real sign-ins.

## Algorithm

### API-AUTH-1 `POST /api/v1/auth/{provider}/start`

1. `provider` must be `google`; `microsoft` and anything else give `400 invalid_request` (v2).
2. Rate limit `policies::SIGN_IN_IP` by `ClientIp`.
3. A session must exist (`AnySession`); the app always calls `GET /session` first, and CSRF (T-501) has already passed.
4. Validate combinations: `invite_token` only with `join` (43 to 64 characters of `[A-Za-z0-9_-]`, else `400`); `mailbox_id` only with `reconnect`. `link`, `reconnect` and `step_up` need state `Authenticated`, else `401`; `sign_in` and `join` need state `PreAuth` or `PendingInviteRequest` `[DEFAULT]` ("Use a different account" from the request screen), and from `Authenticated` give `400 invalid_request`.
5. Per intent (match on `AuthIntent` without `_`):
   - `sign_in`: scopes `GMAIL_SCOPES`, `prompt = SelectAccount`, no `max_age`.
   - `join`: scopes `GMAIL_SCOPES`, `prompt = Consent` (Google then always returns a refresh token), `invite_token_hash = SHA-256(token)` when a token is given.
   - `link`, `reconnect`: delegated to T-601a's start helpers; until merged, `400 invalid_request`.
   - `step_up`: delegated to T-504's start helper; until merged, `400 invalid_request`.
6. `begin_oauth`: `oauth_state = new_state_or_nonce`, `nonce = new_state_or_nonce`, `pkce = new_pkce` (T-502a). From `PreAuth` or `PendingInviteRequest`: `SessionService::rotate` (new ID) and set `state = PreAuth`, `pre_auth = seal_pre_auth({intent, oauth_state, nonce, pkce_verifier, invite_token_hash, pending_email: None, started_at: now})`. From `Authenticated`: same rotation keeps `Authenticated` and sets `pre_auth`. `authorization_url = identity.authorize_url(&AuthRequest { state, nonce, code_challenge, redirect_uri: config.oauth_redirect_uri, scopes, prompt, max_age_s, login_hint })`.
7. Respond `200 { "authorization_url" }` with `Set-Cookie`.

### API-AUTH-2 `GET /api/v1/auth/{provider}/callback`

1. Rate limit `SIGN_IN_IP` by `ClientIp` (over the limit: `429` problem, as the OpenAPI file says).
2. Load the session. No session, no `pre_auth`, or `pre_auth` older than 10 minutes: `failed` plus `security_event { action: "sign_in", outcome: "state_invalid" }`.
3. `open_pre_auth`. Compare query `state` with `pre.oauth_state` in constant time; mismatch is `failed` plus the same event.
4. From here on the `pre_auth` fields are spent: every exit writes the session with `pre_auth = None` (for a `PreAuth` session that means deleting the record and sending `clear_cookie()`; T-501 treats a `PreAuth` record without `pre_auth` as dead anyway). That makes `state`, `nonce` and the verifier single use.
5. `error=access_denied`: `cancelled`. Any other `error` or a missing `code`: `failed`.
6. `identity.exchange(code, pre.pkce_verifier, config.oauth_redirect_uri)`; error: `failed`.
7. `identity.validate_id_token(tokens.id_token, pre.nonce)`; error: `failed` plus `security_event { action: "sign_in", outcome: "id_token_invalid" }`.
8. `sub_is_linked = mailboxes().by_subject(Provider::Gmail, &ProviderSubjectId::new(claims.sub))?.is_some()`.
9. Dispatch on `pre.intent` (match without `_`): `SignIn` to `finish_sign_in`, `Join` to `finish_join`, `Link` and `Reconnect` to T-601a, `StepUp` to T-504.
10. If `!outcome.keeps_tokens()`, `discard_tokens(state, &tokens, sub_is_linked)`: when `sub_is_linked` is false, `identity.revoke(refresh_token or access_token)` best effort (log `op_log` on failure); when true, never revoke (Google revokes the whole grant, which would kill the stored refresh token of that linked mailbox). Then drop the tokens.
11. Security events: `sign_in` with outcome code and `amr` from `claims.amr` when present (V6.3.3, V16.3.1); failures never include the email or `sub`.
12. `redirect(config, outcome, cookie)`.

### `finish_sign_in`

1. Mailbox by `sub` (Provider `Gmail`). None: `not_registered`.
2. `establish(current, mailbox.user_id, recent_auth_at = None)` (T-501): new session, every other session of the user ended and logged.
3. If `tokens.refresh_token` is present and `granted_scopes` contains every entry of `GMAIL_SCOPES` except `openid` and `email` (Google may report those as `openid` and `https://www.googleapis.com/auth/userinfo.email`), `TokenService::store_refresh_token` (this also fixes a `needs_sign_in` mailbox). Otherwise keep the stored one.
4. Outcome `signed_in`.

### `finish_join`

1. `sub_is_linked`: behave exactly as `finish_sign_in` (S7: "For an existing account: signs in as sign_in would"); the invite is not touched.
2. `email = EmailAddress::parse(claims.email)`; failure counts as unverified.
3. No `pre.invite_token_hash`:
   - `email_verified` false or parse failed: `email_unverified`.
   - Else rotate the session to `PendingInviteRequest` with `pre_auth = seal_pre_auth({intent: Join, fresh random state, nonce and verifier that are never used, invite_token_hash: None, pending_email: Some(email), started_at: now})` `[DEFAULT]` (the `pending_invite_request` state reuses `pre_auth` to hold the encrypted email and its 10-minute life). Outcome `not_invited`. Tokens are discarded (AU-02 AC1).
4. With a token hash: rate limit `SIGN_IN_FAILED_SUBJECT` keyed by `Subject(claims.sub)` before checking (every redemption attempt counts; an honest invitee needs one). Over the limit: `failed`.
5. `invite = invites().by_token_hash(hash)`. Build `InviteState` from it and call `domain::invite::check_redemption(invite, Some(hash), &email_lookup_hash(key, &email), claims.email_verified, now)`:
   - `EmailUnverified` gives `email_unverified`; `InviteInvalid` gives `invite_invalid`; `EmailMismatch` gives `email_mismatch` (AU-03 AC2, AC3, AC6).
6. Claim the invite first: put it with `status = Used` and `Precondition::Matches(version)`. `PreconditionFailed` (used, revoked or re-sent at the same moment): `invite_invalid` (V2.3.4).
7. Missing `tokens.refresh_token`, or granted scopes short of `GMAIL_SCOPES`: `failed` (the invite stays used; the admin re-sends) `[DEFAULT]`; log `op_log` `join_missing_grant`.
8. Create the user: `user_id = rng.uuid_v4()`, `wrapped = keys.new_user_key(&user_id)`, `UserRecord { is_admin: false, .. }` with `MustNotExist`.
9. Create the mailbox: `mailbox_id = mailbox_id_for(Provider::Gmail, &sub)` (T-201b); `email_address` sealed with `KeyService` and `Aad { user, scope: mailbox_id, field: aad_fields::MAILBOX_EMAIL }`; `refresh_token` from `svc_common::mint::seal_refresh_token`; `status = Connected`, `linked_at = now`, `is_primary = true`; put with `MustNotExist`. `AlreadyExists` (linked by a racing request): delete the new user record, outcome `mailbox_linked_elsewhere`.
10. `establish(current, user_id, recent_auth_at = claims.auth_time)` (S7 3.6: "A join callback also sets `recent_auth_at`"; using `auth_time` rather than `now` means a stale Google session gives no step-up window).
11. Security events `invite_use` (outcome `used`), `mailbox_link`, `sign_in` (outcome `joined`). Outcome `joined`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-03 AC1 | Valid invite token plus matching verified email creates the user, links the mailbox, uses the token, lands on the Feed |
| AU-03 AC2 | A different, unlinked account is refused (`email_mismatch`) and no token is kept |
| AU-03 AC3 | Unverified email is refused (`email_unverified`) |
| AU-03 AC4 | An existing user signs in with any linked Gmail mailbox by `sub`; no invite needed |
| AU-03 AC6 | Missing, used, expired, revoked or replaced token is refused even when the email matches |
| AU-03 AC7 | Mailboxes are keyed by Google `sub`, never email |
| AU-02 AC1 | An uninvited person reaches the request-invite state and no mailbox token is kept |
| AU-01 AC4 | A revoked invite cannot be redeemed |
| V2.3.4 | An invite is claimed with a conditional write, so it cannot be used twice |
| V3.7.2 | The server redirects only to Google's fixed endpoint and to its own origin |
| V6.3.1 | Sign-in and redemption are rate limited per IP and per Google subject |
| V6.3.3 | `amr` is recorded with the sign-in event when Google sends it |
| V6.3.4 | One pathway (Google OAuth) for join and sign-in |
| V6.4.1 | Invite token: 256-bit, single use, hash only, expiry checked |
| V6.8.1 | Identity by provider plus subject; invite needs the token and the verified email |
| V7.6.2 | Sessions are created only by a user-started Google sign-in |
| V10.1.1 | Provider tokens never reach the browser (bodies, cookies and URLs) |
| V10.1.2 | PKCE verifier, `state` and `nonce` are random and bound to the pre-auth session |
| V10.2.2 | `state` is checked against the session; `iss` pinned (T-502a) |
| V10.5.2 | Users are keyed by Google `sub` |
| V14.2.1 | The callback redirect carries only the outcome code |
| V16.3.1 | Every sign-in success and failure is logged |

## Tests that must pass

Service integration through the router, `fake-google` (T-206) for identity, fake store and keys, virtual clock. A helper drives start, then the fake's authorise endpoint, then the callback.

- `au_03_ac1_invited_user_joins_and_lands_on_feed` (outcome `joined`, user and primary mailbox exist, invite `used`)
- `au_03_ac2_email_mismatch_refused_and_tokens_revoked` (fake-google records a revocation)
- `au_03_ac3_unverified_email_refused`
- `au_03_ac4_existing_user_signs_in_with_second_linked_mailbox`
- `au_03_ac6_missing_token_with_matching_email_is_request_state_not_join`
- `au_03_ac6_used_token_refused`, `au_03_ac6_expired_token_refused`, `au_03_ac6_replaced_token_refused`
- `au_03_ac7_mailbox_id_derived_from_sub`
- `au_02_ac1_uninvited_reaches_pending_state_and_no_token_kept` (store dump holds no refresh token; fake records a revocation)
- `au_01_ac4_revoked_invite_refused`
- `asvs_v2_3_4_two_concurrent_redemptions_one_wins`
- `asvs_v3_7_2_callback_redirects_only_to_app_origin`
- `asvs_v6_3_1_sign_in_ip_limit` and `asvs_v6_3_1_redemption_attempts_per_subject_limited`
- `asvs_v6_3_3_amr_recorded_when_present`
- `asvs_v6_3_4_only_google_provider_accepted` (`microsoft` gives 400)
- `asvs_v6_4_1_invite_single_use`
- `asvs_v6_8_1_linked_sub_signs_in_regardless_of_email_change`
- `asvs_v7_6_2_callback_without_started_flow_creates_no_session`
- `asvs_v10_1_1_no_token_in_any_response_or_cookie` (scan bodies, headers and `Location`)
- `asvs_v10_1_2_state_nonce_verifier_random_and_sealed` (store dump has no clear value)
- `asvs_v10_2_2_state_mismatch_failed_and_logged`
- `asvs_v10_5_2_user_keyed_by_sub`
- `asvs_v14_2_1_redirect_has_outcome_only`
- `asvs_v16_3_1_sign_in_success_and_failure_logged`
- `ses_1_pre_auth_cleared_after_callback` (a replayed callback with the same `state` gives `failed`)
- `sign_in_linked_account_tokens_never_revoked_on_failure` (link-elsewhere style case: `sub` linked, outcome not keeping tokens, no revocation recorded)
- `sign_in_callback_accepts_google_extra_query_params`

## Edge cases and traps

- Never revoke new tokens for a `sub` that is linked to any user; Google revokes the whole grant.
- Check `state` before using `code`; check the ID token before reading any claim.
- The invite token arrives in the request body (from the URL fragment); only its SHA-256 is stored. Never log it.
- `email_verified` and the token are both required; the email alone never creates a user.
- Use `check_redemption`'s order (verified, token, email) so an unverified account learns nothing about the invite.
- Do not put the email, `sub` or any error detail in the redirect URL.
- `join` from an existing account must not consume the invite.
- Clear `pre_auth` on every exit path, including errors, so the callback is single use.
- `auth_time` may be absent; then `recent_auth_at` is `None`.
- Match `AuthIntent` and `Outcome` without `_`.

## Out of scope

- `link` and `reconnect` intents: T-601a. `step_up`: T-504. `GET /session`, sign-out: T-506.
- Creating invites, re-sending and invite requests: T-505.

## Security review checklist

- The callback refuses anything without a live `pre_auth` record, a constant-time `state` match, a successful PKCE exchange and a fully validated ID token with the sealed `nonce`.
- `pre_auth` is cleared on every path; a replayed callback cannot succeed.
- New users need a pending, unexpired, matching invite token hash, `email_verified` true and an HMAC email match; the invite is claimed with a conditional write before the user exists.
- Tokens are revoked only for unlinked subjects and are never returned to the browser.
- Sign-in calls `establish`, so the old session ends and the ID rotates.
- Rate limits apply per IP on both routes and per subject on redemption.
- Security events carry outcome codes only (pseudonymous user ID once known), never email, `sub` or tokens.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The e2e harness (T-1101) can sign in with an invite through `fake-google` using these routes.
