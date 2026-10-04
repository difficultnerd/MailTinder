# T-201b: ServerStore repository traits and records

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 550 lines of code plus tests | T-106, T-107, T-201a |

Split from T-201 (index row "Port traits"). Needs the job, invite, mailbox and Needs Attention enums from T-106 and T-107.

**Read only these spec sections:** `docs/backlog/CONVENTIONS.md` ("Ports", the `ServerStore` comment); S3 "Server database (Firestore collections)" table and "Invariants" (`docs/specs/S3-domain-model.md`); S5 "Firestore (server)" table and "Deletion tests" (`docs/specs/S5-data-inventory.md`); S4 5.6 first paragraph (`docs/specs/S4-architecture.md`); S7 5.8 first bullet (reason codes) and 6 (the "Store" column) (`docs/specs/S7-api-contract.md`). Nothing else is needed.

## Goal

The `ports::store` module defines one typed repository trait per Firestore collection allowed by S5 (`users`, `mailboxes`, `invites`, `invite_requests`, `jobs`, `needs_attention`, `sessions`, `classifier_eval`, `bakeoff_snapshots`, `config`, `rate_limits`), the record type each one stores, optimistic concurrency, paging and the `ServerStore` trait that hands them out. The in-memory fake (T-202a) and the Firestore adapter (T-301) implement exactly this; services never see Firestore.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/ports/src/store/mod.rs` | `ServerStore`, `Repo`, `Keyed`, concurrency and paging types, `StoreError`, `COLLECTIONS` |
| Create | `backend/crates/ports/src/store/ids.rs` | ID and hash newtypes, `Ciphertext` |
| Create | `backend/crates/ports/src/store/records.rs` | one record struct per collection plus nested types |
| Create | `backend/crates/ports/src/store/repos.rs` | one repository trait per collection |
| Create | `backend/crates/ports/src/store/aad_fields.rs` | the `Aad::field` and `SystemAad::field` constants for every encrypted field |
| Change | `backend/crates/ports/src/lib.rs` | `pub mod store;` |
| Change | `backend/crates/ports/Cargo.toml` | add `serde` (derive), `base64` |
| Change (additive only) | `backend/crates/domain/src/*.rs` | add `Serialize, Deserialize` with `#[serde(rename_all = "snake_case")]` to any enum used below that lacks them |
| Create | `backend/crates/ports/tests/store_schema.rs` | schema and invariant tests |

## Types and signatures

```rust
// ---------- store/ids.rs ----------
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id { ($n:ident) => {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(transparent)] pub struct $n(pub Uuid);
}}
uuid_id!(InviteId); uuid_id!(InviteRequestId); uuid_id!(NeedsAttentionId);
uuid_id!(SnapshotId); uuid_id!(EvalId); uuid_id!(SessionRecordId);
// If T-107 already defines InviteId or NeedsAttentionId in `domain`, re-export those instead of redefining.

/// Bytes produced by KeyService::seal or SystemKeyService::seal. Serialises as unpadded base64url.
/// Debug prints "Ciphertext(len=N)".
#[derive(Clone, PartialEq, Eq)] pub struct Ciphertext(pub Vec<u8>);

/// 32-byte digests. Serialise as lower-case hex (64 chars). Debug prints the first 8 hex chars only.
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct Sha256Hash(pub [u8; 32]);      // invite token hash
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct EmailLookupHash(pub [u8; 32]); // HMAC-SHA-256, email lookup key
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct ListKeyHash(pub [u8; 32]);     // HMAC of the list key (T-605 decides the key)
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct SessionHash(pub [u8; 32]);     // SHA-256 of the raw session ID
impl SessionHash { pub fn to_hex(&self) -> String; }                                  // the sessions document ID

/// HMAC-SHA-256 of the user ID under the log pseudonymisation key, first 16 bytes, lower-case hex (32 chars).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(transparent)]
pub struct UserPseudoId(pub String);

/// Opaque per-limit key built by the caller, already hashed (no raw IP or address). Max 200 chars, [a-z0-9:_-].
#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct RateLimitKey(pub String);

// ---------- store/mod.rs ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] pub struct Version(pub String); // opaque
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition { None, MustNotExist, MustExist, Matches(Version) }
#[derive(Clone, Debug, PartialEq)] pub struct Versioned<T> { pub record: T, pub version: Version }
#[derive(Clone, Debug, PartialEq, Eq)] pub struct StoreCursor(pub String); // internal; the api seals it before it leaves
#[derive(Clone, Debug, PartialEq, Eq)] pub struct PageRequest { pub limit: u32, pub after: Option<StoreCursor> }
#[derive(Clone, Debug, PartialEq)] pub struct Page<T> { pub items: Vec<T>, pub next: Option<StoreCursor> }
pub const MAX_PAGE: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("already exists")] AlreadyExists,          // MustNotExist failed
    #[error("precondition failed")] PreconditionFailed, // Matches(v) failed, or MustExist on a missing document
    #[error("store unavailable")] Unavailable,          // retryable
    #[error("corrupt record: {0}")] Corrupt(&'static str),
    #[error("invalid: {0}")] Invalid(&'static str),
}

pub const COLLECTIONS: [&str; 11] = ["users", "mailboxes", "invites", "invite_requests", "jobs",
    "needs_attention", "sessions", "classifier_eval", "bakeoff_snapshots", "config", "rate_limits"];

/// A record knows its own document key.
pub trait Keyed { type Key: Send + Sync; fn key(&self) -> Self::Key; }

#[async_trait::async_trait]
pub trait Repo<K: Send + Sync, R: Keyed<Key = K> + Send + Sync>: Send + Sync {
    async fn get(&self, key: &K) -> Result<Option<Versioned<R>>, StoreError>;
    /// Create or replace. Returns the new version.
    async fn put(&self, record: &R, pre: Precondition) -> Result<Version, StoreError>;
    /// Missing document with Precondition::None is Ok (idempotent).
    async fn delete(&self, key: &K, pre: Precondition) -> Result<(), StoreError>;
}

pub trait ServerStore: Send + Sync {
    fn users(&self) -> &dyn UserRepo;
    fn mailboxes(&self) -> &dyn MailboxRepo;
    fn invites(&self) -> &dyn InviteRepo;
    fn invite_requests(&self) -> &dyn InviteRequestRepo;
    fn jobs(&self) -> &dyn JobRepo;
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo;
    fn sessions(&self) -> &dyn SessionRepo;
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo;
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo;
    fn config(&self) -> &dyn ConfigRepo;
    fn rate_limits(&self) -> &dyn RateLimitRepo;
}

// ---------- store/records.rs ----------
// Every record: #[derive(Clone, PartialEq, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
// Times: #[serde(with = "time::serde::rfc3339")] (and ::option). Field names below are the stored names.
// Every field whose name ends in `_at` is a timestamp; T-301 relies on that rule.
use domain::{JobId, JobMethod, JobStatus, InviteStatus, MailboxId, MailboxStatus, MessageClass,
             NeedsAttentionReason, Provider, ProviderSubjectId, UserId};
use crate::keys::WrappedKey; // give WrappedKey serde as unpadded base64url in this task

pub struct UserRecord {                              // users/{user_id}
    pub user_id: UserId,
    pub created_at: OffsetDateTime,
    pub is_admin: bool,
    pub wrapped_data_key: WrappedKey,
    pub experiments_consent_version: Option<String>,
    pub experiments_opted_in_at: Option<OffsetDateTime>,
}

pub struct MailboxRecord {                           // mailboxes/{mailbox_id}; Debug by hand
    pub mailbox_id: MailboxId,
    pub user_id: UserId,                             // INV-3: exactly one owner, never optional
    pub provider: Provider,
    pub provider_subject_id: ProviderSubjectId,      // Google `sub` (T-101 type, Debug redacted)
    pub email_address: Ciphertext,                   // KeyService, aad_fields::MAILBOX_EMAIL
    pub status: MailboxStatus,                       // stored: connected, needs_sign_in, consent_blocked (v2)
    pub linked_at: OffsetDateTime,
    pub is_primary: bool,
    pub refresh_token: Option<Ciphertext>,           // KeyService, aad_fields::MAILBOX_REFRESH_TOKEN
}

pub struct InviteRecord {                            // invites/{invite_id}; Debug by hand
    pub invite_id: InviteId,
    pub email_address: Ciphertext,                   // SystemKeyService, aad_fields::INVITE_EMAIL
    pub email_lookup: EmailLookupHash,
    pub token_hash: Sha256Hash,
    pub status: InviteStatus,                        // pending, used, revoked, expired
    pub created_at: OffsetDateTime,
    pub last_sent_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,                  // validity: last_sent_at + INVITE_TTL (7 days)
    pub purge_at: OffsetDateTime,                    // TTL field: see Algorithm step 4
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum InviteRequestStatus { Pending }             // approve converts to an invite and deletes; decline deletes
pub struct InviteRequestRecord {                     // invite_requests/{request_id}; Debug by hand
    pub request_id: InviteRequestId,
    pub email_address: Ciphertext,                   // SystemKeyService, aad_fields::INVITE_REQUEST_EMAIL
    pub email_lookup: EmailLookupHash,
    pub created_at: OffsetDateTime,
    pub status: InviteRequestStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum JobOutcomeCode {
    OneClickAccepted, MailtoSent, Redirected, AddressRefused, HttpRejected, TimedOut,
    RetriesExhausted, TokenInvalid, Expired, Cancelled,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
pub struct JobOutcome { pub code: JobOutcomeCode, #[serde(with = "time::serde::rfc3339")] pub at: OffsetDateTime }

pub struct JobRecord {                               // jobs/{job_id}; derive Debug is fine (no sensitive names)
    pub job_id: JobId,
    pub user_id: UserId,
    pub mailbox_id: MailboxId,
    pub list_key_hash: ListKeyHash,
    pub method: JobMethod,                           // one_click, mailto
    pub target: Option<Ciphertext>,                  // KeyService, aad_fields::JOB_TARGET; None once terminal
    pub due_at: OffsetDateTime,
    pub status: JobStatus,
    pub attempts: u32,
    pub outcome: Option<JobOutcome>,
    pub expires_at: OffsetDateTime,                  // TTL field; see Algorithm step 4
}

pub struct NeedsAttentionRecord {                    // needs_attention/{item_id}
    pub item_id: NeedsAttentionId,
    pub user_id: UserId,
    pub mailbox_id: MailboxId,
    pub sender_display: Ciphertext,                  // KeyService, aad_fields::NA_SENDER_DISPLAY
    pub link: Option<Ciphertext>,                    // KeyService, aad_fields::NA_LINK; https only, checked by the writer
    pub reason_code: NeedsAttentionReason,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,                  // created_at + 30 days; TTL field
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum SessionState { PreAuth, PendingInviteRequest, Authenticated }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum AuthIntent { SignIn, Join, Link, Reconnect, StepUp }

pub struct PreAuthFields {                           // Debug by hand. Encrypted with SystemKeyService,
    pub intent: AuthIntent,                          // scope = session_record_id
    pub oauth_state: Ciphertext,                     // aad_fields::PRE_AUTH_STATE
    pub nonce: Ciphertext,                           // aad_fields::PRE_AUTH_NONCE
    pub pkce_verifier: Ciphertext,                   // aad_fields::PRE_AUTH_PKCE_VERIFIER
    pub invite_token_hash: Option<Sha256Hash>,
    pub pending_email: Option<Ciphertext>,           // aad_fields::PRE_AUTH_PENDING_EMAIL
    pub started_at: OffsetDateTime,                  // pre_auth TTL is 10 minutes from here
}
pub struct SessionRecord {                           // sessions/{session_hash hex}; Debug by hand
    pub session_hash: SessionHash,
    pub session_record_id: SessionRecordId,          // stable across rotation; sealed tokens bind to it
    pub state: SessionState,
    pub user_id: Option<UserId>,                     // Some only when authenticated
    pub csrf_token: String,                          // 256-bit random, base64url; compared in constant time by T-501
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub recent_auth_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,                  // TTL field: min(last_seen_at + 15 min, created_at + 12 h)
    pub pre_auth: Option<PreAuthFields>,             // also used while a link, reconnect or step-up round trip runs
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum SwipeDirection { Left, Right, Up, Down }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgeBucket { #[serde(rename = "<7d")] Under7d, #[serde(rename = "7d-90d")] D7To90d,
    #[serde(rename = "90d-1y")] D90To1y, #[serde(rename = "1y-5y")] Y1To5y, #[serde(rename = ">5y")] Over5y }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextTokensBucket { #[serde(rename = "<100")] Under100, #[serde(rename = "100-300")] T100To300, #[serde(rename = ">300")] Over300 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ModelErrorCode { Timeout, Http, InvalidOutput, Disabled, Unavailable }

pub struct HeaderRulesResult { pub class: MessageClass, pub score: u8 }
pub struct ModelPrediction {
    pub class: Option<MessageClass>, pub score: Option<u8>, pub confidence: Option<f32>,
    pub probabilities: Option<[f32; 5]>, pub model_version: String, pub latency_ms: u32,
    pub input_tokens: Option<u32>, pub error_code: Option<ModelErrorCode>,
}
pub struct EvalOutcome { pub direction: SwipeDirection, pub undone: bool, pub time_to_swipe_ms: u32 }
pub struct EvalHeaderFacts {                         // booleans only
    pub has_list_unsubscribe: bool, pub dkim_covers_list_unsubscribe: bool,
    pub has_list_unsubscribe_post: bool, pub has_list_id: bool, pub has_feedback_id: bool,
    pub precedence_bulk: bool, pub auto_submitted: bool, pub from_authenticated: bool,
}
pub struct ClassifierEvalRecord {                    // classifier_eval/{eval_id}
    pub eval_id: EvalId,                             // UUID v5(user ID, Idempotency-Key); never from the message ID
    pub user_pseudo_id: UserPseudoId,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,                  // created_at + 180 days [TUNABLE]; TTL field
    pub header_rules: HeaderRulesResult,
    pub gemini: Option<ModelPrediction>,
    pub jev: Option<ModelPrediction>,
    pub outcome: EvalOutcome,
    pub header_facts: EvalHeaderFacts,
    pub provider: Provider,
    pub age_bucket: AgeBucket,
    pub text_tokens_bucket: TextTokensBucket,
    pub lang_is_english: bool,
    pub input_version: String, pub question_version: String, pub price_version: String,
}

pub struct BakeoffSnapshotRecord {                   // bakeoff_snapshots/{snapshot_id}
    pub snapshot_id: SnapshotId,
    pub name: String,                                // 1 to 100 chars, plain text
    pub created_at: OffsetDateTime,
    pub labelled_swipes: u64,
    pub query_json: String,                          // the API-ADM-11 query, serialised
    pub report_json: String,                         // the suppressed API-ADM-10 body, serialised (T-908)
}

pub struct ClassifiersConfig {                       // config/classifiers (the only config document)
    pub gemini_enabled: bool,
    pub jev_enabled: bool,
    pub updated_at: OffsetDateTime,
}

pub struct RateLimitRecord {                         // rate_limits/{key}:{window_start unix seconds}
    pub key: String,
    pub window_start: OffsetDateTime,                // stored name: window_start_at (ends in _at)
    pub count: u32,
    pub expires_at: OffsetDateTime,                  // window end; TTL field
}

// Keyed impls: UserRecord -> UserId, MailboxRecord -> MailboxId, InviteRecord -> InviteId,
// InviteRequestRecord -> InviteRequestId, JobRecord -> JobId, NeedsAttentionRecord -> NeedsAttentionId,
// SessionRecord -> SessionHash, ClassifierEvalRecord -> EvalId, BakeoffSnapshotRecord -> SnapshotId.

// ---------- store/repos.rs ----------
#[async_trait::async_trait] pub trait UserRepo: Repo<UserId, UserRecord> {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<UserRecord>>, StoreError>; // created_at asc, then user_id
}
#[async_trait::async_trait] pub trait MailboxRepo: Repo<MailboxId, MailboxRecord> {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<MailboxRecord>>, StoreError>; // linked_at asc
    async fn by_subject(&self, provider: Provider, subject: &ProviderSubjectId) -> Result<Option<Versioned<MailboxRecord>>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}
#[async_trait::async_trait] pub trait InviteRepo: Repo<InviteId, InviteRecord> {
    async fn by_token_hash(&self, hash: &Sha256Hash) -> Result<Option<Versioned<InviteRecord>>, StoreError>;
    async fn by_email_lookup(&self, hash: &EmailLookupHash) -> Result<Vec<Versioned<InviteRecord>>, StoreError>;
    async fn list(&self, status: Option<InviteStatus>, page: PageRequest) -> Result<Page<Versioned<InviteRecord>>, StoreError>; // created_at desc
    async fn purge_due(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<InviteId>, StoreError>; // purge_at <= now
}
#[async_trait::async_trait] pub trait InviteRequestRepo: Repo<InviteRequestId, InviteRequestRecord> {
    async fn by_email_lookup(&self, hash: &EmailLookupHash) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError>;
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<InviteRequestRecord>>, StoreError>; // created_at asc
}
#[async_trait::async_trait] pub trait JobRepo: Repo<JobId, JobRecord> {
    async fn by_mailbox(&self, mailbox: &MailboxId) -> Result<Vec<Versioned<JobRecord>>, StoreError>;
    async fn by_user_with_outcome(&self, user: &UserId, limit: u32) -> Result<Vec<Versioned<JobRecord>>, StoreError>; // outcome set, due_at asc
    async fn queued_for_list(&self, user: &UserId, list: &ListKeyHash) -> Result<Vec<Versioned<JobRecord>>, StoreError>; // status queued
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<Versioned<JobRecord>>, StoreError>; // expires_at <= now, asc
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}
#[async_trait::async_trait] pub trait NeedsAttentionRepo: Repo<NeedsAttentionId, NeedsAttentionRecord> {
    async fn by_user(&self, user: &UserId, page: PageRequest) -> Result<Page<Versioned<NeedsAttentionRecord>>, StoreError>; // created_at desc
    async fn count_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<NeedsAttentionId>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}
#[async_trait::async_trait] pub trait SessionRepo: Repo<SessionHash, SessionRecord> {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<SessionRecord>>, StoreError>;
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<SessionHash>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}
#[async_trait::async_trait] pub trait ClassifierEvalRepo: Repo<EvalId, ClassifierEvalRecord> {
    async fn range(&self, from: OffsetDateTime, to: OffsetDateTime, page: PageRequest) -> Result<Page<ClassifierEvalRecord>, StoreError>; // created_at in [from, to), asc
    /// Opt-out and account deletion. Takes every pseudo ID the user has had (one per log key version).
    async fn delete_for_users(&self, ids: &[UserPseudoId]) -> Result<u64, StoreError>;
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<EvalId>, StoreError>;
}
#[async_trait::async_trait] pub trait BakeoffSnapshotRepo: Repo<SnapshotId, BakeoffSnapshotRecord> {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<BakeoffSnapshotRecord>>, StoreError>; // created_at desc
    async fn count(&self) -> Result<u64, StoreError>;
}
#[async_trait::async_trait] pub trait ConfigRepo: Send + Sync {
    async fn get_classifiers(&self) -> Result<Option<Versioned<ClassifiersConfig>>, StoreError>;
    async fn put_classifiers(&self, cfg: &ClassifiersConfig, pre: Precondition) -> Result<Version, StoreError>;
}
#[async_trait::async_trait] pub trait RateLimitRepo: Send + Sync {
    /// Atomically add one to the counter for (key, window_start) and return the new count.
    async fn hit(&self, key: &RateLimitKey, window_start: OffsetDateTime, window: time::Duration) -> Result<u32, StoreError>;
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<u64, StoreError>; // deletes, returns count
}

// ---------- store/aad_fields.rs ----------
pub const MAILBOX_EMAIL: &str = "mailbox.email_address";       // scope = mailbox_id
pub const MAILBOX_REFRESH_TOKEN: &str = "mailbox.refresh_token"; // scope = mailbox_id (S6 5: user + mailbox + field)
pub const JOB_TARGET: &str = "job.target";                      // scope = job_id
pub const NA_SENDER_DISPLAY: &str = "needs_attention.sender_display"; // scope = item_id
pub const NA_LINK: &str = "needs_attention.link";               // scope = item_id
pub const INVITE_EMAIL: &str = "invite.email_address";          // system key, scope = invite_id
pub const INVITE_REQUEST_EMAIL: &str = "invite_request.email_address"; // system key, scope = request_id
pub const PRE_AUTH_STATE: &str = "session.pre_auth.oauth_state";      // system key, scope = session_record_id
pub const PRE_AUTH_NONCE: &str = "session.pre_auth.nonce";
pub const PRE_AUTH_PKCE_VERIFIER: &str = "session.pre_auth.pkce_verifier";
pub const PRE_AUTH_PENDING_EMAIL: &str = "session.pre_auth.pending_email";
pub const APP_FOLDER_FILE: &str = "app_folder.file";            // scope = user_id (T-405)
```

## Algorithm

1. Write the newtypes with hand-written serde: `Ciphertext` and `WrappedKey` as unpadded base64url strings; the four hash types as 64-char lower-case hex. Reject wrong lengths on deserialise with a serde error.
2. Write each record with `#[serde(deny_unknown_fields)]`. Do not derive `Debug` on `MailboxRecord`, `InviteRecord`, `InviteRequestRecord`, `SessionRecord` or `PreAuthFields`; write `Debug` that prints the key field only (for example `SessionRecord { session_record_id: .. }`).
3. Implement `Keyed` for each record.
4. TTL fields `[DEFAULT]` (Firestore TTL policies, one field per collection, set in Terraform by T-1102):
   - `jobs.expires_at`: `due_at + JOB_TTL` (1 hour) while non-terminal; the writer that moves a job to a terminal state sets it to `now + 30 days` `[TUNABLE]` (S3 terminal retention). One field keeps JOB-1 true for both phases.
   - `invites.purge_at`: `expires_at + 30 days` while pending; `now + 30 days` when used, revoked or expired (S5 "then 30 days").
   - `sessions.expires_at`, `needs_attention.expires_at`, `classifier_eval.expires_at`, `rate_limits.expires_at` as commented above.
   - `users`, `mailboxes`, `invite_requests`, `bakeoff_snapshots`, `config` have no TTL.
5. Mailbox IDs `[DEFAULT]`: add `pub fn mailbox_id_for(provider: Provider, subject: &ProviderSubjectId) -> MailboxId` returning `Uuid::new_v5(&MAILBOX_NAMESPACE, format!("{}:{}", provider_str, subject.as_str()))` with a fixed `MAILBOX_NAMESPACE` UUID constant. Creating a mailbox with `Precondition::MustNotExist` then enforces "unique on (provider, provider_subject_id)" (S3) without a second collection, which S5 forbids. `match provider` without `_`.
6. `RateLimitRecord` stores `window_start` under the name `window_start_at` (`#[serde(rename = "window_start_at")]`) so the `_at` rule holds.
7. Write `tests/store_schema.rs` (below). The S5 field allowlist lives in the test as one `const` table per collection, with a comment citing S5; adding a field means editing S5 first.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| INV-1 | No record type has a body, subject or snippet field |
| INV-2 | `JobRecord` and `NeedsAttentionRecord` cannot be built without `expires_at` |
| INV-3 | `MailboxRecord.user_id` is a single, non-optional owner |
| CFG-1 | `config/classifiers` holds only `gemini_enabled`, `jev_enabled`, `updated_at` (schema part; the switch behaviour is T-305) |
| JOB-1 | No job record can hold an access token (schema part; sweeping is T-706) |
| EXP-1 | `classifier_eval` fields match the S5 allowed schema (schema part; content scan is T-906) |

## Tests that must pass

- `inv_1_no_record_has_body_subject_or_snippet_field` (unit): serialise a sample of every record to JSON and assert no key, at any depth, contains `body`, `subject`, `snippet` or `preview`.
- `inv_2_job_and_needs_attention_records_require_expires_at` (unit): deserialising JSON without `expires_at` fails for both.
- `inv_3_mailbox_record_has_exactly_one_user` (unit): JSON with `user_id: null` or missing fails.
- `cfg_1_config_document_holds_only_allowed_fields` (unit): key set equals the three names; an extra key fails to deserialise.
- `job_1_job_record_has_no_access_token_field` (unit): key set equals the S5 list; no key contains `token`.
- `exp_1_eval_record_fields_match_allowed_schema` (unit): key set at every depth equals the S5 and S4 5.6 list.
- `s5_every_collection_has_a_schema_entry` (unit): `COLLECTIONS` equals the keys of the test's field table, and every record's key set equals its entry.
- `mailbox_id_for_is_deterministic_and_provider_scoped` (unit).
- `ciphertext_and_hashes_round_trip_and_reject_bad_length` (unit).
- `sensitive_records_debug_prints_no_ciphertext` (unit): `format!("{:?}")` of each hand-written Debug contains no base64 of the sample bytes.

## Edge cases and traps

- Times must use `time::serde::rfc3339`, never the default `time` serde format (a tuple), or T-301 cannot map them to Firestore timestamps and TTL will silently never fire.
- Every field ending in `_at` must be an `OffsetDateTime`, and no other field may end in `_at`.
- `Precondition::MustExist` on a missing document is `PreconditionFailed`, not `NotFound`; there is no `NotFound` variant because `get` returns `Option`.
- `delete` with `Precondition::None` on a missing document returns `Ok(())`.
- Paging cursors are internal; never hand a `StoreCursor` to the browser unsealed (the api seals it, T-303).
- `RateLimitKey` must never contain a raw IP address or email; the caller hashes first.
- `NeedsAttentionRecord` has no `status` field on purpose: every S3 transition out of `open` deletes the record.
- `NeedsAttentionReason` comes from T-107. It must include a code for UN-01 AC6 "Sign in again", which S7 5.8 does not list; use `SignInRequired`, serialised `sign_in_required` `[DEFAULT]` (the name T-701 and T-705 use) (reported as a spec gap).
- The `classifier_eval` delete takes a list of pseudo IDs because the log pseudonymisation key rotates yearly (S5); one ID per key version.
- Do not add a collection for uniqueness, sessions by user or anything else; S5 allows exactly the eleven in `COLLECTIONS`.
- Do not match `Provider` with `_`.

## Out of scope

- Any implementation (T-202a fake, T-301 Firestore).
- Encryption itself (T-302) and how `list_key_hash` is keyed (T-605).
- Sweeper logic (T-706), session rules (T-501).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Every repository method has a one-line doc comment stating its order and filter, as above.
