# T-202b: Fakes for every other port, the Ports struct and the virtual clock

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 650 lines of code plus tests | T-201a, T-202a |

Split from T-202 (index row "In-memory fakes, Ports struct and virtual clock"). The store fake is T-202a; the mailbox fake is T-203.

**Read only these spec sections:** `docs/backlog/T-201a-port-traits.md` (the interfaces); S10 1 (three rules), 3.1 "Rules", 6.1 and 9.1 first bullet (`docs/specs/S10-test-strategy.md`); S6 section 5 "Sealed tokens" paragraph (`docs/specs/S6-security.md`). Nothing else is needed.

## Goal

Every service can be built from one `Ports` struct, and every test can build that struct entirely from deterministic fakes: virtual time, seeded randomness, real AES-256-GCM with an in-memory key wrap, a recording job scheduler, an egress fake that refuses everything not routed, and scripted identity, classifier and secrets fakes. The real `SystemClock` and `OsRng` also land here, as the only places allowed to read the wall clock or the OS random source.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/ports/src/ports.rs` | `Ports` struct |
| Change | `backend/crates/ports/src/lib.rs` | `pub mod ports; pub use ports::Ports;` |
| Create | `backend/crates/adapters-gcp/src/system_clock.rs` | `SystemClock` (the only `OffsetDateTime::now_utc()` call) |
| Create | `backend/crates/adapters-gcp/src/os_rng.rs` | `OsRng` (the only OS random call) |
| Create | `backend/crates/testkit/src/clock.rs` | `VirtualClock` |
| Create | `backend/crates/testkit/src/rng.rs` | `SeededRng` |
| Create | `backend/crates/testkit/src/keys.rs` | `FakeKeyService`, `FakeSystemKeyService` |
| Create | `backend/crates/testkit/src/scheduler.rs` | `FakeJobScheduler` |
| Create | `backend/crates/testkit/src/egress.rs` | `FakeHttpEgress` |
| Create | `backend/crates/testkit/src/identity.rs` | `FakeIdentityProvider` |
| Create | `backend/crates/testkit/src/classifier.rs` | `FakeClassifier` |
| Create | `backend/crates/testkit/src/secrets.rs` | `FakeSecrets` |
| Create | `backend/crates/testkit/src/app_folder.rs` | `InMemoryAppFolder` |
| Create | `backend/crates/testkit/src/null_mail.rs` | `NullMailProvider` (placeholder until T-203) |
| Create | `backend/crates/testkit/src/fake_ports.rs` | `fake_ports()` and `Fakes` |
| Change | `backend/crates/testkit/Cargo.toml` | add `aes-gcm`, `sha2`, `reqwest` (rustls, no default features), `tokio`, `url` |

## Types and signatures

```rust
// ports/src/ports.rs
#[derive(Clone)]
pub struct Ports {
    pub clock: Arc<dyn Clock>,
    pub rng: Arc<dyn Rng>,
    pub gmail: Arc<dyn MailProvider>,
    pub app_folder: Arc<dyn AppFolderStore>,
    pub store: Arc<dyn ServerStore>,
    pub keys: Arc<dyn KeyService>,
    pub system_keys: Arc<dyn SystemKeyService>,
    pub scheduler: Arc<dyn JobScheduler>,
    pub egress: Arc<dyn HttpEgress>,
    pub identity: Arc<dyn IdentityProvider>,
    pub models: Vec<Arc<dyn Classifier>>,   // bake-off models only; header rules is in-process (T-102)
    pub secrets: Arc<dyn Secrets>,
}
impl Ports {
    /// Exhaustive match: adding Provider::Graph in v2 must fail to compile here.
    pub fn mail(&self, provider: Provider) -> &Arc<dyn MailProvider> { match provider { Provider::Gmail => &self.gmail } }
}

// adapters-gcp
pub struct SystemClock; impl Clock for SystemClock {}         // OffsetDateTime::now_utc()
pub struct OsRng; impl Rng for OsRng {}                       // getrandom; uuid via Builder::from_random_bytes

// testkit/src/clock.rs
pub const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);
pub struct VirtualClock { now: Mutex<OffsetDateTime> }
impl VirtualClock { pub fn new(start: OffsetDateTime) -> Self; pub fn advance(&self, d: time::Duration); pub fn set(&self, t: OffsetDateTime); }

// testkit/src/rng.rs
pub struct SeededRng { seed: u64, counter: AtomicU64 }
impl SeededRng { pub fn new(seed: u64) -> Self; }            // bytes32 = SHA-256(seed_le || counter_le); counter += 1

// testkit/src/keys.rs
pub struct FakeKeyService { kek: [u8; 32], rng: Arc<dyn Rng>, calls: AtomicU64 }
impl FakeKeyService { pub fn new(rng: Arc<dyn Rng>) -> Self; pub fn unwrap_calls(&self) -> u64; }
pub struct FakeSystemKeyService { key: [u8; 32], rng: Arc<dyn Rng> }

// testkit/src/scheduler.rs
#[derive(Clone, Debug, PartialEq, Eq)] pub enum TaskState { Pending, Running, Done, Deleted }
pub struct FakeJobScheduler { tasks: Mutex<BTreeMap<TaskName, (JobId, OffsetDateTime, TaskState)>>, log: Mutex<Vec<SchedulerEvent>> }
#[derive(Clone, Debug, PartialEq, Eq)] pub enum SchedulerEvent { Scheduled(TaskName, OffsetDateTime), Cancelled(TaskName, CancelOutcome) }
impl FakeJobScheduler {
    pub fn new() -> Self;
    pub fn due(&self, now: OffsetDateTime) -> Vec<(TaskName, JobId)>; // Pending with due_at <= now
    pub fn start(&self, task: &TaskName);                             // Pending -> Running
    pub fn finish(&self, task: &TaskName);                            // Running -> Done
    pub fn events(&self) -> Vec<SchedulerEvent>;
}

// testkit/src/egress.rs
pub struct FakeHttpEgress { routes: Mutex<HashMap<String, Route>>, one_click: Mutex<VecDeque<Result<OneClickOutcome, EgressError>>>, log: Mutex<Vec<EgressRecord>> }
pub enum Route { Forward(SocketAddr), Scripted(VecDeque<Result<EgressResponse, EgressError>>) }
#[derive(Clone, Debug)] pub struct EgressRecord { pub host: String, pub method: HttpMethod, pub path: String, pub header_names: Vec<String>, pub allowed: bool }
impl FakeHttpEgress {
    pub fn new() -> Self;                                   // refuses every host
    pub fn forward(&self, host: &str, to: SocketAddr);      // e.g. "gmail.googleapis.com" -> fake-google
    pub fn script(&self, host: &str, r: Result<EgressResponse, EgressError>);
    pub fn script_one_click(&self, r: Result<OneClickOutcome, EgressError>);
    pub fn records(&self) -> Vec<EgressRecord>;
    pub fn violations(&self) -> Vec<EgressRecord>;          // allowed == false
}

// testkit/src/identity.rs
pub struct FakeIdentityProvider { /* scripted queues and logs */ }
impl FakeIdentityProvider {
    pub fn new() -> Self;
    pub fn script_exchange(&self, r: Result<TokenSet, IdError>);
    pub fn script_claims(&self, r: Result<IdClaims, IdError>);
    pub fn script_refresh(&self, refresh: &str, r: Result<String, IdError>);
    pub fn revoked(&self) -> Vec<String>;                  // raw values, test only
    pub fn authorize_requests(&self) -> usize;
}

// testkit/src/classifier.rs
pub struct FakeClassifier { id: ClassifierId, script: Mutex<HashMap<String, Result<Classification, ClassifierError>>>,
    default: Mutex<Result<Classification, ClassifierError>>, calls: AtomicU64, in_flight: AtomicU64, max_in_flight: AtomicU64,
    gate: Option<Arc<tokio::sync::Semaphore>> }
impl FakeClassifier {
    pub fn new(id: &str, default: Result<Classification, ClassifierError>) -> Self;
    pub fn script(&self, subject: &str, r: Result<Classification, ClassifierError>); // keyed by input.subject
    pub fn with_gate(self, gate: Arc<tokio::sync::Semaphore>) -> Self;              // classify waits for a permit
    pub fn calls(&self) -> u64; pub fn max_in_flight(&self) -> u64;
    pub fn inputs(&self) -> Vec<String>;                   // a digest per call (SHA-256 hex of the fields), never the text
}

// testkit/src/secrets.rs, app_folder.rs, null_mail.rs
pub struct FakeSecrets { map: Mutex<HashMap<SecretName, Vec<u8>>> }
impl FakeSecrets { pub fn with_defaults() -> Self; pub fn set(&self, n: SecretName, v: &[u8]); pub fn remove(&self, n: SecretName); }
pub struct InMemoryAppFolder { files: Mutex<HashMap<MailboxId, (Vec<u8>, u64)>>, fail_next: AtomicU32 }
impl InMemoryAppFolder { pub fn new() -> Self; pub fn user_deleted_file(&self, mb: &MailboxId); pub fn fail_next(&self, n: u32); }
pub struct NullMailProvider; // every method returns MailError::Invalid("no_mailbox_fake")

// testkit/src/fake_ports.rs
pub struct Fakes {
    pub clock: Arc<VirtualClock>, pub rng: Arc<SeededRng>, pub store: Arc<InMemoryServerStore>,
    pub keys: Arc<FakeKeyService>, pub system_keys: Arc<FakeSystemKeyService>, pub scheduler: Arc<FakeJobScheduler>,
    pub egress: Arc<FakeHttpEgress>, pub identity: Arc<FakeIdentityProvider>, pub app_folder: Arc<InMemoryAppFolder>,
    pub secrets: Arc<FakeSecrets>, pub gemini: Arc<FakeClassifier>, pub jev: Arc<FakeClassifier>,
}
pub fn fake_ports() -> (Ports, Fakes); // seed 42, clock T0, gmail = NullMailProvider (T-203 switches it to FakeMailbox)
```

## Algorithm

1. `FakeKeyService` (real crypto, simple format, test only):
   1. `kek` = `rng.bytes32()` at construction.
   2. `new_user_key(user)`: `dek = rng.bytes32()`; wrapped = AES-256-GCM(kek, nonce = first 12 bytes of `rng.bytes32()`, aad = `b"fake.dek|"` + user UUID bytes) as `nonce || ct || tag`.
   3. `seal(user, wrapped, aad, pt)`: unwrap (AAD binds the wrapped key to the user, so a key copied to another user fails with `OpenFailed`); encrypt with `aad_bytes = user UUID bytes || u32 BE len(scope) || scope || u32 BE len(field) || field`; output `nonce || ct || tag`.
   4. `open` reverses; any failure is `OpenFailed`; input shorter than 28 bytes is `Malformed`.
   5. Count unwraps in `calls` for tests that check caching elsewhere.
2. `FakeSystemKeyService`: the same, one fixed key, AAD `u32 len(scope) || scope || u32 len(field) || field`.
3. `FakeJobScheduler`:
   1. `schedule(job, due)`: name = `TaskName::for_job(job)`. If the name exists in any state, return it unchanged (idempotent). Else insert `Pending` and log.
   2. `cancel(task)`: `Pending` gives `Cancelled` (state `Deleted`); `Running` gives `AlreadyRunning`; `Done`, `Deleted` or unknown give `NotFound`. Log every call.
   3. No timers: tests call `due(now)` with the virtual clock and deliver the job themselves. Delivering a task twice is allowed (Cloud Tasks may), so `due` does not change state; `start` and `finish` do.
4. `FakeHttpEgress`:
   1. `call(req)`: host = `req.url.host_str()`. Not routed: push a record with `allowed: false` and return `Err(HostNotAllowed)`. `Scripted`: pop the next result (empty queue gives `Err(Connect)`). `Forward(addr)`: rewrite the URL to `http://{addr}{path}?{query}`, send with a `reqwest::Client` built with `redirect(Policy::none())`, `no_proxy()` and the request timeout; return status, headers and body.
   2. `one_click_post(url)`: if the host is routed to `Forward(addr)`, POST body `List-Unsubscribe=One-Click` with `Content-Type: application/x-www-form-urlencoded` and map the status like T-306 (2xx Accepted, 3xx Redirected, else Rejected; timeout TimedOut). Otherwise pop `script_one_click`; empty queue is a violation and `Err(HostNotAllowed)`.
   3. Records keep header names only, never values.
5. `FakeIdentityProvider`: `authorize_url` returns `https://accounts.example.test/o/oauth2/v2/auth?state=..&nonce=..&code_challenge=..&code_challenge_method=S256` plus `prompt` and `max_age` when set; each scripted queue pops in order; an empty queue returns `Err(IdError::Unavailable)`. `revoke` records the raw value and makes later `refresh` of that value return `InvalidGrant`.
6. `FakeClassifier::classify`: increment `calls` and `in_flight`, update `max_in_flight`, wait for a gate permit if a gate is set, look up `input.subject` in the script, else the default; decrement `in_flight`. Never sleep.
7. `InMemoryAppFolder`: ETag is the per-mailbox write counter as a string. `write(None)` with a file present is `Conflict`; `write(Some(tag))` with a different current tag or no file is `Conflict`; `delete` is idempotent; `user_deleted_file` removes the file.
8. `fake_ports()` wires all of the above with `SeededRng::new(42)` and `VirtualClock::new(T0)`; `models = vec![gemini, jev]` with ids `gemini@flash-lite` and `jev@1.13.0` and a default `Err(ClassifierError::Disabled)`.
9. `SystemClock::now` returns `OffsetDateTime::now_utc()`. `OsRng::bytes32` fills from `getrandom`; on the (practically impossible) error, abort the process with a fixed message rather than return weak bytes; this is the one place a hard stop is right, write it with `std::process::abort()` and a comment.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | Test infrastructure only. It enables S10 rule 2 (deterministic by construction) and S10 6.1 (no test reaches a real unsubscribe target) |

## Tests that must pass

- `virtual_clock_advances_only_when_told` (unit).
- `seeded_rng_is_deterministic_and_seeds_differ` (unit).
- `fake_key_service_round_trip` (unit).
- `fake_key_service_aad_binding_rejects_other_user_scope_or_field` (unit): change each of user, scope and field in turn; each `open` is `OpenFailed`.
- `fake_key_service_wrapped_key_bound_to_user` (unit).
- `fake_scheduler_schedule_is_idempotent_and_cancel_outcomes` (unit: Pending, Running, Done, unknown).
- `fake_egress_refuses_unrouted_host_and_records_violation` (unit).
- `fake_egress_forward_follows_no_redirect` (integration with a local `axum` route answering 302).
- `fake_identity_revoke_makes_refresh_fail` (unit).
- `fake_classifier_counts_calls_and_max_in_flight` (unit with a gate).
- `in_memory_app_folder_etag_conflicts` (unit).
- `ports_mail_matches_every_provider` (unit).
- `fake_ports_builds` (unit).

## Edge cases and traps

- Never call `OffsetDateTime::now_utc()`, `SystemTime::now()`, `rand::thread_rng()` or `getrandom` in `testkit`; the T-003 Semgrep rule excludes only `system_clock.rs` and `os_rng.rs`.
- Never `tokio::time::sleep` in a fake; use the gate or the virtual clock.
- The egress fake must default to refuse. A fake that forwards unknown hosts to the internet defeats S10 6.1.
- `FakeHttpEgress` uses plain `http://` to local sockets on purpose; that permission lives only in `testkit` (production rules are T-306).
- `FakeClassifier::inputs` stores digests only, so a test log can never print card text.
- `Ports` is `Clone` (all `Arc`); do not put non-`Arc` state in it.
- Do not match `Provider` with `_` in `Ports::mail`.
- Fakes must be `Send + Sync`: use `std::sync::Mutex` and atomics, and never hold a lock across `.await` (the classifier gate wait happens with no lock held).

## Out of scope

- `FakeMailbox` and the mail and app folder contract suites (T-203).
- Real adapters (M3, M4).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Doc comments on `fake_ports` and `Fakes` explain each handle.
