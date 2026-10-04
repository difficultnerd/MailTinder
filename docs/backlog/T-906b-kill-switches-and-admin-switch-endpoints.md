# T-906b: Kill switches and admin switch endpoints

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 250 lines of code plus tests | T-305, T-504, T-902, T-906a |

**Read only these spec sections:** S2 CL-04 AC1 and AC8, AU-01 AC5 (`docs/specs/S2-v1-acceptance-criteria.md`), S4 5.7 bullet "Kill switches" (`docs/specs/S4-architecture.md`), S5 row `config/classifiers` and test CFG-1 (`docs/specs/S5-data-inventory.md`), S7 5.13 API-ADM-8 and API-ADM-9, S7 3.6, S7 6 row "Experiment admin changes" (`docs/specs/S7-api-contract.md`), S10 9.2 row JEV-1 (`docs/specs/S10-test-strategy.md`), S6 7 "kill switch changes" (`docs/specs/S6-security.md`). Nothing else is needed.

Split note: the index's T-906 is split into T-906a (evaluation records) and this file.

## Goal

An admin can switch Gemini or Jev off (or back on) without a deploy. The effective switch for each model is the start-up environment variable AND the Firestore document `config/classifiers`, re-read at most once a minute. The Feed's bake-off gate uses it together with consent (T-902). The admin reads and changes the switches through API-ADM-8 and API-ADM-9; changes need step-up and are logged. This task also adds the end-to-end JEV-1 test that proves no model call happens without consent or with a switch off.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/classify/switches.rs` | `ClassifierSwitches` (env plus cached document) |
| Create | `backend/crates/api/src/routes/admin_classifier.rs` | API-ADM-8, API-ADM-9 |
| Change | `backend/crates/api/src/routes/mod.rs` | Register with auth `admin` (GET) and `admin, step-up` (PATCH) |
| Change | `backend/crates/api/src/services/feed.rs` | Gate = `bakeoff_gate(user, switches.current())` |
| Change | `backend/crates/api/src/experiments.rs` (T-902) | Use `ClassifierSwitches` instead of a direct document read |
| Create | `backend/crates/api/tests/kill_switches.rs` | Service integration tests |
| Create | `backend/crates/api/tests/jev_1_no_model_call.rs` | JEV-1 with `fake-jev` and `fake-vertex` |

## Types and signatures

```rust
// api/src/classify/switches.rs
pub const SWITCH_REFRESH: time::Duration = time::Duration::seconds(60);   // S4 5.7 "checked every minute"
pub const SWITCH_STALE_MAX: time::Duration = time::Duration::minutes(5);  // [DEFAULT] after this without a good read, fail closed

pub struct EnvSwitches { pub gemini: bool, pub jev: bool }               // CLASSIFIER_GEMINI_ENABLED, CLASSIFIER_JEV_ENABLED; "true" only, anything else false
impl EnvSwitches { pub fn from_env() -> Self; }

pub struct ClassifierSwitches { env: EnvSwitches, cache: tokio::sync::Mutex<Cached> /* doc value, read_at, last_ok_at */ }
impl ClassifierSwitches {
    pub fn new(env: EnvSwitches) -> Self;
    /// Effective (gemini, jev). Re-reads config/classifiers through T-305's reader when the cached
    /// value is older than SWITCH_REFRESH (Clock based). Missing document: (false, false).
    pub async fn current(&self, ports: &Ports) -> (bool, bool);
    pub fn invalidate(&self);   // called after ADM-9 writes, so this instance sees the change at once
}

// api/src/routes/admin_classifier.rs
#[derive(Serialize)] pub struct ModelSwitchDto { pub model: &'static str, pub classifier_id: String, pub enabled: bool, pub changed_at: Option<OffsetDateTime> }
#[derive(Serialize)] pub struct AdminClassifierDto { pub models: Vec<ModelSwitchDto>, pub participants: u64 }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct PatchClassifier { pub models: Vec<PatchModel> }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct PatchModel { pub model: ModelName, pub enabled: bool }
#[derive(Deserialize, Clone, Copy)] #[serde(rename_all = "snake_case")] pub enum ModelName { Gemini, Jev }

pub async fn get_classifier(State(app): State<AppState>, s: AdminSession) -> Result<Json<AdminClassifierDto>, ApiError>;
pub async fn patch_classifier(State(app): State<AppState>, s: SteppedUpAdmin, Json(b): Json<PatchClassifier>) -> Result<Json<AdminClassifierDto>, ApiError>;
```

`ClassifiersConfig` and `ConfigRepo::{get_classifiers, put_classifiers}` come from T-201b; the cached reader from T-305 (use it inside `current` if it already caches; do not cache twice). `AdminSession` and `SteppedUpAdmin` extractors come from T-501 and T-504.

## Algorithm

`current`:

1. If the cache is younger than `SWITCH_REFRESH`, use it.
2. Else read the document. Success: cache it with `last_ok_at = now`. Failure: keep the old value if `now - last_ok_at < SWITCH_STALE_MAX`, else treat as `(false, false)` and log `classifier_config_unavailable`.
3. Return `(env.gemini && doc.gemini_enabled, env.jev && doc.jev_enabled)`.

API-ADM-8 (GET): `models` lists Gemini then Jev with `classifier_id` from the built `ClassifierSet` (`"gemini@<model>"`, `"jev@1.13.0"`), `enabled` = effective value, `changed_at` = the document's `updated_at` (the document has one timestamp for both, CFG-1). `participants` = number of users with `consent_is_current` (page through `users().list`; trial scale is small `[DEFAULT]`).

API-ADM-9 (PATCH):

1. Admin and step-up are checked by the extractors (AU-01 AC5, CL-04 AC8); a missing step-up gives `403 step_up_required`, a non-admin `403 forbidden` plus a security event.
2. Rate limit 50 per day per admin (`rate_limits().hit`); over gives `429`.
3. Empty `models` or a duplicate model: `400 invalid_request`.
4. Read the document (missing: start from both false), apply each change, set `updated_at = now`, write with `Precondition::Matches(version)` (or `MustNotExist` when missing); on `PreconditionFailed` re-read and re-apply once.
5. The written document has exactly `gemini_enabled`, `jev_enabled`, `updated_at` (CFG-1).
6. `invalidate()` the local cache; other instances pick it up within `SWITCH_REFRESH`.
7. Security event `kill_switch_changed` per changed model (model name, new value, pseudonymous admin ID).
8. Return the ADM-8 body.

JEV-1 test set-up: run `api` in process with `GeminiClassifier` pointed at `fake-vertex` and `JevClassifier` at `fake-jev`, a test `HttpEgress` that fails the test if a model host is called while the scenario expects zero calls, and the corpus mailbox. Scenarios, each triaging the whole corpus Feed:

1. User without consent, both switches on: zero requests at both fakes.
2. Consenting user, `CLASSIFIER_JEV_ENABLED=false` at start-up: zero Jev requests, Gemini requests present.
3. Consenting user, Jev on; mid-run an admin flips `jev_enabled` to false in the document from another `api` instance (so no `invalidate` on this one); after advancing the clock 60 s, no further Jev requests.
4. Consenting user opts out mid-session: no requests to either model after the opt-out response.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-04 AC1 | An admin switches a model off without a deploy; the change applies within a minute |
| CL-04 AC8 | Changing a kill switch needs step-up |
| AU-01 AC5 | The kill switch write is refused without a fresh Google sign-in |
| JEV-1 | No request reaches Gemini or Jev without consent, with a switch off, or after opt-out |
| CFG-1 | `config/classifiers` holds only the allowed fields and only an admin write with step-up changes it |
| CL-02 AC2 | Without consent no request reaches Gemini or Jev (end to end) |
| V8.2.1 | ADM-8 and ADM-9 are refused to non-admins |
| V16.3.3 | Kill switch changes are logged |

## Tests that must pass

- `jev_1_no_model_call_without_consent` (service integration, scenario 1)
- `jev_1_env_kill_switch_stops_model` (service integration, scenario 2)
- `jev_1_config_flip_stops_model_after_next_check` (service integration, scenario 3)
- `jev_1_consent_withdrawn_mid_session` (service integration, scenario 4)
- `cl_02_ac2_no_model_call_without_consent_end_to_end` (same run as scenario 1, asserting through the egress guard)
- `cl_04_ac1_switch_off_without_deploy` (service integration: PATCH then next Feed request on the same instance makes no call)
- `cl_04_ac8_kill_switch_needs_step_up` (service integration: `step_up_required`, document unchanged)
- `au_01_ac5_kill_switch_refused_without_step_up` (service integration: `auth_time` 5 min 1 s old)
- `cfg_1_config_doc_holds_only_allowed_fields` (service integration: stored JSON key set is exactly the three names)
- `cfg_1_only_admin_with_step_up_changes_switches` (service integration)
- `asvs_v8_2_1_admin_classifier_refused_for_non_admin` (service integration: GET and PATCH)
- `asvs_v16_3_3_kill_switch_change_logged` (service integration)
- `switches_fail_closed_after_stale_max` (unit: store errors for 5 minutes give `(false, false)`)
- `switches_missing_doc_is_off` (unit)

## Edge cases and traps

- Env values: only the exact string `true` enables; unset, `1` or `TRUE` mean off `[DEFAULT: fail closed]`.
- The effective switch is env AND document. The document can never turn on a model the environment turned off.
- Do not read the document on every card; the cache is per instance and Clock based.
- `changed_at` is shared by both models because CFG-1 forbids per-model timestamps; do not add fields.
- A PATCH that sets a model to its current value still writes `updated_at` and logs, so the admin sees an effect.

## Out of scope

- The Experiments consent route: T-902. The admin screen: T-1007. Evaluation records: T-906a.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
