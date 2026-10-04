# T-506: Session endpoint and sign-out

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | sonnet | about 200 lines of code plus tests | T-501, T-503, T-504 |

**Read only these spec sections:** S7 section 5.2 API-AUTH-3 and API-AUTH-4, section 3.2 (`docs/specs/S7-api-contract.md`); the OpenAPI paths `/session`, `/auth/sign-out` and schemas `Session`, `SessionState`, `Mailbox` (`docs/specs/S7-api-contract.openapi.yaml`); S2 AU-07 AC1, AC2, AC6 (`docs/specs/S2-v1-acceptance-criteria.md`); S5 Browser table and SES-1 (`docs/specs/S5-data-inventory.md`); ASVS register rows V7.4.1, V14.3.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-501-sessions-cookie-and-csrf.md` "Types and signatures". Nothing else is needed.

## Goal

The app can always ask `GET /api/v1/session` what state it is in (it never gets a 401 there, and gets a CSRF token even before sign-in), and can sign out with `POST /api/v1/auth/sign-out`, which ends the session on the server and tells the browser to clear its stored data.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/session.rs` | API-AUTH-3 and API-AUTH-4 handlers and DTOs |
| Change | `backend/crates/api/src/lib.rs` | Mount both routes; add templates |
| Create | `backend/crates/api/tests/session_endpoint.rs` | Service integration tests |

## Types and signatures

```rust
// routes/session.rs
#[derive(Serialize)]
pub struct SessionDto {
    pub state: &'static str,                 // "anonymous" | "pending_invite_request" | "authenticated"
    pub csrf_token: String,
    pub user: Option<SessionUserDto>,
    pub pending_invite_email: Option<String>,
    pub step_up_valid_until: Option<String>, // RFC 3339 UTC
    pub mailboxes: Vec<SessionMailboxDto>,   // empty unless authenticated
    pub idle_expires_at: Option<String>,
    pub absolute_expires_at: Option<String>,
}
#[derive(Serialize)] pub struct SessionUserDto { pub user_id: Uuid, pub is_admin: bool }
#[derive(Serialize)] pub struct SessionMailboxDto { pub mailbox_id: Uuid, pub provider: &'static str, pub email_address: String, pub status: &'static str }

pub async fn get_session(State(state): State<AppState>, headers: HeaderMap, Extension(rid): Extension<RequestId>) -> Result<Response, ApiError>;
pub async fn sign_out(State(state): State<AppState>, AnySession(s): AnySession, Extension(rid): Extension<RequestId>) -> Result<Response, ApiError>;
```

## Algorithm

### API-AUTH-3 `GET /api/v1/session`

1. `SessionService::load(headers)`. `None` (no cookie, unknown or expired): `create_anonymous()` and add its `Set-Cookie`. Never return `401`.
2. Map the state: `PreAuth` gives `"anonymous"`; `PendingInviteRequest` gives `"pending_invite_request"`; `Authenticated` gives `"authenticated"`. Match without `_`.
3. `csrf_token`: the record's token.
4. `pending_invite_email`: only in `pending_invite_request`, from `open_pre_auth(..).pending_email`.
5. Authenticated only:
   - `user`: `{ user_id, is_admin }` from the user record.
   - `step_up_valid_until`: T-504 `step_up_valid_until(recent_auth_at, now)` (`recent_auth_at + 5 minutes` when still in the future, else `null`).
   - `mailboxes`: `mailboxes().by_user(user)` (linked order); `email_address` opened with `KeyService` and `Aad { user, scope: mailbox_id, field: aad_fields::MAILBOX_EMAIL }`; `provider` `"gmail"` (match `Provider` without `_`); `status` `"connected"` or `"needs_sign_in"` (`consent_blocked` is v2 and cannot occur).
   - `idle_expires_at = min(last_seen_at + 15 min, created_at + 12 h)`, `absolute_expires_at = created_at + 12 h`.
6. Other states: those four fields `null` or empty.
7. `200`, JSON. The T-500 layer adds `Cache-Control: no-store` (the body holds addresses).

### API-AUTH-4 `POST /api/v1/auth/sign-out`

1. Needs any session (`AnySession`) and passes CSRF (T-501). No session: `401`.
2. `SessionService::end(hash, user, EndReason::SignedOut, rid)`: deletes the record and logs `session_end` with outcome `signed_out` (AU-07 AC6).
3. Response `204` with `Set-Cookie: clear_cookie()` and `Clear-Site-Data: "cache", "storage"` (exactly this value; not `"cookies"`, which would also clear the `__session` cookie for other tabs mid-request and is not in S7).
4. Stored refresh tokens stay, so queued unsubscribe jobs still run (S7 5.2).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-07 AC1 | After another sign-in, the old browser's `GET /session` reports `anonymous` and user routes give 401 |
| AU-07 AC2 | Sign-out ends the session on the server and sends `Clear-Site-Data: "cache", "storage"` |
| AU-07 AC6 | Sign-out is recorded as a security event |
| SES-1 | A signed-out session cannot be used again |
| V7.4.1 | Sign-out deletes the session record |
| V14.3.1 | `Clear-Site-Data` on sign-out |

## Tests that must pass

- `au_07_ac1_old_session_reports_anonymous_after_new_sign_in` (service integration)
- `au_07_ac2_sign_out_deletes_record_and_clears_site_data` (service integration: header value exact, cookie cleared with `Max-Age=0`)
- `au_07_ac6_sign_out_logged` (service integration, `obs::capture`)
- `ses_1_signed_out_cookie_refused_on_user_route` (service integration)
- `asvs_v7_4_1_sign_out_removes_server_record` (service integration: fake store has no session for the user)
- `asvs_v14_3_1_clear_site_data_cache_and_storage` (service integration)
- `session_endpoint_never_401_and_creates_anonymous` (service integration: no cookie, garbage cookie, expired cookie)
- `session_endpoint_authenticated_shape_matches_openapi` (service integration: field names and types)
- `session_endpoint_step_up_valid_until_null_after_window` (service integration, virtual clock)
- `session_endpoint_pending_shows_pending_email_only_in_that_state` (service integration)
- `sign_out_without_csrf_token_refused` (service integration)

## Edge cases and traps

- `GET /session` must not require CSRF and must not change anything except creating an anonymous session when none exists (that is the documented exception in S7 5.2).
- Do not include the session ID, refresh token status or `session_record_id` in the response.
- Never log the mailbox addresses in the response.
- Sign-out must not revoke provider tokens or delete mailboxes.
- `state` is `"anonymous"` for a `pre_auth` record, not `"pre_auth"` (OpenAPI `SessionState`).

## Out of scope

- Ending another user's session (admin): T-804. Account deletion ending the session: T-803.
- App-side clearing of memory on sign-out and on any 401: T-1001a and T-007.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Both responses validate against the OpenAPI `Session` schema and `/auth/sign-out` definition.
