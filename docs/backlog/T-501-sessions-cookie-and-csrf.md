# T-501: Sessions, cookie and CSRF

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 550 lines of code plus tests | T-201b, T-202a, T-301, T-302, T-500 |

**Read only these spec sections:** S7 section 2.1 bullet "Session record ID", 3.2, 3.3 (`docs/specs/S7-api-contract.md`); S6 section 4 bullet "Sessions" and section 3 rows T7, T8 (`docs/specs/S6-security.md`); S3 `Session` row (`docs/specs/S3-domain-model.md`); S5 `sessions/{id}` rows, Browser table and SES-1 (`docs/specs/S5-data-inventory.md`); S2 AU-03 AC5, AU-07 AC1, AC6 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 7.2 rows "Session", "One session per user", "CSRF and CORS" (`docs/specs/S10-test-strategy.md`); ASVS register sections V3.3, V3.5, V7.2, V7.3, V7.4, V7.5, V8.2 and rows V16.3.2, V16.3.3 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-201b-server-store-traits.md` `SessionRecord`, `PreAuthFields`, `SessionRepo`, `aad_fields`. Nothing else is needed.

## Goal

The api gets server-side sessions behind the `__session` cookie: creation, lookup by hash, idle and absolute timeouts, rotation that keeps `session_record_id`, one session per user, encrypted `pre_auth` fields, the extractors every route uses (`AuthedSession`, `AdminSession` and friends), the CSRF synchroniser token plus `Origin` check on every unsafe method, and the default read and write rate limits. Sign-in (T-502b), step-up (T-504), the session endpoint and sign-out (T-506) and every user route build on it.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/session/mod.rs` | `pub mod cookie; pub mod store; pub mod extract; pub mod csrf; pub mod pre_auth;` |
| Create | `backend/crates/api/src/session/cookie.rs` | `SESSION_COOKIE`, `set_cookie`, `clear_cookie`, `read_cookie` |
| Create | `backend/crates/api/src/session/store.rs` | `SessionService`: create, load, touch, rotate, establish, end |
| Create | `backend/crates/api/src/session/pre_auth.rs` | Seal and open `PreAuthFields` |
| Create | `backend/crates/api/src/session/extract.rs` | `LoadedSession`, `AuthedSession`, `AdminSession`, `PendingInviteSession`, `AnySession` |
| Create | `backend/crates/api/src/session/csrf.rs` | `csrf_layer` (token and `Origin`) |
| Change | `backend/crates/api/src/lib.rs` | `pub mod session;`; add the session layer, CSRF layer and default limits to `build_router` |
| Create | `backend/crates/api/tests/sessions.rs` | Service integration tests |
| Change | `backend/crates/api/Cargo.toml` | `subtle = "2"`, `base64 = "0.22"` |

## Types and signatures

```rust
// cookie.rs
pub const SESSION_COOKIE: &str = "__session";   // fixed by Firebase Hosting (S7 3.2)
pub fn set_cookie(raw: &RawSessionId) -> HeaderValue;   // "__session=<v>; Path=/; Secure; HttpOnly; SameSite=Lax"
pub fn clear_cookie() -> HeaderValue;                    // "__session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
pub fn read_cookie(headers: &HeaderMap) -> Option<RawSessionId>;

/// 32 random bytes, base64url without padding (43 chars). Never stored, never logged.
pub struct RawSessionId(Sensitive<String>);
impl RawSessionId { pub fn hash(&self) -> SessionHash; }  // SHA-256 of the 43-char string's bytes

// store.rs
pub const IDLE_TIMEOUT: Duration = Duration::minutes(15);      // decided, S6 9
pub const ABSOLUTE_TIMEOUT: Duration = Duration::hours(12);    // decided, S6 9
pub const PRE_AUTH_TTL: Duration = Duration::minutes(10);      // [TUNABLE] S7 3.2
pub const TOUCH_INTERVAL: Duration = Duration::seconds(60);    // [DEFAULT] write last_seen_at at most once a minute

pub struct LoadedSession { pub raw: RawSessionId, pub record: Versioned<SessionRecord> }
pub struct NewCookie(pub HeaderValue);                         // handlers add it as Set-Cookie

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason { SignedOut, Replaced, AdminEnded, AccountDeleted }  // security event outcomes: signed_out, replaced, admin_ended, account_deleted

pub struct SessionService<'a> { state: &'a AppState }
impl<'a> SessionService<'a> {
    pub fn new(state: &'a AppState) -> Self;
    /// New anonymous pre_auth session (S7 API-AUTH-3 state "anonymous").
    pub async fn create_anonymous(&self) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError>;
    /// Cookie to live record, enforcing every timeout; expired records are deleted. Touches last_seen_at.
    pub async fn load(&self, headers: &HeaderMap) -> Result<Option<LoadedSession>, ApiError>;
    /// New cookie value, same session_record_id, same created_at; `edit` changes other fields. Old record deleted.
    pub async fn rotate(&self, current: &LoadedSession, edit: impl FnOnce(&mut SessionRecord) + Send) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError>;
    /// Sign-in and join: a brand-new authenticated session for `user`; deletes `current` and every other
    /// session of the user (one session per user) and logs session_end/replaced for each.
    pub async fn establish(&self, current: Option<&LoadedSession>, user: &UserId, recent_auth_at: Option<OffsetDateTime>) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError>;
    /// Deletes one record and logs session_end with the reason.
    pub async fn end(&self, hash: &SessionHash, user: Option<&UserId>, reason: EndReason, request_id: RequestId) -> Result<(), ApiError>;
    /// Admin end and account deletion: deletes every session of the user.
    pub async fn end_all_for_user(&self, user: &UserId, reason: EndReason, request_id: RequestId) -> Result<u64, ApiError>;
}

// pre_auth.rs
pub struct PreAuthPlain {                    // Debug written by hand: intent only
    pub intent: AuthIntent,
    pub oauth_state: Sensitive<String>,
    pub nonce: Sensitive<String>,
    pub pkce_verifier: Sensitive<String>,
    pub invite_token_hash: Option<Sha256Hash>,
    pub pending_email: Option<Sensitive<String>>,
    pub started_at: OffsetDateTime,
}
pub async fn seal_pre_auth(keys: &dyn SystemKeyService, record_id: &SessionRecordId, p: &PreAuthPlain) -> Result<PreAuthFields, ApiError>;
pub async fn open_pre_auth(keys: &dyn SystemKeyService, record_id: &SessionRecordId, f: &PreAuthFields) -> Result<PreAuthPlain, ApiError>;

// extract.rs (axum extractors; each reads the LoadedSession the session layer put in request extensions)
pub struct AnySession(pub LoadedSession);                         // any state; 401 if none
pub struct AuthedSession {                                        // state Authenticated; 401 otherwise
    pub user: UserId,
    pub session_record_id: SessionRecordId,
    pub is_admin: bool,                                           // read from UserRecord on every request
    pub recent_auth_at: Option<OffsetDateTime>,
    pub session_hash: SessionHash,
}
pub struct AdminSession(pub AuthedSession);                       // 403 forbidden + authz_failure event if not admin
pub struct PendingInviteSession { pub loaded: LoadedSession }     // state PendingInviteRequest; 401 otherwise

// csrf.rs
pub const CSRF_HEADER: &str = "x-csrf-token";
pub async fn csrf_layer(State(state): State<AppState>, req: Request, next: Next) -> Response;
```

T-601a lists `AuthedSession.session_record_id` as `Uuid`; use T-201b's `SessionRecordId(Uuid)` and say so in the pull request. T-601a's `rotate_session(&AppState, &AuthedSession)` is `SessionService::rotate` here.

## Algorithm

### Cookie and IDs

1. `RawSessionId`: `ports.rng.bytes32()`, base64url no padding. `hash()` = SHA-256 of the string bytes, as `SessionHash`; the document key is `SessionHash::to_hex()`.
2. `read_cookie`: parse every `Cookie` header; take the first `__session` pair; accept only a 43-character `[A-Za-z0-9_-]` value, else `None`. A value the server did not issue simply finds no record (fixation: the server never adopts a client-chosen ID).
3. `set_cookie`: exactly `__session=<v>; Path=/; Secure; HttpOnly; SameSite=Lax`. No `Domain`, no `Max-Age` or `Expires` (a browser-session cookie; the server enforces lifetime) `[DEFAULT]`. The session ID appears only in `Set-Cookie`, never in a body or URL.

### Load (session layer, runs on every `/api/v1` request after the request ID layer)

1. `read_cookie`; none gives `None`.
2. `store.sessions().get(&hash)`; none gives `None`.
3. With `now = ports.clock.now()`, the record is dead when any holds: `now >= created_at + ABSOLUTE_TIMEOUT`; `now >= last_seen_at + IDLE_TIMEOUT`; state `PreAuth` or `PendingInviteRequest` and `pre_auth` is `None` or `now >= pre_auth.started_at + PRE_AUTH_TTL`. Dead: delete with `Precondition::Matches(version)` (ignore `PreconditionFailed`) and return `None`.
4. State `Authenticated` with `pre_auth` set and `now >= pre_auth.started_at + PRE_AUTH_TTL` (an abandoned step-up or link round trip): clear `pre_auth` on the next write (step 6).
5. State `Authenticated`: `users().get(user_id)` must exist, else delete the session and return `None`.
6. Touch when `now - last_seen_at >= TOUCH_INTERVAL` or step 4 applies: `last_seen_at = now`, `expires_at = min(now + IDLE_TIMEOUT, created_at + ABSOLUTE_TIMEOUT)`, put with `Matches(version)`. On `PreconditionFailed` re-read once; if the record is gone (rotated or ended by another request), return `None`.
7. Put `LoadedSession` in request extensions. Then apply the default rate limit (`limits::default_policy(method)`): reads keyed by `Session(session_record_id)`, writes keyed by `User` when authenticated else `Session`; no session uses `Ip`.

### States and extractors

- `AuthedSession` requires state `Authenticated`. Missing or other state: `401 unauthenticated` (S7 3.2 "Any other call returns 401").
- `AdminSession` reads `UserRecord.is_admin` (never from the client or a cached cookie). Not admin: `403 forbidden` and `security_event { action: "authz_failure", outcome: "refused", .. }` with the route template (V16.3.2, AU-01 AC3).
- `PendingInviteSession` requires `PendingInviteRequest`. `AnySession` accepts all three.
- A user route never reads the user ID from the request body or path; always from `AuthedSession` (V8.2.1, deny by default).

### Rotation and one session per user

1. `rotate(current, edit)`: new `RawSessionId`; copy the record; set the new `session_hash`; keep `session_record_id`, `created_at`, `user_id`, `recent_auth_at`; new `csrf_token` (`rng.bytes32()` base64url) `[DEFAULT]` (the app re-reads `GET /session` after every redirect); apply `edit`; put the new record with `MustNotExist`; delete the old with `Precondition::None`. Return the cookie. Used by step-up, link and reconnect (S7 3.2: the ID rotates; `session_record_id` does not).
2. `establish(current, user, recent_auth_at)`: new `RawSessionId`, new `session_record_id` (`rng.uuid_v4()`), new CSRF token, `created_at = last_seen_at = now`, state `Authenticated`, `pre_auth: None`, `expires_at = now + IDLE_TIMEOUT`. Put with `MustNotExist`. Delete `current` if given. Then `sessions().by_user(user)`; delete every record whose hash is not the new one, and for each write `security_event { action: "session_end", outcome: "replaced", user: pseudo(user) }` (AU-07 AC1, AC6).
3. Two sign-ins racing may each delete the other's new session; the user then signs in again. Acceptable; never two live sessions.
4. `end(hash, ..)`: delete with `Precondition::None`; `security_event { action: "session_end", outcome: <reason> }`.

### `pre_auth` sealing

- Each secret field is sealed with `SystemKeyService::seal(&SystemAad { scope: session_record_id.0.to_string(), field: aad_fields::PRE_AUTH_* }, bytes)`. `invite_token_hash` is already a hash and is stored as is. `intent` and `started_at` are clear.
- `open_pre_auth` failure (wrong scope, tampered) is `ApiError::Unauthenticated` for the callback's purposes; T-502b turns it into `outcome=failed`.
- Because the scope is `session_record_id`, rotating keeps `pre_auth` openable; `establish` drops `pre_auth` (the S5 rule: cleared on the state change).

### CSRF (S7 3.3)

Runs for `POST`, `PUT`, `PATCH`, `DELETE` under `/api/v1`, before the handler, after the session layer:

1. `Origin` header must be present and byte-equal to `config.app_origin`. Absent, `null` or different: fail.
2. A `LoadedSession` must exist, and the `X-CSRF-Token` header must equal `record.csrf_token`, compared with `subtle::ConstantTimeEq` on bytes (different lengths: fail).
3. Fail: `403 csrf_failed` and `security_event { action: "csrf_failure", outcome: "refused", user: pseudo if authenticated }` (V16.3.3).
4. `GET` and `HEAD` are never CSRF checked and must never change state (V3.5.3); the OAuth callback is a `GET` protected by `state` (T-502b). There is no exemption list.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-03 AC5 | Cookie `__session`, HttpOnly, Secure, SameSite=Lax; ID rotates at sign-in; idle 15 minutes, absolute 12 hours |
| AU-07 AC1 | A new sign-in ends the user's other session; its next request gets 401 |
| AU-07 AC6 | A session ended by a new sign-in is recorded as a security event |
| SES-1 | Expired, signed-out and superseded sessions cannot be used; one record per user; `pre_auth` fields cleared on state change and gone after 10 minutes |
| V3.3.1 | `Secure` set (accepted prefix deviation asserted by attribute) |
| V3.3.2 | `SameSite=Lax` |
| V3.3.3 | No `Domain`; `Path=/` (accepted prefix deviation asserted by attribute) |
| V3.3.4 | `HttpOnly`; the session ID only ever travels in `Set-Cookie` |
| V3.5.1 | Synchroniser token plus `Origin` check on every unsafe method |
| V3.5.3 | `GET` never changes state |
| V7.2.1 | Session looked up by hash on the server |
| V7.2.2 | Random reference token per session |
| V7.2.3 | 256-bit CSPRNG session IDs |
| V7.2.4 | New ID at sign-in and rotation; the old ID is dead |
| V7.3.1 | Idle timeout 15 minutes |
| V7.3.2 | Absolute lifetime 12 hours, not reset by rotation |
| V7.4.3 | One session per user |
| V7.5.2 | One session per user: a new sign-in ends the old one |
| V8.2.1 | Route auth level enforced by extractors, deny by default |
| V16.3.2 | Failed admin authorisation is logged |
| V16.3.3 | CSRF failures are logged |

## Tests that must pass

Service integration tests use the router with T-202 fakes, the virtual clock and test routes (`GET /api/v1/__t/me` with `AuthedSession`, `POST /api/v1/__t/write` with `AuthedSession`, `GET /api/v1/__t/admin` with `AdminSession`) plus a test helper that calls `SessionService::establish` for a seeded user.

- `au_03_ac5_cookie_attributes_exact` (service integration: parse `Set-Cookie`, assert name, `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, no `Domain`, no `Max-Age`)
- `au_03_ac5_id_rotates_at_sign_in` (service integration: anonymous cookie differs from authenticated cookie; old cookie gives 401)
- `au_03_ac5_idle_timeout_15_minutes` (service integration: 14:59 ok, 15:00 refused)
- `au_03_ac5_absolute_timeout_12_hours_despite_activity` (service integration: touch every 10 minutes, refused at 12:00)
- `au_07_ac1_new_sign_in_ends_old_session` (service integration: two establishes; first cookie 401)
- `au_07_ac6_replaced_session_logged` (service integration, `obs::capture`)
- `ses_1_signed_out_session_refused` (service integration via `end`)
- `ses_1_one_record_per_user_after_new_sign_in` (service integration: fake store holds one session for the user)
- `ses_1_pre_auth_gone_after_10_minutes` (service integration)
- `ses_1_pre_auth_cleared_on_establish` (service integration)
- `asvs_v3_3_1_cookie_secure` , `asvs_v3_3_2_cookie_samesite_lax`, `asvs_v3_3_3_cookie_no_domain_path_root`, `asvs_v3_3_4_cookie_httponly_and_id_not_in_body` (service integration)
- `asvs_v3_5_1_missing_csrf_token_refused`, `asvs_v3_5_1_wrong_csrf_token_refused`, `asvs_v3_5_1_foreign_origin_refused`, `asvs_v3_5_1_missing_origin_refused`, `asvs_v3_5_1_token_from_other_session_refused` (service integration)
- `asvs_v3_5_3_get_never_requires_or_changes_state` (service integration: a `GET` route cannot be reached by `POST` and vice versa)
- `asvs_v7_2_1_store_holds_hash_not_raw_id` (service integration: the raw cookie value appears nowhere in the fake store dump)
- `asvs_v7_2_2_unknown_cookie_value_not_adopted` (service integration: a client-chosen 43-char value gives no session and is not created)
- `asvs_v7_2_3_session_id_is_32_random_bytes` (unit: decodes to 32 bytes from `Rng::bytes32`)
- `asvs_v7_2_4_rotate_kills_old_id_keeps_record_id` (service integration)
- `asvs_v7_3_1_idle_timeout`, `asvs_v7_3_2_absolute_timeout` (service integration, may share helpers with the AU tests)
- `asvs_v7_4_3_single_session_per_user`, `asvs_v7_5_2_new_sign_in_ends_old` (service integration)
- `asvs_v8_2_1_user_route_without_session_401` (service integration)
- `asvs_v16_3_2_non_admin_on_admin_route_logged` (service integration)
- `asvs_v16_3_3_csrf_failure_logged` (service integration)
- `session_touch_written_at_most_once_a_minute` (service integration: count fake store writes)
- `session_pre_auth_seal_bound_to_record_id` (unit: fields sealed for one record ID do not open under another)

## Edge cases and traps

- Rotation must keep `created_at`; resetting it would let a busy user live forever (V7.3.2).
- Store only the hash; never log the raw ID, the CSRF token or the cookie header.
- Compare CSRF tokens in constant time; `==` on strings is not enough.
- `Origin` must match exactly; do not accept a prefix match or a `Referer` fallback.
- Do not exempt any `POST` from CSRF, including sign-out and `auth/start` (S7 3.3; S10 7.2 "link" and "reauth").
- `is_admin` comes from the user record on each request, never from the session record or the client.
- Expired records are deleted when seen; the sweeper (T-706) is the backstop, not the control.
- `pre_auth` uses `SystemKeyService` because no user `data_key` exists yet; never put the PKCE verifier, `state` or `nonce` in clear in the store.
- Do not set `Max-Age` on the session cookie, and do not add a second cookie: Firebase Hosting strips every other name.
- Use `ports.clock` for every time comparison; tests advance the virtual clock.

## Out of scope

- OAuth start and callback: T-502b. Step-up rules and `require_step_up`: T-504. `GET /session` and sign-out: T-506. Admin "end a user's session" route: T-804.
- Sweeping expired session records: T-706.

## Security review checklist

- Session IDs come from `Rng::bytes32` (CSPRNG in production), 256 bits, base64url; only SHA-256 hashes reach Firestore.
- Every timeout check uses `created_at` (absolute), `last_seen_at` (idle) and `pre_auth.started_at` (10 minutes), and a dead record is deleted before any handler runs.
- `establish` deletes every other session of the user and logs each; `rotate` keeps `session_record_id` and `created_at` and kills the old ID.
- `Set-Cookie` attributes match S7 3.2 exactly, and no response body, log line or URL contains the session ID.
- CSRF layer covers every unsafe method on every `/api/v1` route with no exemptions, checks `Origin` exactly and compares tokens in constant time.
- `AuthedSession` and `AdminSession` read identity only from the server record; admin refusal is logged.
- `pre_auth` fields are sealed with associated data bound to `session_record_id` and the field name.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `build_router` applies session, CSRF and default-limit layers to every `/api/v1` route, and T-500's tests still pass.
