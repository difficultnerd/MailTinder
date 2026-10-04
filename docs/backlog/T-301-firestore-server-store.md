# T-301: Firestore ServerStore adapter

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | sonnet | about 750 lines of code plus tests (the shared contract suite already exists) | T-202a, T-202b |

**Read only these spec sections:** `docs/backlog/T-201b-server-store-traits.md` (the interface, TTL fields and ordering rules); `docs/backlog/T-202a-in-memory-store-and-contract-suite.md` ("Types and signatures" for the suite); S5 "Firestore (server)" table and the sentence after it (`docs/specs/S5-data-inventory.md`); S4 1 "Data store" row and S4 5.7 first bullet (`docs/specs/S4-architecture.md`); S10 10.1 last line (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

## Goal

`adapters-gcp` gains `FirestoreStore`, the production `ServerStore` over the Firestore REST API (`v1`), with optimistic concurrency from document update times, atomic rate-limit counters, timestamp-typed TTL fields, and the shared platform HTTP client and token source the other Google Cloud adapters (T-302, T-304, T-305) reuse. The T-202a contract suite runs against the Firestore emulator in CI, so the fake and the real store behave the same.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gcp/src/gcp_http.rs` | `GcpHttp`: the one HTTP client for Google Cloud platform APIs |
| Create | `backend/crates/adapters-gcp/src/token_source.rs` | `TokenSource`, `MetadataTokenSource`, `StaticTokenSource` |
| Create | `backend/crates/adapters-gcp/src/firestore/mod.rs` | `FirestoreStore`, `FirestoreConfig` |
| Create | `backend/crates/adapters-gcp/src/firestore/value.rs` | `serde_json::Value` to and from Firestore `Value` |
| Create | `backend/crates/adapters-gcp/src/firestore/rest.rs` | `get`, `commit`, `run_query` calls |
| Create | `backend/crates/adapters-gcp/src/firestore/repos.rs` | one `impl` per repository trait |
| Create | `backend/crates/adapters-gcp/firestore-indexes.json` | composite indexes the queries need (Terraform T-1102 applies them) |
| Create | `backend/crates/adapters-gcp/tests/firestore_contract.rs` | contract suite against the emulator, `#![cfg(feature = "firestore-emulator")]` |
| Change | `backend/crates/adapters-gcp/Cargo.toml` | deps `reqwest` (workspace, rustls), `serde_json`, `base64`, `time`, `tokio`; feature `firestore-emulator = []` |
| Create | `scripts/firestore-emulator.sh` | start the emulator on `127.0.0.1:8085` and wait until it answers |
| Change | `.github/workflows/ci.yml` | `rust` job starts the emulator before `cargo test` (S10 10.1) |

## Types and signatures

```rust
// token_source.rs
#[async_trait::async_trait]
pub trait TokenSource: Send + Sync { async fn bearer(&self) -> Result<obs::Sensitive<String>, GcpError>; }
pub struct MetadataTokenSource { http: reqwest::Client, clock: Arc<dyn Clock>, cache: Mutex<Option<(obs::Sensitive<String>, OffsetDateTime)>> }
impl MetadataTokenSource { pub fn new(clock: Arc<dyn Clock>) -> Self; }
pub struct StaticTokenSource(pub obs::Sensitive<String>);   // emulator ("owner") and tests

// gcp_http.rs
pub const PLATFORM_HOSTS: [&str; 4] = ["firestore.googleapis.com", "cloudkms.googleapis.com",
                                       "secretmanager.googleapis.com", "cloudtasks.googleapis.com"];
pub struct GcpHttp { client: reqwest::Client, tokens: Arc<dyn TokenSource>, emulator: Option<SocketAddr> }
impl GcpHttp {
    pub fn new(tokens: Arc<dyn TokenSource>) -> Result<Self, GcpError>;
    pub fn with_emulator(tokens: Arc<dyn TokenSource>, emulator: SocketAddr) -> Result<Self, GcpError>;
    /// Refuses any URL whose host is not in PLATFORM_HOSTS (or the emulator socket over http).
    pub async fn json<B: Serialize, T: DeserializeOwned>(&self, method: reqwest::Method, url: &Url, body: Option<&B>) -> Result<T, GcpError>;
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GcpError {
    #[error("host not allowed")] HostNotAllowed,
    #[error("unauthenticated")] Unauthenticated,
    #[error("permission denied")] PermissionDenied,
    #[error("not found")] NotFound,
    #[error("already exists")] AlreadyExists,
    #[error("failed precondition")] FailedPrecondition,  // Firestore update_time mismatch
    #[error("conflict")] Aborted,                         // contention; retryable
    #[error("unavailable")] Unavailable,                  // 429, 5xx, timeout, connect
    #[error("bad response")] BadResponse,
}

// firestore/mod.rs
pub struct FirestoreConfig { pub project_id: String, pub database: String } // database "(default)"
pub struct FirestoreStore { http: Arc<GcpHttp>, base: Url, /* repo structs */ }
impl FirestoreStore { pub fn new(http: Arc<GcpHttp>, cfg: FirestoreConfig) -> Self; }
impl ServerStore for FirestoreStore { /* returns &self.users and so on */ }

// firestore/value.rs
pub fn to_fields(v: &serde_json::Value) -> Result<serde_json::Map<String, serde_json::Value>, GcpError>; // top-level object only
pub fn from_fields(fields: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, GcpError>;
```

## Algorithm

1. Base URL: production `https://firestore.googleapis.com/v1/projects/{project}/databases/(default)/documents`; emulator `http://{emulator}/v1/projects/{project}/databases/(default)/documents`. Document name: `{base}/{collection}/{doc_id}`.
2. Document IDs: UUID keys in hyphenated lower case; `SessionHash` as 64-char hex; config as `classifiers`; rate limits as `{key}:{window_start unix seconds}`.
3. Value mapping (`value.rs`), applied recursively:
   - `null` to `{"nullValue": null}`; bool to `booleanValue`; integer to `integerValue` (a decimal string); other numbers to `doubleValue`; array to `arrayValue.values`; object to `mapValue.fields`.
   - string: if its object key ends in `_at` (T-201b rule), parse as RFC 3339 and emit `timestampValue` in RFC 3339 UTC; a parse failure is `BadResponse` (a bug). Otherwise `stringValue`.
   - Reverse mapping: `timestampValue` back to an RFC 3339 string; `integerValue` string back to a number; `doubleValue` to a number.
4. `get`: `GET {name}`; 404 gives `Ok(None)`; else map fields to JSON, deserialise the record, `Version(updateTime)`.
5. `put(record, pre)`: `POST {base}:commit` with one write: `{"update": {"name", "fields"}, "currentDocument": <pre>}`. No `updateMask`, so the whole document is replaced. `pre`: `None` omits `currentDocument`; `MustNotExist` gives `{"exists": false}`; `MustExist` gives `{"exists": true}`; `Matches(v)` gives `{"updateTime": v}`. Response `writeResults[0].updateTime` is the new `Version`. Map errors: `ALREADY_EXISTS` to `AlreadyExists`; `FAILED_PRECONDITION` or `NOT_FOUND` with a precondition to `PreconditionFailed`; `ABORTED`, `UNAVAILABLE`, `DEADLINE_EXCEEDED`, 429, 5xx to `Unavailable`.
6. `delete(key, pre)`: commit with `{"delete": name, "currentDocument": ...}`; with `None`, a missing document is fine.
7. Queries: `POST {base}:runQuery` with `structuredQuery` `{from:[{collectionId}], where, orderBy, limit, startAt}`. Filters use `fieldFilter` (`EQUAL`, `LESS_THAN_OR_EQUAL`, `GREATER_THAN_OR_EQUAL`, `LESS_THAN`) combined with `compositeFilter` `AND`. Order exactly as each T-201b method states, then `__name__` ascending as the tiebreak.
8. Paging cursor: `StoreCursor` = unpadded base64url of JSON `[<last order value as Firestore Value>, "<last document name>"]`; next query uses `startAt: {values: [...], before: false}`. Fetch `limit + 1` rows to know if `next` exists.
9. Bulk deletes (`delete_all_for_user`, `delete_for_users`, `expires_by` for rate limits): query names (`select: {fields: [{fieldPath: "__name__"}]}`), then commit deletes in batches of at most 500 writes; repeat until the query returns nothing; return the count.
10. `RateLimitRepo::hit`: one commit write `{"update": {"name", "fields": {key, window_start_at, expires_at}}, "updateMask": {"fieldPaths": ["key","window_start_at","expires_at"]}, "updateTransforms": [{"fieldPath": "count", "increment": {"integerValue": "1"}}]}`. The new count is `writeResults[0].transformResults[0].integerValue`. This is atomic in Firestore; no read first.
11. Retries: none inside the store. `Unavailable` goes to the caller.
12. `MetadataTokenSource`: `GET http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token` with header `Metadata-Flavor: Google`; cache until `expires_in - 60` seconds by `Clock`. This client is separate from `HttpEgress` on purpose: the egress SSRF policy refuses the metadata server, and only this fixed URL may reach it.
13. `GcpHttp`: `reqwest::Client` with `redirect(Policy::none())`, `https_only(true)` unless the emulator is configured, 10-second timeout `[DEFAULT]`, rustls. Before each call check the URL host is in `PLATFORM_HOSTS` (exact match) or equals the emulator socket.
14. `firestore-indexes.json`: list every composite index the queries need, in the Terraform `google_firestore_index` shape (collection, fields with order). At least: `mailboxes(user_id ASC, linked_at ASC)`, `invites(status ASC, created_at DESC)`, `jobs(user_id ASC, list_key_hash ASC, status ASC)`, `jobs(user_id ASC, due_at ASC)` for `by_user_with_outcome`, `needs_attention(user_id ASC, created_at DESC)`, `classifier_eval(user_pseudo_id ASC)` (single field, automatic), plus `expires_at` single-field indexes (automatic). The emulator does not enforce indexes, so this file is the only guard; T-1101's staging smoke catches a miss.
15. Emulator in CI: `scripts/firestore-emulator.sh` installs nothing; it runs `gcloud emulators firestore start --host-port=127.0.0.1:8085 --quiet &` and polls `http://127.0.0.1:8085/` until it answers (60 s cap). In `ci.yml` `rust` job, before `cargo test`, add steps: `actions/setup-java` (Temurin 21) and `google-github-actions/setup-gcloud` with `install_components: cloud-firestore-emulator,beta`, both pinned to full commit SHAs like the existing actions; then run the script and export `FIRESTORE_EMULATOR_HOST=127.0.0.1:8085`. CI already runs `cargo test --all-features`, which turns on `firestore-emulator`.
16. Contract test: `make` builds a `FirestoreStore` against the emulator with a fresh `project_id` per call (`test-` plus a counter and the process ID), so each case starts empty without deleting anything.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| INV-2 | Every TTL field is written as a Firestore timestamp, so the TTL backstop can fire |
| RL-1 | Rate-limit counters carry `expires_at` and `expires_by` deletes old windows |
| CFG-1 | `config/classifiers` is stored with only its three fields |

## Tests that must pass

- `server_store_contract_firestore_emulator` (contract, `adapters-gcp/tests/firestore_contract.rs`, feature `firestore-emulator`): the whole T-202a suite passes.
- `inv_2_ttl_fields_are_timestamp_values` (unit on `value.rs`: every `_at` key becomes `timestampValue`; a record round-trips unchanged).
- `rl_1_rate_limit_hit_increments_atomically` (emulator: 20 concurrent `hit`s return 1 to 20 with no duplicates).
- `cfg_1_config_document_fields_on_the_wire` (unit: the commit body for `put_classifiers` has exactly three fields).
- `firestore_value_round_trip_property` (property, `proptest` over JSON trees of null, bool, ints, floats, strings, arrays, maps).
- `firestore_conditional_put_conflict_is_precondition_failed` (emulator).
- `gcp_http_refuses_non_platform_host` (unit).
- `metadata_token_cached_until_expiry` (unit with a local `axum` stub standing in for the metadata server, via a test-only constructor that takes its URL, and `VirtualClock`).

## Edge cases and traps

- Firestore TTL only works on `timestampValue` fields. A `stringValue` date silently disables TTL; the `_at` rule exists for this.
- `integerValue` is a JSON string on the wire, both ways.
- `updateTime` has nanosecond precision; keep the exact string as `Version`, never reformat it.
- Do not send `updateMask` on a normal `put`, or removed optional fields would survive in the stored document.
- Never build JSON for queries by string formatting; use `serde_json::json!`.
- Do not log document names, field values or cursors: they hold user IDs and ciphertext. Log the operation and status only (T-307 fields).
- Do not use the Firestore emulator's `Authorization: Bearer owner` outside the emulator path.
- `--all-features` in CI turns the emulator feature on. If T-004's coverage job also uses `--all-features`, it needs the same emulator steps; tell the reviewer.
- Do not add a gRPC client crate; REST through `reqwest` keeps the dependency and licence surface small. Check `cargo deny check licenses` stays green (no `aws-lc-sys`, whose licence includes OpenSSL terms not on the allowlist).
- `PLATFORM_HOSTS` is not the `HttpEgress` allowlist (T-306): user-influenced URLs never reach `GcpHttp`.

## Out of scope

- KMS, Cloud Tasks and Secret Manager adapters (T-302, T-304, T-305), which reuse `GcpHttp` and `TokenSource`.
- Terraform for the database, TTL policies and indexes (T-1102).
- Sweeper logic (T-706).

## Done when

- The tests above pass and every required check is green (S10 10.1), with the emulator running in the `rust` job.
- Definition of done in S10 10.4.
- `firestore-indexes.json` lists an index for every query that combines an equality filter with an order on another field.
