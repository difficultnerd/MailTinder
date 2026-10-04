# T-504: Step-up by fresh Google sign-in

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 300 lines of code plus tests | T-502b |

**Read only these spec sections:** S7 section 3.6 and API-AUTH-1 paragraph on `step_up` (`docs/specs/S7-api-contract.md`); S6 section 4 bullet "Re-authentication (step-up, ASVS V7.5.1)" (`docs/specs/S6-security.md`); S2 glossary `STEP_UP_WINDOW`, AU-01 AC5 (`docs/specs/S2-v1-acceptance-criteria.md`); S9 section 1.1 (`docs/specs/S9-functional-screens.md`); S10 7.2 row "Step-up" (`docs/specs/S10-test-strategy.md`); ASVS register rows V6.8.4, V7.2.4, V7.5.1, V16.3.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-502b-google-sign-in-and-invite-redemption.md` "Types and signatures". Nothing else is needed.

## Goal

Sensitive actions (delete account, link or disconnect a mailbox, every admin write) can demand a fresh Google sign-in made in the last 5 minutes with one of the user's own linked Google accounts. This task adds the `step_up` intent to the OAuth routes, the `require_step_up` check, and the `SteppedUpUser` and `SteppedUpAdmin` extractors the route tasks use.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/auth/step_up.rs` | `STEP_UP_WINDOW`, `start_step_up`, `finish_step_up`, `require_step_up` |
| Change | `backend/crates/api/src/auth/start.rs` | `step_up` intent calls `start_step_up` |
| Change | `backend/crates/api/src/auth/callback.rs` | `StepUp` dispatch calls `finish_step_up` |
| Change | `backend/crates/api/src/session/extract.rs` | `SteppedUpUser`, `SteppedUpAdmin` |
| Create | `backend/crates/api/tests/step_up.rs` | Service integration tests against `fake-google` |

## Types and signatures

```rust
// auth/step_up.rs
pub const STEP_UP_WINDOW_S: i64 = 300;   // S2 glossary, decided; [TUNABLE] per S7 3.6
pub const STEP_UP_MAX_AGE_S: u32 = 300;  // S6 4, S4 3.1, S2 glossary and ASVS V6.8.4: max_age=300

/// Ok when recent_auth_at is within the window: now - t <= 300 s and t <= now + 60 s skew.
pub fn require_step_up(session: &AuthedSession, clock: &dyn Clock) -> Result<(), ApiError>; // Err(ApiError::StepUpRequired)
pub fn step_up_valid_until(session_recent_auth_at: Option<OffsetDateTime>, now: OffsetDateTime) -> Option<OffsetDateTime>; // for T-506
pub async fn start_step_up(state: &AppState, session: &LoadedSession, authed: &AuthedSession) -> Result<(StartResponse, Option<NewCookie>), ApiError>;
pub async fn finish_step_up(ctx: CallbackContext<'_>) -> (Outcome, Option<NewCookie>);

// session/extract.rs
pub struct SteppedUpUser(pub AuthedSession);    // AuthedSession plus require_step_up
pub struct SteppedUpAdmin(pub AuthedSession);   // AdminSession plus require_step_up (admin check first)
```

## Algorithm

### Start (`intent: "step_up"`)

1. Session must be `Authenticated` (else `401`).
2. `login_hint`: the primary mailbox's address (`mailboxes().by_user`, `is_primary`, opened with `KeyService` and `aad_fields::MAILBOX_EMAIL`). If it cannot be opened, omit the hint; never fail the step-up for it.
3. `begin_oauth(state, session, AuthIntent::StepUp, None, OAuthParams { scopes: &STEP_UP_SCOPES, prompt: Some(Prompt::Login), max_age_s: Some(STEP_UP_MAX_AGE_S), login_hint })`. The session stays `Authenticated`; its ID rotates and `pre_auth` holds the round trip (T-501).
4. Nothing about the waiting action is stored (S7 3.6); the app keeps it and resends with the same `Idempotency-Key`.

### Callback (`finish_step_up`)

The generic callback (T-502b) has already checked `state`, exchanged the code and validated the ID token with the sealed `nonce`.

1. The session must still be `Authenticated` with the same user as when started; else `failed`.
2. `sub` check: `mailboxes().by_subject(Gmail, sub)` must exist and its `user_id` must equal the session's user. Else outcome `step_up_wrong_account`, `security_event { action: "step_up", outcome: "wrong_account", user }`, nothing changes (`recent_auth_at` untouched).
3. `auth_time` check with `now = clock.now()`: `claims.auth_time` must be present (a missing claim means Google did not re-authenticate: refuse), `now - auth_time <= STEP_UP_WINDOW_S`, and `auth_time <= now + 60 s`. Otherwise outcome `failed`, `security_event { action: "step_up", outcome: "stale_auth_time" }`. This refuses a freshly issued ID token from a silent re-issue whose `auth_time` is old (S10 7.2).
4. Success: `SessionService::rotate(current, |r| { r.recent_auth_at = Some(auth_time); r.pre_auth = None; })` (new session ID, same `session_record_id`, so the undo stack survives; V7.2.4). `security_event { action: "step_up", outcome: "success", amr }`. Outcome `stepped_up`.
5. Tokens: the step-up grant is used only for its ID token (S7 3.4). The `sub` is linked, so T-502b's `discard_tokens` drops them without revoking.
6. `recent_auth_at` is set to `auth_time`, not `now` `[DEFAULT]`: the window then measures from the real Google sign-in, which is what V6.8.4 asks for.

### `require_step_up`

1. `recent_auth_at` is `None`: `StepUpRequired`.
2. `now - t > STEP_UP_WINDOW_S` (300 s passes; 300 s and 1 s fails) or `t > now + 60 s`: `StepUpRequired`.
3. Else `Ok(())`.
4. `SteppedUpAdmin` runs the admin check first (a non-admin gets `403 forbidden`, not `step_up_required`), then `require_step_up`.

### Route coverage

Every route on S7's step-up list uses `SteppedUpUser` or `SteppedUpAdmin`: API-ACCT-1 (T-803), API-AUTH-1 `link` (T-601a), API-MBX-2 (T-601b), API-ADM-2, 3, 4, 6, 7 (T-505), API-ADM-9 (T-906b), API-ADM-11, 14 (T-908b), API-ADM-15 (T-804). This task adds a test `step_up_route_list_matches_s7` that holds that list as a constant and checks each listed route template rejects a session without step-up; route tasks add their template when they land.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-01 AC5 | An admin write without a fresh Google sign-in in 5 minutes is refused with `step_up_required` |
| V6.8.4 | Step-up uses `max_age=300` and checks the ID token `auth_time` |
| V7.2.4 | The session ID rotates at step-up; the old ID is dead |
| V7.5.1 | Sensitive actions need a fresh Google sign-in within 5 minutes |
| V16.3.1 | Step-up successes and failures are logged |

## Tests that must pass

Service integration against `fake-google` (T-206 `NextLogin.auth_age_s`, `TokenScenario::MissingAuthTime`), virtual clock, test routes `POST /api/v1/__t/sensitive` (`SteppedUpUser`) and `POST /api/v1/admin/__t/write` (`SteppedUpAdmin`).

- `au_01_ac5_admin_write_without_step_up_refused`
- `au_01_ac5_admin_write_after_step_up_allowed`
- `asvs_v6_8_4_authorize_url_has_prompt_login_and_max_age_300` (unit on the built URL)
- `asvs_v6_8_4_missing_auth_time_refused`
- `asvs_v6_8_4_stale_auth_time_refused_even_if_token_fresh` (`auth_age_s = 301`)
- `asvs_v7_2_4_step_up_rotates_id_keeps_record_id` (old cookie 401; sealed undo token from T-303 still opens)
- `asvs_v7_5_1_window_boundary_300_ok_301_refused`
- `asvs_v7_5_1_wrong_google_account_refused_nothing_changed` (outcome `step_up_wrong_account`, `recent_auth_at` unchanged)
- `asvs_v7_5_1_account_linked_to_other_user_refused` (`sub` belongs to another user)
- `asvs_v16_3_1_step_up_success_and_failure_logged`
- `step_up_non_admin_gets_forbidden_not_step_up_required`
- `step_up_tokens_never_revoked` (fake-google records no revocation)
- `step_up_route_list_matches_s7`

## Edge cases and traps

- S7 section 3.6, API-AUTH-1 and the OpenAPI `intent` description say `max_age=0`; S6 4, S4 3.1, S2 and ASVS V6.8.4 say `max_age=300`. Use 300 (decided for the backlog); `prompt=login` forces the password prompt either way.
- Never default a missing `auth_time` to `iat` or `now`.
- Compare `sub`, never email: the wrong-account check must hold even if two Google accounts share an address alias.
- Do not revoke step-up tokens; the grant is shared with the linked mailbox.
- Admin check before step-up check, so non-admins learn nothing about step-up state.
- Do not store the waiting action or its body server side.
- Use `ports.clock`; the boundary test depends on it.

## Out of scope

- The overlay and resending the waiting request: T-1001a.
- Each sensitive route itself: T-505, T-601a, T-601b, T-803, T-804, T-906b, T-908b.
- `step_up_valid_until` in `GET /session`: T-506 (uses the helper here).

## Security review checklist

- The authorisation request for step-up always carries `prompt=login` and `max_age=300` and requests only `openid email`.
- The callback accepts only an ID token whose `sub` maps to a mailbox of the same user and whose `auth_time` is present and within 300 seconds of the injected clock.
- A refused step-up leaves `recent_auth_at` and the session state unchanged apart from clearing `pre_auth`.
- A successful step-up rotates the session ID and keeps `session_record_id`.
- `SteppedUpAdmin` checks the admin flag from the user record before step-up.
- No step-up path revokes provider tokens or logs `sub`, email or tokens.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `SteppedUpUser` and `SteppedUpAdmin` are exported for the route tasks listed under "Route coverage".
