# T-305: Secret Manager and the classifier config document

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | sonnet | about 300 lines of code plus tests | T-202b, T-301 |

**Read only these spec sections:** S4 5.7 bullets "Jev API key" and "Kill switches" (`docs/specs/S4-architecture.md`); S5 "Keys and secrets held outside Firestore" table and the `config/classifiers` row, CFG-1 (`docs/specs/S5-data-inventory.md`); S2 CL-04 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 5.13 API-ADM-9 (`docs/specs/S7-api-contract.md`); S10 9.2 row JEV-1 (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-201a-port-traits.md` ("secrets.rs") and `docs/backlog/T-201b-server-store-traits.md` (`ClassifiersConfig`, `ConfigRepo`). Nothing else is needed.

## Goal

`adapters-gcp` gains `SecretManagerSecrets`, the production `Secrets` port that reads the four secrets from Secret Manager once and keeps them in memory, and `ClassifierSwitches`, which combines the start-up environment flags with the `config/classifiers` Firestore document, re-read at most once a minute, so an admin can turn Gemini or Jev off without a deploy and calls stop within one check (CL-04 AC1, CFG-1).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gcp/src/secrets.rs` | `SecretManagerSecrets`, `SecretsConfig` |
| Create | `backend/crates/adapters-gcp/src/classifier_switches.rs` | `ClassifierSwitches`, `BakeoffModel`, `EnvSwitches` |
| Create | `backend/crates/adapters-gcp/tests/secrets_rest.rs` | against a local `axum` stub of Secret Manager |
| Create | `backend/crates/adapters-gcp/tests/classifier_switches.rs` | with `InMemoryServerStore` and `VirtualClock` |

## Types and signatures

```rust
// secrets.rs
pub struct SecretsConfig { pub project_id: String }      // secret IDs come from SecretName::secret_id()
pub struct SecretManagerSecrets { http: Arc<GcpHttp>, cfg: SecretsConfig, base: Url, cache: Mutex<HashMap<SecretName, obs::Sensitive<Vec<u8>>>> }
impl SecretManagerSecrets {
    pub fn new(http: Arc<GcpHttp>, cfg: SecretsConfig) -> Self;          // base https://secretmanager.googleapis.com/v1/
    /// Load the secrets this service needs at start-up; fail start-up if any is missing.
    pub async fn preload(&self, names: &[SecretName]) -> Result<(), SecretError>;
}
impl Secrets for SecretManagerSecrets {}

// classifier_switches.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum BakeoffModel { Gemini, Jev }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvSwitches { pub gemini: bool, pub jev: bool }
impl EnvSwitches {
    /// CLASSIFIER_GEMINI_ENABLED and CLASSIFIER_JEV_ENABLED: "true" or "1" is on; unset or anything else is off.
    pub fn from_env() -> Self;
}
pub const CONFIG_REFRESH: time::Duration = time::Duration::seconds(60); // S4 5.7 "checked every minute"
pub struct ClassifierSwitches { env: EnvSwitches, store: Arc<dyn ServerStore>, clock: Arc<dyn Clock>,
    state: tokio::sync::Mutex<Option<(SwitchState, OffsetDateTime)>> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct SwitchState { pub gemini: bool, pub jev: bool }
impl ClassifierSwitches {
    pub fn new(env: EnvSwitches, store: Arc<dyn ServerStore>, clock: Arc<dyn Clock>) -> Self;
    /// Effective switch: env flag AND document flag, refreshed when the cached read is CONFIG_REFRESH old.
    pub async fn enabled(&self, model: BakeoffModel) -> bool;
    pub async fn snapshot(&self) -> SwitchState;
    /// Admin write (API-ADM-9 in T-906 calls it after step-up). Writes only the three allowed fields
    /// and refreshes this instance's cache immediately.
    pub async fn set(&self, model: BakeoffModel, enabled: bool) -> Result<SwitchState, StoreError>;
}
```

## Algorithm

1. `SecretManagerSecrets::get(name)`: cache hit returns a clone. Miss: `GET {base}projects/{project}/secrets/{name.secret_id()}/versions/latest:access`; response `{"name", "payload": {"data": "<base64>", "dataCrc32c": "<decimal>"}}`; decode, check CRC32C when present (mismatch gives `Unavailable`), store and return. HTTP 404 gives `Missing`, 403 gives `Denied`, 429 and 5xx give `Unavailable`.
2. Rotation `[DEFAULT]`: values are read once per process. Rotating a secret means adding a version and restarting the services (Cloud Run revision); no background refresh. For the two HMAC keys, which rotate yearly (S5), the old version is still needed to find old records (the email lookup HMAC for invites, the log pseudonym for `classifier_eval` opt-out deletes); that multi-version read is out of scope here and is reported as a gap.
3. Each service preloads only the secrets it is allowed (S4 2): `api` all four; `unsub` and `worker` `GoogleOAuthClientSecret` and `LogPseudonymHmacKey`. IAM enforces it; preload makes a missing grant fail fast at start-up.
4. `ClassifierSwitches::enabled(model)`:
   1. If the env flag for `model` is off, return `false` without reading Firestore (JEV-1: start-up switch off means zero calls).
   2. Lock `state`. If empty or `now - fetched_at >= CONFIG_REFRESH`, read `config().get_classifiers()`:
      - `Ok(Some(doc))`: state = the document's flags.
      - `Ok(None)` `[DEFAULT]`: state = both `true` (no document yet means the admin has not switched anything off; the env flags still govern).
      - `Err(_)` `[DEFAULT]`: state = both `false` (fail closed: an optional experiment stops rather than runs blind, ASVS V16.5.3).
      Set `fetched_at = now`.
   3. Return env flag AND state flag.
5. `set(model, enabled)`: read the current document (or the defaults), change one flag, `updated_at = clock.now()`, `put_classifiers` with `Matches(version)` (or `MustNotExist` when absent); on `PreconditionFailed` re-read and retry up to 3 times, then return the error. Update the cache with the written state and `fetched_at = now`.
6. Never sleep or spawn timers: refresh is lazy on read, driven by the `Clock`, so tests advance the virtual clock instead of waiting.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-04 AC1 | An admin switch-off reaches every instance within one refresh (60 seconds), without a deploy |
| CFG-1 | The config document holds only `gemini_enabled`, `jev_enabled`, `updated_at`; a switch turned off stops that model within one check |

## Tests that must pass

- `cl_04_ac1_switch_off_seen_within_one_refresh` (unit: two `ClassifierSwitches` on one store; instance A sets Jev off; B still says on until the clock advances 60 s, then off; Gemini unaffected).
- `cfg_1_switch_write_holds_only_allowed_fields` (unit: `export_json` of the store shows exactly three keys).
- `cfg_1_env_off_never_reads_store` (unit: `fail_next` on the store; `enabled` still returns false without error and without consuming the failure).
- `classifier_switches_missing_document_defaults_on` and `classifier_switches_store_error_fails_closed` (unit).
- `classifier_switches_set_retries_on_conflict` (unit).
- `secrets_access_decodes_and_checks_crc32c` (stub).
- `secrets_cached_after_first_read` (stub: one request for two `get`s).
- `secrets_404_is_missing_403_is_denied` (stub).
- `secrets_preload_fails_fast_on_missing` (stub).

## Edge cases and traps

- Secret values are `Sensitive<Vec<u8>>`; never log them, their length or their CRC.
- Do not read secrets from environment variables or files as a fallback; Secret Manager is the one source (ASVS V13.3.1).
- `Ok(None)` and `Err` are different on purpose: missing document means defaults on, read error means off.
- A `tokio::sync::Mutex` is fine here because the lock is held across the store read on purpose (one refresh at a time per instance).
- The env parse is strict: `"TRUE "` with a space is off. Document this in the `from_env` doc comment.
- Calls go through `GcpHttp` (platform hosts), not `HttpEgress`.

## Out of scope

- The admin route API-ADM-9, its step-up and security log entry, and wiring switches into the Feed (T-906).
- Terraform for secrets and IAM (T-1102).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
