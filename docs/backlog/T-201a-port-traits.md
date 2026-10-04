# T-201a: Port traits (everything except ServerStore)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 450 lines of code plus tests | T-101 |

Split from T-201 (index row "Port traits"). T-201a holds every port except `ServerStore`; T-201b holds `ServerStore`, its records and repositories.

**Read only these spec sections:** `docs/backlog/CONVENTIONS.md` ("Rust conventions", "Core domain types", "Ports"); S10 3.1 (`docs/specs/S10-test-strategy.md`, the crate table and the rules under it); S4 5.1 and 5.5 (`docs/specs/S4-architecture.md`, classifier trait and model input); S6 section 5 (`docs/specs/S6-security.md`, the "Sealed tokens" and "Access tokens" paragraphs only). Nothing else is needed.

## Goal

The `ports` crate exists with every I/O boundary as an object-safe trait, plus the small value and error types each trait needs. Services, fakes (T-202b) and real adapters (M3, M4) all compile against these names. The `obs::Sensitive<T>` wrapper also exists, because port types carry tokens.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create or keep | `backend/crates/obs/src/sensitive.rs` | `Sensitive<T>`; if the file already exists with the same API, leave it |
| Change | `backend/crates/obs/src/lib.rs` | `pub mod sensitive; pub use sensitive::Sensitive;` |
| Change | `backend/crates/ports/Cargo.toml` | deps: `domain`, `obs`, `async-trait`, `time`, `uuid`, `url`, `thiserror` |
| Change | `backend/crates/ports/src/lib.rs` | module list and re-exports below |
| Create | `backend/crates/ports/src/clock.rs` | `Clock` |
| Create | `backend/crates/ports/src/rng.rs` | `Rng` |
| Create | `backend/crates/ports/src/mail.rs` | `MailProvider`, `MailboxCtx`, `MailError`, paging types, capabilities |
| Create | `backend/crates/ports/src/app_folder.rs` | `AppFolderStore`, `ETag`, `AppFolderError` |
| Create | `backend/crates/ports/src/keys.rs` | `KeyService`, `SystemKeyService`, `WrappedKey`, `Aad`, `SystemAad`, `KeyError` |
| Create | `backend/crates/ports/src/scheduler.rs` | `JobScheduler`, `TaskName`, `CancelOutcome`, `SchedError` |
| Create | `backend/crates/ports/src/egress.rs` | `HttpEgress` and its request, response, outcome and error types |
| Create | `backend/crates/ports/src/identity.rs` | `IdentityProvider` and its types |
| Create | `backend/crates/ports/src/classifier.rs` | `Classifier`, `ClassifierId`, `ClassifierInput`, `ClassifierError` |
| Create | `backend/crates/ports/src/secrets.rs` | `Secrets`, `SecretName`, `SecretError` |
| Create | `backend/crates/ports/tests/object_safety.rs` | compile-time checks |

## Types and signatures

```rust
// ---------- obs/src/sensitive.rs ----------
/// Wraps a secret or personal value. Prints "[redacted]"; never serialises.
pub struct Sensitive<T>(T);
impl<T> Sensitive<T> {
    pub fn new(value: T) -> Self;
    pub fn expose(&self) -> &T;          // the only way to read it; grep-able
    pub fn into_inner(self) -> T;
}
impl<T> std::fmt::Debug for Sensitive<T> { /* writes "[redacted]" */ }
impl<T> std::fmt::Display for Sensitive<T> { /* writes "[redacted]" */ }
impl<T: Clone> Clone for Sensitive<T>;
impl<T> From<T> for Sensitive<T>;
// Deliberately NOT implemented: Serialize, Deserialize, PartialEq on the raw value, Deref.

// ---------- ports/src/clock.rs ----------
pub trait Clock: Send + Sync { fn now(&self) -> time::OffsetDateTime; }

// ---------- ports/src/rng.rs ----------
pub trait Rng: Send + Sync {
    fn bytes32(&self) -> [u8; 32];     // CSPRNG in production
    fn uuid_v4(&self) -> uuid::Uuid;
}

// ---------- ports/src/mail.rs ----------
use domain::{LabelSet, MailboxId, MessageId, MessageMeta, MailtoTarget, Provider};
use obs::Sensitive;

pub struct MailboxCtx { pub mailbox: MailboxId, pub access_token: Sensitive<String> }
#[derive(Clone, Debug, PartialEq, Eq)] pub struct PageToken(pub String);
/// NewestFirst pages the inbox from the top; NewerThan and OlderThan bound the page by internal date
/// (Feed new mail first, then backlog, S2 FD-03). All three return newest first within the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOrder { NewestFirst, NewerThan(time::OffsetDateTime), OlderThan(time::OffsetDateTime) }
#[derive(Clone, Debug, PartialEq)]
pub struct MessagePage { pub items: Vec<MessageMeta>, pub next: Option<PageToken> }

/// Flags the shared contract suite (T-203) reads instead of assuming Gmail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub labels_are_sets: bool,   // Gmail true; Graph (v2) false (one folder plus categories)
    pub spam_is_label: bool,     // Gmail true
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MailError {
    #[error("unauthorized")] Unauthorized,
    #[error("forbidden")] Forbidden,
    #[error("not found")] NotFound,
    #[error("rate limited")] RateLimited { retry_after_s: u64 },
    #[error("transient")] Transient,
    #[error("invalid: {0}")] Invalid(String), // static reason text only, never provider error text
}

#[async_trait::async_trait]
pub trait MailProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn list_inbox(&self, mb: &MailboxCtx, page: Option<PageToken>, order: ListOrder) -> Result<MessagePage, MailError>;
    async fn get_meta(&self, mb: &MailboxCtx, id: &MessageId) -> Result<MessageMeta, MailError>;
    async fn get_preview(&self, mb: &MailboxCtx, id: &MessageId) -> Result<String, MailError>;
    async fn set_labels(&self, mb: &MailboxCtx, id: &MessageId, add: &LabelSet, remove: &LabelSet) -> Result<LabelSet, MailError>;
    async fn trash(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;      // labels before
    async fn report_spam(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>; // labels before
    async fn restore_labels(&self, mb: &MailboxCtx, id: &MessageId, exact: &LabelSet) -> Result<(), MailError>;
    async fn ensure_label(&self, mb: &MailboxCtx, name: &str) -> Result<String, MailError>;
    async fn send_mailto(&self, mb: &MailboxCtx, to: &MailtoTarget) -> Result<(), MailError>;
    async fn inbox_count(&self, mb: &MailboxCtx) -> Result<u64, MailError>;
    // No delete method exists or may be added (INV-5).
}

// ---------- ports/src/app_folder.rs ----------
#[derive(Clone, Debug, PartialEq, Eq)] pub struct ETag(pub String);
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppFolderError { #[error("etag conflict")] Conflict, #[error(transparent)] Mail(#[from] MailError) }

#[async_trait::async_trait]
pub trait AppFolderStore: Send + Sync {
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError>; // ciphertext
    async fn write(&self, mb: &MailboxCtx, bytes: &[u8], if_match: Option<&ETag>) -> Result<ETag, AppFolderError>;
    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError>;
}

// ---------- ports/src/keys.rs ----------
use domain::UserId;

/// KMS ciphertext of a user's data_key. Debug prints only the length.
#[derive(Clone, PartialEq, Eq)] pub struct WrappedKey(pub Vec<u8>);

/// Associated data for one encrypted value. Field names are constants, never user input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aad { pub user: UserId, pub scope: String, pub field: &'static str }

/// Associated data for values encrypted before any user exists (see SystemKeyService).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemAad { pub scope: String, pub field: &'static str }

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("key service unavailable")] Unavailable,     // retryable (KMS 5xx, timeout)
    #[error("open failed")] OpenFailed,                 // wrong key, wrong AAD, tampered: one variant on purpose
    #[error("malformed ciphertext")] Malformed,
    #[error("unsupported scheme version {0}")] UnsupportedVersion(u8),
    #[error("denied")] Denied,                          // KMS permission denied
}

#[async_trait::async_trait]
pub trait KeyService: Send + Sync {
    async fn new_user_key(&self, user: &UserId) -> Result<WrappedKey, KeyError>;
    async fn seal(&self, user: &UserId, wrapped: &WrappedKey, aad: &Aad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError>;
    async fn open(&self, user: &UserId, wrapped: &WrappedKey, aad: &Aad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError>;
}

/// Encrypts the few fields that exist before a user does: invite and invite-request email
/// addresses and pre_auth session fields (S3). See "Edge cases" for why this exists.
#[async_trait::async_trait]
pub trait SystemKeyService: Send + Sync {
    async fn seal(&self, aad: &SystemAad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError>;
    async fn open(&self, aad: &SystemAad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError>;
}

// ---------- ports/src/scheduler.rs ----------
use domain::JobId;
#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct TaskName(pub String);
impl TaskName { pub fn for_job(job: &JobId) -> TaskName; } // "job-" + simple (no hyphens) UUID, lower case
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum CancelOutcome { Cancelled, NotFound, AlreadyRunning }
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchedError { #[error("scheduler unavailable")] Unavailable, #[error("rejected: {0}")] Rejected(&'static str) }

#[async_trait::async_trait]
pub trait JobScheduler: Send + Sync {
    async fn schedule(&self, job: &JobId, due_at: time::OffsetDateTime) -> Result<TaskName, SchedError>; // idempotent per job
    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError>;
}

// ---------- ports/src/egress.rs ----------
use url::Url;
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum HttpMethod { Get, Post, Put, Patch, Delete }
pub struct EgressRequest {
    pub method: HttpMethod,
    pub url: Url,
    pub headers: Vec<(String, Sensitive<String>)>, // values may be bearer tokens; never logged
    pub body: Option<Vec<u8>>,
    pub timeout: std::time::Duration,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressResponse { pub status: u16, pub headers: Vec<(String, String)>, pub body: Vec<u8> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneClickOutcome {
    Accepted { status: u16 },   // 2xx
    Redirected { status: u16 }, // 3xx, not followed
    Rejected { status: u16 },   // 4xx or 5xx
    TimedOut,                   // no full response within the timeout
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusedRange {
    Unspecified, Loopback, Private, Cgnat, LinkLocal, Metadata, UniqueLocal,
    Multicast, Broadcast, Documentation, Benchmarking, Reserved, Translation, // NAT64, 6to4, Teredo, IPv4-compatible
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("scheme not allowed")] SchemeNotAllowed,
    #[error("credentials in url")] CredentialsInUrl,
    #[error("port not allowed")] PortNotAllowed,
    #[error("ip literal host")] IpLiteralHost,
    #[error("host not allowed")] HostNotAllowed,
    #[error("address refused: {0:?}")] AddressRefused(RefusedRange),
    #[error("permanent delete refused")] PermanentDeleteRefused,
    #[error("not permitted for this service")] NotPermitted,
    #[error("dns failure")] DnsFailed,
    #[error("connect failure")] Connect,
    #[error("tls failure")] Tls,
    #[error("timeout")] Timeout,
    #[error("response too large")] ResponseTooLarge,
}

#[async_trait::async_trait]
pub trait HttpEgress: Send + Sync {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError>;
    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError>;
}

// ---------- ports/src/identity.rs ----------
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Prompt { Login, Consent, SelectAccount }
pub struct AuthRequest {
    pub state: String, pub nonce: String,
    pub code_challenge: String,           // base64url(SHA-256(verifier)), method S256 only
    pub redirect_uri: Url,
    pub scopes: Vec<&'static str>,
    pub prompt: Option<Prompt>,
    pub max_age_s: Option<u32>,
    pub login_hint: Option<Sensitive<String>>,
}
pub struct TokenSet {                      // Debug written by hand: field names only
    pub access_token: Sensitive<String>,
    pub refresh_token: Option<Sensitive<String>>,
    pub id_token: Sensitive<String>,       // raw JWT, validated with validate_id_token
    pub expires_in_s: u64,
    pub granted_scopes: Vec<String>,
}
pub struct IdClaims {                      // Debug written by hand
    pub sub: String,
    pub email: Sensitive<String>,
    pub email_verified: bool,
    pub auth_time: Option<time::OffsetDateTime>,
    pub amr: Vec<String>,
    pub issued_at: time::OffsetDateTime,
    pub expires_at: time::OffsetDateTime,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("invalid grant")] InvalidGrant,              // revoked, expired or replayed code or refresh token
    #[error("invalid id token: {0}")] InvalidIdToken(&'static str), // "signature", "iss", "aud", "exp", "nonce", "alg"
    #[error("access denied")] AccessDenied,              // user cancelled at the provider
    #[error("identity provider unavailable")] Unavailable,
}

#[async_trait::async_trait]
pub trait IdentityProvider: Send + Sync {
    fn authorize_url(&self, req: &AuthRequest) -> Url;
    async fn exchange(&self, code: &str, verifier: &Sensitive<String>, redirect_uri: &Url) -> Result<TokenSet, IdError>;
    async fn validate_id_token(&self, raw: &Sensitive<String>, nonce: &str) -> Result<IdClaims, IdError>;
    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError>; // access token
    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError>;
}

// ---------- ports/src/classifier.rs ----------
use domain::Classification;
#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct ClassifierId(pub String); // "header_rules@1", "gemini@flash-lite", "jev@1.13.0"
/// Built in memory by T-903 from the S4 5.5 allowlist. Never stored, never logged.
pub struct ClassifierInput {                // Debug written by hand: "ClassifierInput { .. }"
    pub from_display: String, pub from_domain: String,
    pub list_id: Option<String>,
    pub has_list_unsubscribe: bool, pub has_list_unsubscribe_post: bool,
    pub precedence: Option<String>, pub auto_submitted: Option<String>,
    pub esp_header_names: Vec<String>,
    pub auth_summary: String,
    pub subject: String,
    pub text: String,                       // stripped, redacted, about 500 tokens
    pub input_version: &'static str,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClassifierError {
    #[error("timeout")] Timeout,
    #[error("http {0}")] Http(u16),
    #[error("invalid output")] InvalidOutput,
    #[error("disabled")] Disabled,
    #[error("unavailable")] Unavailable,
}
#[async_trait::async_trait]
pub trait Classifier: Send + Sync {
    fn id(&self) -> ClassifierId;
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}

// ---------- ports/src/secrets.rs ----------
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretName { GoogleOAuthClientSecret, JevApiKey, EmailLookupHmacKey, LogPseudonymHmacKey }
impl SecretName { pub fn secret_id(self) -> &'static str; } // "google-oauth-client-secret", "jev-api-key", "email-lookup-hmac-key", "log-pseudonym-hmac-key"
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretError { #[error("secret unavailable")] Unavailable, #[error("secret denied")] Denied, #[error("secret missing")] Missing }
#[async_trait::async_trait]
pub trait Secrets: Send + Sync {
    async fn get(&self, name: SecretName) -> Result<Sensitive<Vec<u8>>, SecretError>;
}
```

`ports/src/lib.rs` re-exports every public item above at the crate root (`pub use mail::*;` and so on), and declares `pub mod store;` only after T-201b lands (leave a `// T-201b adds store` comment).

## Algorithm

1. Create `obs::Sensitive` exactly as above. `Debug` and `Display` write the literal `[redacted]`. No `Serialize` or `Deserialize`.
2. Write each module with the items above, no logic except `TaskName::for_job` and `SecretName::secret_id`.
3. `TaskName::for_job(job)` returns `format!("job-{}", job.0.simple())`. Cloud Tasks task IDs allow letters, digits, hyphens and underscores; this keeps T-304 and the fake in step.
4. Write `Debug` by hand for `WrappedKey` (`WrappedKey(len=N)`), `MailboxCtx`, `TokenSet`, `IdClaims`, `AuthRequest` and `ClassifierInput`, printing only non-sensitive fields or `..`.
5. Add `tests/object_safety.rs`: for each trait, a function that takes `std::sync::Arc<dyn Trait>` and does nothing, so the build fails if a trait stops being object safe.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| INV-5 | `MailProvider` has no delete method, so core code cannot permanently delete a message |
| INV-7 | `ports` names provider-neutral types only; no Gmail type appears in any signature |

## Tests that must pass

- `inv_5_mail_provider_has_no_delete_method` (unit, `ports`): a test that lists the trait's method names through a hand-maintained array and asserts none contains `delete`; plus a doc comment on the trait. Keep the array in step with the trait.
- `inv_7_ports_has_no_provider_specific_types` (unit, `ports`): reads each `src/*.rs` file with `include_str!` and asserts that no line outside a `//` or `///` comment contains `gmail` or `graph` (case-insensitive).
- `sensitive_debug_and_display_print_redacted` (unit, `obs`).
- `wrapped_key_debug_prints_length_only` (unit, `ports`).
- `task_name_for_job_is_stable_and_hyphen_free` (unit, `ports`).
- `object_safety` (compile test, `ports/tests/object_safety.rs`).

## Edge cases and traps

- The privacy Semgrep rule `privacy-rust-derive-debug-on-sensitive-struct` fails any struct that derives `Debug` and has a field whose name contains `token`, `email`, `secret` or `api_key`, whatever the field's type. Write those `Debug` impls by hand.
- Do not add `Deref` to `Sensitive`; `expose()` keeps every read visible in review.
- `MailError::Invalid` carries a static reason chosen by the adapter, never the provider's error text (provider text can contain addresses).
- `KeyError::OpenFailed` deliberately does not say whether the key, the AAD or the tag was wrong.
- `Aad::field` is `&'static str` so callers cannot build field names from request data.
- `ListOrder` must not be matched with `_`; nor may `Provider` anywhere (CONVENTIONS).
- `IdentityProvider::exchange` takes `redirect_uri` (added to the CONVENTIONS signature) because Google requires the same redirect URI on the token request.
- `SystemKeyService` is new (not in CONVENTIONS). It exists because invites, invite requests and `pre_auth` sessions are encrypted before any user `data_key` exists (S3, S5). T-302 implements it; the spec gap is reported to James.

## Out of scope

- `ServerStore` and its records (T-201b).
- Any implementation or fake (T-202a, T-202b, M3, M4).
- The `Ports` struct (T-202b).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cargo doc -p ports` builds with no warnings; every public item has a one-line doc comment.
