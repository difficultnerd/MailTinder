# T-202a: In-memory ServerStore and the ServerStore contract suite

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 500 lines of code plus about 300 lines of contract tests | T-201b |

Split from T-202 (index row "In-memory fakes, Ports struct and virtual clock"). T-202a is the store half; T-202b holds every other fake, the `Ports` struct and the virtual clock.

**Read only these spec sections:** `docs/backlog/T-201b-server-store-traits.md` (the whole file: it is the interface you implement); S10 1 rule 3 and 3.1 "Rules" (`docs/specs/S10-test-strategy.md`); S3 "Invariants" (`docs/specs/S3-domain-model.md`). Nothing else is needed.

## Goal

`testkit` gains `InMemoryServerStore`, a complete in-memory implementation of every repository in T-201b with the same concurrency and ordering rules Firestore will have, and `testkit::contract::server_store`, one shared suite that any `ServerStore` must pass. The Firestore adapter (T-301) runs the same suite against the emulator, so the fake cannot drift.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/testkit/src/store/mod.rs` | `InMemoryServerStore` |
| Create | `backend/crates/testkit/src/store/table.rs` | generic `Table<K, R>` with versions and preconditions |
| Create | `backend/crates/testkit/src/contract/mod.rs` | `pub mod server_store;` (T-203 adds `mail_provider`) |
| Create | `backend/crates/testkit/src/contract/server_store.rs` | the shared suite |
| Change | `backend/crates/testkit/src/lib.rs` | `pub mod store; pub mod contract;` |
| Create | `backend/crates/testkit/tests/server_store_in_memory.rs` | runs the suite against the fake |

## Types and signatures

```rust
// testkit/src/store/table.rs
pub(crate) struct Table<K, R> { rows: std::sync::Mutex<BTreeMap<K, (R, u64)>>, next_version: AtomicU64 }
impl<K: Ord + Clone, R: Clone + Keyed<Key = K>> Table<K, R> {
    pub fn get(&self, k: &K) -> Option<Versioned<R>>;
    pub fn put(&self, r: &R, pre: &Precondition) -> Result<Version, StoreError>;
    pub fn delete(&self, k: &K, pre: &Precondition) -> Result<(), StoreError>;
    pub fn scan(&self) -> Vec<Versioned<R>>;          // snapshot clone, key order
    pub fn retain(&self, keep: impl Fn(&R) -> bool) -> u64; // returns number removed
}

// testkit/src/store/mod.rs
#[derive(Default)]
pub struct InMemoryServerStore { /* one Table per collection, plus config and rate_limits maps */ }
impl InMemoryServerStore {
    pub fn new() -> Self;
    /// Test helper: every document as (collection, serde_json::Value), for leak scans (S10 7.3).
    pub fn export_json(&self) -> Vec<(&'static str, serde_json::Value)>;
    /// Test helper: make the next N calls of any method fail with StoreError::Unavailable.
    pub fn fail_next(&self, n: u32);
}
impl ServerStore for InMemoryServerStore { /* returns &self.users etc. */ }

// testkit/src/contract/server_store.rs
/// Each call to `make` returns a fresh, empty store.
pub async fn server_store<F, Fut>(make: F) -> Result<(), String>
where F: Fn() -> Fut, Fut: std::future::Future<Output = std::sync::Arc<dyn ServerStore>>;
/// Sample records for tests (fixed UUIDs and times; example.com nowhere because fields are ciphertext).
pub mod samples {
    pub fn user(n: u8) -> UserRecord; pub fn mailbox(user: &UserId, n: u8) -> MailboxRecord;
    pub fn invite(n: u8) -> InviteRecord; pub fn invite_request(n: u8) -> InviteRequestRecord;
    pub fn job(user: &UserId, mailbox: &MailboxId, n: u8) -> JobRecord;
    pub fn needs_attention(user: &UserId, mailbox: &MailboxId, n: u8) -> NeedsAttentionRecord;
    pub fn session(user: Option<&UserId>, n: u8) -> SessionRecord;
    pub fn eval(pseudo: &str, n: u8) -> ClassifierEvalRecord;
    pub fn snapshot(n: u8) -> BakeoffSnapshotRecord;
    pub fn classifiers(gemini: bool, jev: bool) -> ClassifiersConfig;
}
```

## Algorithm

1. `Table::put`:
   1. Lock. Look up the key.
   2. `Precondition::None`: always write. `MustNotExist`: if present, `AlreadyExists`. `MustExist`: if absent, `PreconditionFailed`. `Matches(v)`: if absent or the stored version string differs, `PreconditionFailed`.
   3. New version = `next_version.fetch_add(1) + 1`, stored as `u64`, returned as `Version(n.to_string())`.
2. `Table::delete`: same precondition rules; missing with `None` is `Ok(())`.
3. Every repository query in T-201b is a `scan()` plus filter plus sort plus limit, following the order stated in each method's comment. Ties break by key, ascending.
4. Paging: sort, then skip items up to and including the cursor key; `StoreCursor` is the last returned key rendered as a string (for UUIDs, the hyphenated form; for `SessionHash`, hex). `limit` is clamped to `1..=MAX_PAGE`; `next` is `Some` only when more items remain.
5. `RateLimitRepo::hit`: map keyed by `(key, window_start)`; increment under the lock; create with `count = 1` and `expires_at = window_start + window`.
6. `ConfigRepo`: one `Option<(ClassifiersConfig, u64)>` with the same precondition rules.
7. `expires_by(now, limit)` returns records whose TTL field is `<= now`, oldest first. For `RateLimitRepo` and `NeedsAttentionRepo` follow the signature (delete versus list) exactly.
8. `fail_next(n)`: an `AtomicU32` checked at the top of every method; when positive, decrement and return `Unavailable`.
9. `export_json` serialises each record with `serde_json::to_value`.
10. Contract suite, one `async fn` per case below, called in order by `server_store(make)`, each on a fresh store. Return `Err(format!("<case>: <what failed>"))` on the first failure; never panic.

Cases in the contract suite (the minimum; add more if a method has no case):

- put then get returns the record and a version; second put with `Matches(old)` fails `PreconditionFailed`; with `Matches(current)` succeeds and changes the version.
- `MustNotExist` on an existing key gives `AlreadyExists`; `MustExist` on a missing key gives `PreconditionFailed`.
- delete missing with `None` is `Ok`; delete with stale `Matches` fails and leaves the record.
- conditional transition race: two writers read the same job version and both `put` with `Matches(v)`; exactly one succeeds (SW-05 AC5 depends on this).
- mailbox `by_subject` finds by (provider, subject); `by_user` returns only that user's mailboxes in `linked_at` order; `delete_all_for_user` removes only theirs.
- invite `by_token_hash`, `by_email_lookup`, `list` with and without status filter, `purge_due`.
- job `queued_for_list` returns only queued jobs of that user and list; `by_user_with_outcome` skips jobs with no outcome; `expires_by` returns overdue jobs only.
- needs attention `by_user` pages newest first across two pages with no duplicates and no gaps; `count_for_user`; `expires_by`.
- session `by_user`, `expires_by`, `delete_all_for_user`.
- classifier eval `range` is half-open `[from, to)`; `delete_for_users` with two pseudo IDs removes both users' rows and no others.
- snapshot `list` newest first; `count`.
- config: absent at start; put with `MustNotExist`; read back.
- rate limits: three `hit`s in one window return 1, 2, 3; a new window starts at 1; `expires_by` removes the old window.
- paging: `limit` 0 is treated as 1; `limit` 1000 as `MAX_PAGE`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | No S2 AC is proved here. The suite underpins SW-05 AC5 (T-606), INV-2 and JOB-1 (T-706) and DEL-1 (T-803) |

## Tests that must pass

- `server_store_contract_in_memory` (contract, `testkit/tests/server_store_in_memory.rs`): `server_store(|| async { Arc::new(InMemoryServerStore::new()) as Arc<dyn ServerStore> }).await` returns `Ok`.
- `in_memory_fail_next_returns_unavailable` (unit).
- `in_memory_export_json_lists_every_collection_written` (unit).
- `in_memory_versions_never_repeat` (property, `proptest`: random put and delete sequences; every returned version is new).

## Edge cases and traps

- Hold the `Mutex` only inside one `Table` call; never across an `.await` (use `std::sync::Mutex`, and the trait methods do no awaiting inside the lock).
- Do not use `HashMap` iteration order for queries; sort explicitly, or tests become flaky (S10 10.5).
- Versions are per store, not per table, so a version from one record never matches another by accident.
- The fake must enforce preconditions exactly like Firestore; a fake that always succeeds would hide the undo race.
- `delete` of a missing key with `MustExist` is `PreconditionFailed`, not `Ok`.
- Do not add a `clear()` or reset method used by services; tests make a fresh store instead.
- `Table` needs `K: Ord`: add `PartialOrd, Ord` derives to the T-201b ID and hash newtypes that lack them (additive).
- The contract suite must not depend on wall time: use the fixed times from `samples`.
- No `unwrap` in the suite; it returns `Result<(), String>` so the T-301 emulator run reports the failing case by name.

## Out of scope

- Other fakes, `Ports`, virtual clock (T-202b).
- Firestore (T-301), which reuses this suite.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Every method of every repository in T-201b is exercised by at least one contract case.
