# Backlog conventions

Shared names every task file uses, so tasks written separately fit together. A task may add to these; it must not rename them. If a task needs a different shape, it says so in "Edge cases and traps" and the change lands in this file in the same pull request.

## Repository layout

```
backend/
  Cargo.toml                 workspace root (T-001 converts the template's single crate)
  crates/
    domain/                  entities, classes, rules, state machines (no I/O, no provider types)
    ports/                   traits only
    adapters-gmail/          Gmail and Drive over HTTP (v1)
    adapters-gcp/            Firestore, Cloud KMS, Cloud Tasks, Secret Manager, Vertex AI
    adapters-models/         Jev client (Gemini lives in adapters-gcp)
    egress/                  HttpEgress implementation with the allowlist and SSRF checks
    obs/                     structured logging, redaction types, pseudonymous IDs
    testkit/                 fakes, fixtures, contract suites (dev-dependency only)
    api/                     Cloud Run service binary
    unsub/                   Cloud Run service binary
    worker/                  Cloud Run service binary (sweeps)
    fake-google/             HTTP fake binary (Gmail, Drive, OAuth and OIDC)
    unsub-testbed/           local unsubscribe sites binary
app/lib/
  main.dart
  api/                       ApiClient interface, HttpApiClient, FakeApiClient, models
  state/                     ChangeNotifier view models
  screens/<screen>/          one folder per S9 screen
  widgets/                   shared widgets
scripts/e2e.sh
tools/check_ac_coverage.py
infra/terraform/{modules,envs/prod,envs/staging}
```

## Rust conventions

- Edition 2021, `rust-version` from the template. Libraries: `tokio`, `axum`, `serde`, `serde_json`, `thiserror`, `async-trait`, `uuid` (v4 and v5), `time`, `reqwest` (rustls only), `proptest` (dev). Add others only when a task names them.
- No `unwrap` or `expect` outside tests (clippy denies them via `-D warnings`); tests return `Result` and use `?`.
- Each crate has one `Error` enum (`thiserror`). The `api` crate maps errors to the S7 section 4 error model.
- Ports are object safe: `#[async_trait] pub trait X: Send + Sync`. Services hold `Arc<dyn X>` inside a `Ports` struct.
- Time comes only from `Clock`; randomness only from `Rng`. No `SystemTime::now()` or `rand::thread_rng()` outside the real adapters.
- Secrets and personal data use `obs::Sensitive<T>`, which prints `[redacted]` in `Debug` and `Display` and has no `Serialize`.

## Core domain types (`domain`)

```rust
pub struct UserId(pub Uuid);
pub struct MailboxId(pub Uuid);
pub struct MessageId(pub String);        // provider message ID, opaque
pub struct JobId(pub Uuid);
pub struct RuleId(pub Uuid);
pub struct CategoryId(pub Uuid);
pub enum Provider { Gmail }              // Graph added in v2; match arms must not use `_` so v2 forces a review
pub struct SenderKey(String);            // normalised: lower case, relays unwrapped (S3)
pub enum MessageClass { List, BulkNoHeader, Notice, Personal, Suspect }
pub enum SwipeAction { Keep, Skip, Reject, File { category: CategoryId } }

pub struct HeaderFacts {                 // built by the adapter from headers; booleans and keys only
    pub list_unsubscribe: Option<UnsubscribeOptions>, // parsed, DKIM-covered only (T-406)
    pub list_id: Option<String>,
    pub feedback_id: Option<String>,
    pub precedence_bulk: bool,
    pub auto_submitted: bool,
    pub from_authenticated: bool,        // DKIM-aligned From or provider pass (S3 rule counting)
    pub esp_hint: Option<String>,        // e.g. "mailchimp", for bulk_reason
    pub is_reply_or_thread: bool,
}
pub struct UnsubscribeOptions { pub one_click_https: Option<Url>, pub https: Option<Url>, pub mailto: Option<MailtoTarget> }

pub struct MessageMeta {                 // what the Feed needs; no body
    pub mailbox: MailboxId, pub id: MessageId, pub internal_date: OffsetDateTime,
    pub from_display: String, pub from_address: String, pub sender: SenderKey,
    pub subject: String, pub labels: LabelSet, pub facts: HeaderFacts,
}
pub struct LabelSet(BTreeSet<String>);   // exact provider label IDs, for exact restore

pub struct Classification { pub class: MessageClass, pub bulk_score: u8, pub bulk_reason: String,
                            pub confidence: Option<f32>, pub probabilities: Option<[f32; 5]> }
```

## Ports (`ports`)

```rust
#[async_trait] pub trait Clock: Send + Sync { fn now(&self) -> OffsetDateTime; }
#[async_trait] pub trait Rng: Send + Sync { fn bytes32(&self) -> [u8; 32]; fn uuid_v4(&self) -> Uuid; }

#[async_trait] pub trait MailProvider: Send + Sync {
    async fn list_inbox(&self, mb: &MailboxCtx, page: Option<PageToken>, order: ListOrder) -> Result<MessagePage, MailError>;
    async fn get_meta(&self, mb: &MailboxCtx, id: &MessageId) -> Result<MessageMeta, MailError>;
    async fn get_preview(&self, mb: &MailboxCtx, id: &MessageId) -> Result<String, MailError>; // plain text, stripped (T-402)
    async fn set_labels(&self, mb: &MailboxCtx, id: &MessageId, add: &LabelSet, remove: &LabelSet) -> Result<LabelSet, MailError>;
    async fn trash(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;     // returns labels before
    async fn report_spam(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;
    async fn restore_labels(&self, mb: &MailboxCtx, id: &MessageId, exact: &LabelSet) -> Result<(), MailError>;
    async fn ensure_label(&self, mb: &MailboxCtx, name: &str) -> Result<String, MailError>;   // returns label ID
    async fn send_mailto(&self, mb: &MailboxCtx, to: &MailtoTarget) -> Result<(), MailError>;
    async fn inbox_count(&self, mb: &MailboxCtx) -> Result<u64, MailError>;
    // No delete method exists (INV-5).
}
pub struct MailboxCtx { pub mailbox: MailboxId, pub access_token: Sensitive<String> }
pub enum MailError { Unauthorized, Forbidden, NotFound, RateLimited { retry_after_s: u64 }, Transient, Invalid(String) }

#[async_trait] pub trait AppFolderStore: Send + Sync {
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError>;   // ciphertext
    async fn write(&self, mb: &MailboxCtx, bytes: &[u8], if_match: Option<&ETag>) -> Result<ETag, AppFolderError>; // Conflict on ETag mismatch
    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError>;
}

#[async_trait] pub trait ServerStore: Send + Sync { /* typed repositories, one per S3 collection, defined in T-201:
    users(), mailboxes(), invites(), invite_requests(), jobs(), needs_attention(), sessions(),
    classifier_eval(), bakeoff_snapshots(), config(); each with get, put (create or replace with precondition), delete, query helpers named by the task that needs them */ }

#[async_trait] pub trait KeyService: Send + Sync {
    async fn new_user_key(&self, user: &UserId) -> Result<WrappedKey, KeyError>;
    async fn seal(&self, user: &UserId, wrapped: &WrappedKey, aad: &Aad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError>;
    async fn open(&self, user: &UserId, wrapped: &WrappedKey, aad: &Aad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError>;
}
pub struct Aad { pub user: UserId, pub scope: String, pub field: &'static str } // e.g. scope = mailbox ID, field = "refresh_token"

#[async_trait] pub trait JobScheduler: Send + Sync {
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError>;
    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError>; // AlreadyRunning is a normal outcome
}

#[async_trait] pub trait HttpEgress: Send + Sync {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError>; // checks, pinning, no redirects (T-306)
    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError>;   // allowlisted hosts only
}

#[async_trait] pub trait IdentityProvider: Send + Sync {
    fn authorize_url(&self, req: &AuthRequest) -> Url;      // PKCE, state, nonce, prompt, max_age
    async fn exchange(&self, code: &str, verifier: &Sensitive<String>) -> Result<TokenSet, IdError>;
    async fn validate_id_token(&self, raw: &str, nonce: &str) -> Result<IdClaims, IdError>; // sub, email, email_verified, auth_time, amr
    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError>; // access token
    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError>;
}

#[async_trait] pub trait Classifier: Send + Sync {
    fn id(&self) -> ClassifierId;
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}
```

## Flutter conventions

- No state-management package: `ChangeNotifier` view models in `lib/state/`, read with `ListenableBuilder`. Routing with `Navigator` and named routes.
- One `ApiClient` abstract class in `lib/api/api_client.dart`; `HttpApiClient` and `FakeApiClient` implement it. Models are hand-written classes with `fromJson`, matching `docs/specs/S7-api-contract.openapi.yaml`.
- Copy strings come from S9 word for word, kept in `lib/copy.dart`.
- Every control has a `Semantics` label; every swipe has a button (XC-03).
- Nothing about cards is written to browser storage.

## Test naming

From S10 section 2: Rust `<story>_<ac>_<behaviour>` (for example `sw_03_ac2_reject_queues_unsubscribe_with_delay`), invariants `inv_5_...`, ASVS `asvs_v6_3_3_...`, S5 IDs `ses_1_...`, classifier IDs `guard_1_...`, `bake_1_...`, `stat_1_...`. Dart descriptions start with the ID: `'SW-03 AC2 reject toast states the delay'`.

## Additions from the task files (3 October 2026)

Shared items that task files define. The owning task creates them; later tasks import them.

| Item | Owner | Notes |
| --- | --- | --- |
| Crates `svc-common` (token minting, internal caller check, Needs Attention writer, job record encryption), `e2e` (WebDriver journeys, `fantoccini`), `smoke` (staging smoke binary) | T-503 creates `svc-common` with `mint.rs`; T-701 adds the other modules; T-1101a, T-1105 | `unsub` and `worker` never import `api` |
| `HeaderFacts` gains `list_unsubscribe_present`, `reply_to_mismatch`, `display_name_spoof`; derives `Default` | T-101; T-401 fills them | |
| Hand-written redacting `Debug` for `MessageMeta`, `HeaderFacts`, `UnsubscribeOptions`, `MailtoTarget`, `SenderKey`, `MessageId`, `SortRule`, `SwipeRecord`; `MessageId` has no `Display` | T-101, T-104 | Deriving `Debug` would trip the privacy rules |
| `SenderStats`, `Tunables`, `ClassifierId`, `MailboxIdentity`, `ProviderSubjectId`, `MailboxStatus`, `DomainError` | T-101 | T-602b imports `SenderStats` |
| `EmailAddress`, `MailtoTarget::parse` | T-404 | |
| `JobStatus`, `JobMethod`, `JobState`, `JobEvent` | T-106 | |
| `InviteId`, `InviteRequestId`, `NeedsAttentionId`, `InviteStatus`, `NeedsAttentionReason` (includes `SignInRequired`, wire value `sign_in_required`) | T-107 | One name everywhere |
| `SortRule` (field `matcher`, serialised as `match`) | T-104 | |
| `SwipeOutcome`, `derive_swipe_ids` | T-105a | UUID v5 from `job:<key>` and `rule:<key>` |
| UUID v5 namespaces `NS_SWIPE`, `NS_NA_ITEM`, `NS_RULE_ACTION`, `NS_RULE_FROM_PROMPT`, and a separate eval namespace | T-605, T-906a | Eval IDs must not collide with job or rule IDs |
| `AchievementId`, `AchievementProgress` | T-109 | Flutter uses the same strings |
| `domain::user_state` (`UserState`, `StoredRule`, `MailboxPosition`, `SkipState`, `RecentSwipe`, `PendingUnsubscribe`, `Totals`) and `UserStateStore` in `api` | T-602b | T-802 adds `levels_cleared` |
| `MailProvider` gains `provider()`, `capabilities()`, `list_messages`, `count_messages`, `rename_label`, `remove_label`, `web_url`, `get_text(mb, id, max_chars)`; `MessageQuery`; `ListOrder` (`NewestFirst`, `NewerThan`, `OlderThan`); `MessagePage { items, next }` | T-201a, T-602a, T-903 | |
| `IdentityProvider::exchange` takes `redirect_uri` | T-201a | |
| `EgressRequest.timeout: Duration`; `RefusedRange`; full `EgressError` | T-201a, T-306 | |
| Ports `SystemKeyService` (second KMS key for data that exists before a user), `Secrets` with `SecretName`, `CallerVerifier`, `InviteMailer` | T-201a, T-701, T-404 | `InviteMailer` lives on `AppState` |
| `ServerStore` gains `rate_limits()`, `Version`, `Precondition`; `aad_fields` constants; `JobOutcomeCode` (includes `Batched`, `MailboxRemoved`, `OwnerMismatch`, `Refused`); `JobRecord.sender_display` (encrypted); `TaskName::for_job`; `mailbox_id_for` (UUID v5 of provider and subject) | T-201b, T-701 | |
| `cancel_queued_job` (get, then put with `Precondition::Matches`) | T-601b | T-606, T-701 and T-803 reuse it |
| `AdminSession` (T-501), `SteppedUpAdmin` (T-504) | | Axum extractors |
| Real clock and RNG only in files named `system_clock.rs` and `os_rng.rs` | T-202b, T-003 | Semgrep enforces |
| Flutter: `app/lib/platform/` with conditional imports; `ApiException.problem`; `FakeApiClient` handler and call-recording pattern; view models take a `DateTime Function()` clock | T-1001a, T-1007b | |
