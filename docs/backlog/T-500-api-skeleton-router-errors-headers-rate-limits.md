# T-500: api service skeleton: router, errors, headers, rate limits

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | sonnet | about 550 lines of code plus tests | T-202a, T-202b, T-307 |

**Read only these spec sections:** S7 section 1, section 2 (table only, not 2.1), section 4 (all), section 6 (all) (`docs/specs/S7-api-contract.md`); the `Problem`, `ProblemCode` and `RateLimited` components of `docs/specs/S7-api-contract.openapi.yaml`; S5 "Logs and telemetry" and the `rate_limits` row (`docs/specs/S5-data-inventory.md`); ASVS register rows V1.5.2, V2.4.1, V3.4.1, V3.4.2, V3.4.4, V3.4.5, V3.4.6, V3.5.2, V4.1.1, V4.1.3, V13.4.4, V13.4.5, V14.2.2, V14.3.2, V15.3.3, V15.3.4, V16.5.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/CONVENTIONS.md`; T-307's `obs` API (`docs/backlog/T-307-structured-logging-redaction.md`, Types section). Nothing else is needed.

## Goal

The `api` crate becomes a running `axum` service with everything every route needs and no business routes yet: shared state, the S7 section 4 error model as RFC 9457 Problem Details, request IDs, the security and caching headers on every response, strict JSON input (size, content type, unknown fields), client IP from the trusted proxy position, and the S7 section 6 rate limiter with its response headers. Every later M5 to M9 api task builds on these names.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/lib.rs` | `pub mod config; pub mod error; pub mod http; pub mod limits; pub mod state;` and `pub fn build_router(state: AppState) -> Router` |
| Create | `backend/crates/api/src/main.rs` | Load config and secrets, build real `Ports`, `obs::init("api", ...)`, bind `0.0.0.0:$PORT` |
| Create | `backend/crates/api/src/config.rs` | `ApiConfig` |
| Create | `backend/crates/api/src/state.rs` | `AppState` |
| Create | `backend/crates/api/src/error.rs` | `ApiError`, Problem body |
| Create | `backend/crates/api/src/http/mod.rs` | `pub mod headers; pub mod json; pub mod request_id; pub mod client_ip;` |
| Create | `backend/crates/api/src/http/headers.rs` | Security and caching headers layer |
| Create | `backend/crates/api/src/http/json.rs` | `ApiJson<T>` extractor and JSON response helper |
| Create | `backend/crates/api/src/http/request_id.rs` | `RequestId` extension and layer |
| Create | `backend/crates/api/src/http/client_ip.rs` | `ClientIp` from `X-Forwarded-For` |
| Create | `backend/crates/api/src/limits.rs` | `Policy`, `POLICIES`, `RateLimiter`, `LimitSubject` |
| Create | `backend/crates/api/tests/skeleton.rs` | Service integration tests over a test-only router |
| Change | `backend/crates/api/Cargo.toml` | `axum = "0.7"`, `tower = "0.4"`, `tower-http = { version = "0.5", features = ["limit"] }`, `serde_path_to_error = "0.1"`, `hmac = "0.12"`, `sha2 = "0.10"`, `hex = "0.4"` |

## Types and signatures

```rust
// config.rs
pub struct ApiConfig {
    pub app_origin: String,              // exact, e.g. "https://mailtinder.example.com"; no trailing slash
    pub google_client_id: String,
    pub oauth_redirect_uri: Url,         // "{app_origin}/api/v1/auth/google/callback" (T-502b)
    pub xff_trusted_hops: usize,         // [DEFAULT] 1; see client_ip
    pub rate_key: Sensitive<Vec<u8>>,    // HMAC key for rate-limit keys: the log pseudonymisation key (Secrets)
    pub email_lookup_key: Sensitive<Vec<u8>>, // Secrets::EmailLookupHmacKey; used from T-502b on
    pub max_body_bytes: usize,           // 16 * 1024 [TUNABLE] S7 2
}

// state.rs
#[derive(Clone)]
pub struct AppState { pub ports: Arc<Ports>, pub config: Arc<ApiConfig>, pub limits: Arc<RateLimiter> }

// error.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    InvalidRequest { fields: Vec<String> },          // JSON pointers only, never values; at most 20
    Unauthenticated, CsrfFailed, Forbidden, StepUpRequired, NotFound,
    AppFolderMoveFailed, LastMailbox, MailboxNeedsSignIn { mailbox_id: Option<Uuid> },
    MessageChanged, CategoryExists, ConsentOutdated, ExperimentUnavailable,
    VersionsMixed, SnapshotLimit, NotAcceptable, UndoExpired, PayloadTooLarge, UnsupportedMediaType,
    RateLimited { retry_after_s: u64 },
    ProviderError { mailbox_id: Option<Uuid> },
    ProviderUnavailable { mailbox_id: Option<Uuid>, retry_after_s: Option<u64> },
    Internal,
}
impl ApiError {
    pub fn status(&self) -> StatusCode;
    pub fn code(&self) -> &'static str;   // the S7 section 4 `code`
    pub fn title(&self) -> &'static str;  // fixed per code, no request data
}
impl From<MailError> for ApiError { /* see Algorithm step 6 */ }
impl From<StoreError> for ApiError { /* Unavailable -> Internal; everything -> Internal (logged as op) */ }
impl IntoResponse for ApiError { /* problem+json, needs the RequestId: see step 4 */ }

// http/request_id.rs
#[derive(Clone, Copy, Debug)] pub struct RequestId(pub Uuid);   // request extension, set first

// http/json.rs
pub struct ApiJson<T>(pub T);   // extractor: content type, size, deny_unknown_fields (on T), error mapping
pub fn json_ok<T: Serialize>(status: StatusCode, body: &T) -> Response;

// http/client_ip.rs
#[derive(Clone, Debug, PartialEq, Eq)] pub struct ClientIp(pub Option<IpAddr>);
pub fn client_ip(headers: &HeaderMap, trusted_hops: usize) -> ClientIp;

// limits.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum KeyKind { Ip, User, Session, Mailbox, EmailHash, Subject }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum StoreKind { Memory, Firestore }
#[derive(Clone, Copy, Debug)]
pub struct Policy { pub name: &'static str, pub limit: u32, pub window_s: u64, pub burst: Option<u32>, pub key: KeyKind, pub store: StoreKind }
pub mod policies { use super::*;  // S7 section 6, every value [TUNABLE]
    pub const SIGN_IN_IP: Policy        = Policy { name: "sign_in_ip", limit: 20, window_s: 600, burst: None, key: KeyKind::Ip, store: StoreKind::Firestore };
    pub const SIGN_IN_FAILED_SUBJECT: Policy = Policy { name: "sign_in_failed", limit: 10, window_s: 3600, burst: None, key: KeyKind::Subject, store: StoreKind::Firestore };
    pub const INVITE_REQUEST_IP: Policy = Policy { name: "invite_request_ip", limit: 5, window_s: 3600, burst: None, key: KeyKind::Ip, store: StoreKind::Firestore };
    pub const INVITE_REQUEST_EMAIL: Policy = Policy { name: "invite_request_email", limit: 1, window_s: 86400, burst: None, key: KeyKind::EmailHash, store: StoreKind::Firestore };
    pub const SWIPES: Policy            = Policy { name: "swipes", limit: 60, window_s: 60, burst: Some(10), key: KeyKind::User, store: StoreKind::Memory };
    pub const FEED: Policy              = Policy { name: "feed", limit: 30, window_s: 60, burst: None, key: KeyKind::User, store: StoreKind::Memory };
    pub const UNSUB_JOBS: Policy        = Policy { name: "unsub_jobs", limit: 300, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const OTHER_READS: Policy       = Policy { name: "reads", limit: 120, window_s: 60, burst: None, key: KeyKind::Session, store: StoreKind::Memory };
    pub const OTHER_WRITES: Policy      = Policy { name: "writes", limit: 30, window_s: 60, burst: None, key: KeyKind::User, store: StoreKind::Memory };
    pub const ADMIN_INVITE_SENDS: Policy = Policy { name: "admin_invites", limit: 50, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const ADMIN_SESSION_KILL: Policy = Policy { name: "admin_session_kill", limit: 20, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const ACCOUNT_DELETE: Policy    = Policy { name: "account_delete", limit: 3, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const EXPERIMENT_OPT: Policy    = Policy { name: "experiment_opt", limit: 10, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const EXPERIMENT_ADMIN: Policy  = Policy { name: "experiment_admin", limit: 50, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const BAKEOFF_REPORT: Policy    = Policy { name: "bakeoff_report", limit: 30, window_s: 3600, burst: None, key: KeyKind::User, store: StoreKind::Memory };
    pub const SNAPSHOT_SAVE: Policy     = Policy { name: "snapshot_save", limit: 20, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
    pub const SNAPSHOT_DELETE: Policy   = Policy { name: "snapshot_delete", limit: 50, window_s: 86400, burst: None, key: KeyKind::User, store: StoreKind::Firestore };
}
/// The raw subject of a limit. Hashed with ApiConfig::rate_key before it becomes a RateLimitKey.
pub enum LimitSubject<'a> { Ip(&'a ClientIp), User(&'a UserId), Session(&'a SessionRecordId), Mailbox(&'a MailboxId), Email(&'a str), Subject(&'a str) }
#[derive(Clone, Copy, Debug)] pub struct LimitInfo { pub policy: &'static Policy, pub remaining: u32, pub reset_s: u64 }
pub struct RateLimiter { /* memory: Mutex<HashMap<String, Window>>, store: Arc<dyn ServerStore>, clock: Arc<dyn Clock>, key: Sensitive<Vec<u8>> */ }
impl RateLimiter {
    pub fn new(store: Arc<dyn ServerStore>, clock: Arc<dyn Clock>, key: Sensitive<Vec<u8>>) -> Self;
    /// Counts one hit. Over the limit: Err(ApiError::RateLimited) and a `rate_limit_hit` security event.
    pub async fn check(&self, policy: &'static Policy, subject: LimitSubject<'_>, request_id: RequestId) -> Result<LimitInfo, ApiError>;
}
/// Adds RateLimit-Policy and RateLimit headers to a successful response.
pub fn apply_limit_headers(resp: &mut Response, info: &LimitInfo);
```

`AdminSession` is defined by T-501 (it needs the session), not here; T-906b's note that it comes from T-500 should read T-501.

## Algorithm

1. **Router.** `build_router(state)` nests every route under `/api/v1` (this task adds none except test routes under `#[cfg(test)]`). Layer order, outermost first: request ID, security headers, request log, body limit, then routes. The fallback for any unmatched path or method (including `TRACE`, `OPTIONS`, `CONNECT`) returns `ApiError::NotFound` `[DEFAULT]` (S7 has no 405 code; deny by default reveals nothing). `main.rs` calls `obs::register_http_routes(ROUTE_TEMPLATES)` with the static list of templates each later task appends to.
2. **Request ID.** First layer: `RequestId(ports.rng.uuid_v4())`, inserted as an extension; every response, success or error, gets `X-Request-Id`. A client-sent `X-Request-Id` is ignored.
3. **Security headers** on every response, including errors and the fallback, set by overwriting (never appended twice):
   - `Cache-Control: no-store`, `Pragma: no-cache`
   - `X-Content-Type-Options: nosniff`
   - `Referrer-Policy: no-referrer`
   - `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`
   - `Strict-Transport-Security: max-age=31536000; includeSubDomains`
   - Remove any `Access-Control-*` header a handler might add. No CORS layer exists.
4. **Errors.** `ApiError::into_response` cannot see the request, so a layer (`problem_layer`) fills `request_id` and `type` after the handler: handlers return `ApiError` as a response extension plus status, and the layer renders the body. Body (`Content-Type: application/problem+json; charset=utf-8`):
   `{ "type": "https://mailtinder.app/problems/<code>", "title": <fixed>, "status": n, "code": "<code>", "request_id": "<uuid>" }` plus `mailbox_id`, `retry_after_seconds`, `fields` only when set. `RateLimited` and `ProviderUnavailable` with a retry value also set `Retry-After: <seconds>`. Titles: one fixed English phrase per code, for example `rate_limited` "Too many requests", `internal_error` "Something went wrong"; keep them in one `match` without `_`.
5. **Status by code** exactly per S7 section 4 (400, 401, 403 x3, 404, 409 x9, 406, 410, 413, 415, 429, 502, 503, 500).
6. **`From<MailError>`:** `Unauthorized` gives `MailboxNeedsSignIn`; `Forbidden` gives `ProviderError`; `NotFound` gives `MessageChanged`; `RateLimited { retry_after_s }` gives `ProviderUnavailable { retry_after_s: Some(..) }` (S7 6 last bullets); `Transient` gives `ProviderUnavailable { None }`; `Invalid(_)` gives `ProviderError`. Callers set `mailbox_id` afterwards with a `with_mailbox(id)` helper.
7. **Panics.** A `tower_http::catch_panic`-style layer turns a panic into `Internal` with no panic text in the body.
8. **JSON input (`ApiJson<T>`).**
   1. For `POST`, `PUT`, `PATCH`, `DELETE` with a body: `Content-Type` must be `application/json`, optionally with `; charset=utf-8` (case-insensitive), else `UnsupportedMediaType` (V3.5.2: a form post from another site cannot match).
   2. Body over `max_body_bytes` gives `PayloadTooLarge` (check `Content-Length` first, then the streamed size).
   3. Deserialize with `serde_path_to_error::deserialize(&mut serde_json::Deserializer::from_slice(..))`. On error, `InvalidRequest { fields: vec![json_pointer(path)] }`. The pointer is built from field names and indices only, never the offending value.
   4. Every request struct in every task derives `Deserialize` with `#[serde(deny_unknown_fields)]`.
9. **JSON output.** `json_ok` writes `Content-Type: application/json; charset=utf-8` (V4.1.1).
10. **Client IP.** Split `X-Forwarded-For` on `,`, trim, take the entry `xff_trusted_hops` positions from the right (1 means the rightmost, the address Google's front end saw). Parse as `IpAddr`; anything else, or a missing header, gives `ClientIp(None)`, which is limited as one shared bucket `"unknown"`. Never use the leftmost entry or `X-Real-IP` (V4.1.3, V15.3.4). `[DEFAULT]` 1 hop; confirm on staging (T-1105) whether Firebase Hosting adds a hop, and set the config, not code.
11. **Rate-limit keys.** `RateLimitKey(format!("{}:{}", policy.name, hex(hmac_sha256(rate_key, subject_bytes))[..32]))`, where `subject_bytes` is the IP string, the UUID string or the lower-cased email. No raw IP, address or ID is ever stored or logged.
12. **Limiter.**
    - `Firestore` policies: `window_start = now` rounded down to a multiple of `window_s` since the Unix epoch; `count = store.rate_limits().hit(&key, window_start, Duration::seconds(window_s))`. Over `limit` gives `RateLimited { retry_after_s: window_start + window_s - now }` (at least 1). `StoreError::Unavailable` fails open for read policies and closed for write policies `[DEFAULT]`: sign-in, invite and deletion limits are security controls.
    - `Memory` fixed window: the same arithmetic in a `Mutex<HashMap<String, (window_start, count)>>`; drop entries whose window ended when the map passes 10,000 keys.
    - `Memory` with `burst` (swipes): token bucket, capacity `burst`, refill `limit / window_s` tokens per second from `Clock`, so a full minute allows 60 but never more than 10 at once.
    - On a hit over the limit: `obs::security_event(SecurityEvent { action: "rate_limit_hit", outcome: "refused", user: <pseudo id when a user is known>, request_id: Some(..), .. })` and mark the request log `rate_limit_hit: true`.
13. **RateLimit headers** on success (IETF `draft-ietf-httpapi-ratelimit-headers`, structured-field form): `RateLimit-Policy: "<name>";q=<limit>;w=<window_s>` and `RateLimit: "<name>";r=<remaining>;t=<reset_s>`.
14. **Default read and write limits** (`OTHER_READS`, `OTHER_WRITES`) need the session, so T-501's session layer applies them; this task exposes `pub fn default_policy(method: &Method) -> Option<&'static Policy>` (GET gives `OTHER_READS`; POST, PUT, PATCH, DELETE give `OTHER_WRITES`).
15. **No extras.** No `/docs`, `/metrics`, `/health` with details, or debug route in a release build (V13.4.5, V15.2.3). A plain `GET /api/v1/healthz` returning `204` is allowed `[DEFAULT]` for Cloud Run probes.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| V1.5.2 | Request JSON uses typed structs with `deny_unknown_fields` |
| V2.4.1 | Rate limits per S7 section 6 return 429 with `Retry-After` |
| V3.4.1 | Every api response carries HSTS with one year and `includeSubDomains` |
| V3.4.2 | No CORS headers are ever sent |
| V3.4.4 | Every response has `X-Content-Type-Options: nosniff` |
| V3.4.5 | Every response has `Referrer-Policy: no-referrer` |
| V3.4.6 | Every response has CSP `frame-ancestors 'none'` |
| V3.5.2 | Unsafe requests must be `application/json`, so a simple cross-site form cannot call them |
| V4.1.1 | Every body response has a `Content-Type` with charset |
| V4.1.3 | Client IP comes only from the trusted `X-Forwarded-For` position |
| V13.4.4 | `TRACE` is not supported |
| V13.4.5 | No documentation or metrics routes are exposed |
| V14.2.2 | `Cache-Control: no-store` on every response |
| V14.3.2 | Anti-caching headers on every response |
| V15.3.3 | Unknown fields are refused (mass assignment) |
| V15.3.4 | Rate limiting uses the trusted client IP |
| V16.5.1 | Error bodies are generic Problem Details with no internal detail |
| RL-1 | Rate-limit counters expire with their window |

## Tests that must pass

All service integration tests build the router with T-202 fakes and a test route set (`#[cfg(test)]` routes `GET /api/v1/__t/ok`, `POST /api/v1/__t/echo` taking `ApiJson<Echo>`, `GET /api/v1/__t/panic`, `GET /api/v1/__t/mail_error/{kind}`).

- `asvs_v1_5_2_unknown_field_is_400_with_pointer_not_value` (service integration)
- `asvs_v15_3_3_extra_field_refused` (service integration)
- `asvs_v2_4_1_sign_in_ip_limit_returns_429_with_retry_after` (service integration, virtual clock)
- `asvs_v2_4_1_swipe_bucket_allows_burst_10_then_refills` (unit, virtual clock)
- `asvs_v2_4_1_ratelimit_headers_on_success` (service integration)
- `asvs_v2_4_1_limit_hit_writes_security_event` (service integration, `obs::capture`)
- `asvs_v3_4_1_hsts_on_every_response` (service integration: ok, error, fallback, panic)
- `asvs_v3_4_2_no_cors_headers_even_with_origin` (service integration)
- `asvs_v3_4_4_nosniff_on_every_response` (service integration)
- `asvs_v3_4_5_referrer_policy_on_every_response` (service integration)
- `asvs_v3_4_6_frame_ancestors_none_on_every_response` (service integration)
- `asvs_v3_5_2_form_content_type_refused_415` (service integration: `application/x-www-form-urlencoded` and `text/plain`)
- `asvs_v4_1_1_content_type_with_charset` (service integration: JSON and problem responses)
- `asvs_v4_1_3_leftmost_forwarded_for_ignored` (unit on `client_ip`)
- `asvs_v15_3_4_spoofed_forwarded_for_shares_real_client_bucket` (service integration)
- `asvs_v13_4_4_trace_not_supported` (service integration: `TRACE` gives 404 problem)
- `asvs_v13_4_5_no_docs_or_metrics_routes` (service integration: `/api/v1/docs`, `/metrics`, `/openapi.json` give 404)
- `asvs_v14_2_2_no_store_on_every_response` (service integration)
- `asvs_v14_3_2_pragma_no_cache_on_every_response` (service integration)
- `asvs_v16_5_1_panic_gives_generic_problem` (service integration: body has no panic text)
- `asvs_v16_5_1_problem_body_shape_snapshot` (unit: one snapshot per `ApiError` variant, `type`, `title`, `status`, `code`)
- `rl_1_firestore_counter_expires_with_window` (service integration: fake store record has `expires_at` = window end; after the window a new count starts)
- `api_payload_too_large_413` (service integration)
- `api_request_id_on_every_response_and_in_problem` (service integration)
- `api_mail_error_mapping_table` (unit on `From<MailError>`)

## Edge cases and traps

- Headers must be on error responses too; a layer that only touches `Ok` responses fails half the tests.
- Do not echo the invalid value or serde's message in `fields`; serde messages can contain the input.
- `deny_unknown_fields` does not work with `#[serde(flatten)]`; do not flatten request structs.
- Do not trust `X-Forwarded-For`'s leftmost value; it is client controlled.
- Do not use `SystemTime::now()` in the limiter; use `ports.clock`.
- Do not store or log raw IPs or emails in rate-limit keys; hash with the key first.
- Do not add `tower_http::cors`.
- Do not return `405` with an `Allow` header; it leaks the route table.
- Memory limits are per instance by design (S7 6); do not "fix" them with Firestore.
- The problem `type` URL is a fixed identifier; never build it from request data.

## Out of scope

- Sessions, CSRF and the `Origin` check: T-501. Auth routes: T-502b, T-504, T-506.
- Which policy a business route uses: each route's task calls `RateLimiter::check`.
- The OpenAPI-generated per-route test matrix: built incrementally by each route task.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cargo run -p api` starts locally against fakes when built with the `testkit` feature, and serves `GET /api/v1/healthz`.
