# T-902: Experiments consent

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 250 lines of code plus tests | T-500 |

**Read only these spec sections:** S2 CL-02 AC1 to AC3 (`docs/specs/S2-v1-acceptance-criteria.md`), S7 5.13 API-EXP-1 and API-EXP-2, S7 4 rows `consent_outdated` and `experiment_unavailable`, S7 6 row "Experiment opt-in changes" (`docs/specs/S7-api-contract.md`), S5 rows `users/{id}: experiments_consent_version, experiments_opted_in_at` and `classifier_eval/{id}` (`docs/specs/S5-data-inventory.md`), S6 7 "consent changes" (`docs/specs/S6-security.md`), S4 5.8 first bullet (`docs/specs/S4-architecture.md`). Nothing else is needed.

## Goal

A user can read and change their Experiments consent. Opting in records the consent text version; opting out stops model calls at once and deletes the user's `classifier_eval` records before the response returns. The function `consent_is_current(user)` is the consent half of the bake-off gate that the Feed uses (T-901, T-906b).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/experiments.rs` | API-EXP-1, API-EXP-2 |
| Create | `backend/crates/api/src/experiments.rs` | `CURRENT_CONSENT_VERSION`, `consent_is_current`, `bakeoff_gate` |
| Change | `backend/crates/api/src/routes/mod.rs` | Register the two routes (auth `user`) |
| Create | `backend/crates/api/tests/experiments.rs` | Service integration tests |

## Types and signatures

```rust
// api/src/experiments.rs
pub const CURRENT_CONSENT_VERSION: &str = "2026-10-03";   // S7 5.13; bump when the S9 7.8 text changes
pub const OPT_CHANGES_PER_DAY: u32 = 10;                  // S7 6 [TUNABLE]

pub fn consent_is_current(user: &UserRecord) -> bool;     // version == CURRENT and opted_in_at is Some
/// Consent AND switches. `switches` is (gemini_enabled, jev_enabled) from the switch reader
/// (T-906b; until then read `config().get_classifiers()` directly, missing doc = both false).
pub fn bakeoff_gate(user: &UserRecord, switches: (bool, bool)) -> BakeoffGate;

// api/src/routes/experiments.rs
#[derive(Serialize)] pub struct ExperimentsDto { pub classifier_bakeoff: BakeoffConsentDto }
#[derive(Serialize)] pub struct BakeoffConsentDto {
    pub available: bool, pub opted_in: bool, pub consent_version: Option<String>,
    pub current_consent_version: &'static str, pub opted_in_at: Option<OffsetDateTime>,
}
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct PutExperiments { pub classifier_bakeoff: PutBakeoff }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct PutBakeoff { pub opted_in: bool, pub consent_version: Option<String> }

pub async fn get_experiments(State(app): State<AppState>, s: AuthedSession) -> Result<Json<ExperimentsDto>, ApiError>;
pub async fn put_experiments(State(app): State<AppState>, s: AuthedSession, Json(b): Json<PutExperiments>) -> Result<Json<ExperimentsDto>, ApiError>;
```

`UserRecord` and `ClassifierEvalRepo::delete_for_users` come from T-201b; `BakeoffGate` from T-901 (if T-901 is not merged, define it here with the same shape and T-901 imports it). The pseudonymous IDs come from the `obs` pseudonymiser (T-307): every `UserPseudoId` the user has had, one per log key version.

## Algorithm

GET:

1. Load the user. `opted_in = consent_is_current(user)`. If the stored version is not current, report `opted_in: false` (S7: an old version counts as opted out).
2. `available = gemini_enabled || jev_enabled`.
3. Return the DTO.

PUT:

1. Rate limit: `rate_limits().hit("exp_opt:<user_id hash>", day start, 1 day)`; over `OPT_CHANGES_PER_DAY` gives `429 rate_limited`.
2. `opted_in: true`:
   1. `consent_version` must equal `CURRENT_CONSENT_VERSION`, else `409 consent_outdated`.
   2. If both switches are off: `409 experiment_unavailable`.
   3. Write `experiments_consent_version = Some(current)`, `experiments_opted_in_at = Some(now)` with `Precondition::Matches(version)`; retry once on conflict.
   4. Security event `experiments_opt_in` (pseudonymous user ID, consent version).
3. `opted_in: false` (always allowed, any `consent_version`, even while paused):
   1. Clear both fields on the user record first (the gate closes on the next read).
   2. `classifier_eval().delete_for_users(&all_pseudo_ids(user))`; wait for it. On a store error return `503` and leave the consent cleared; a retry deletes the rest.
   3. Security event `experiments_opt_out` (pseudonymous user ID, count deleted).
4. Return the GET body.

Gate freshness: the Feed reads the user record on every request (no cache), so `bakeoff_gate` sees an opt-out on the very next Feed request. A Feed request already in flight may finish its model calls; T-906a refuses to write an eval record when consent is no longer current at swipe time.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-02 AC2 | Without current consent the gate is closed, so no request reaches Gemini or Jev |
| CL-02 AC3 | Opting out stops model calls at once and deletes the user's `classifier_eval` records |
| EXP-4 | Opting in sets `experiments_consent_version` and `experiments_opted_in_at`; opting out clears both |
| V2.4.1 | Opt-in changes are rate limited per user |
| V16.3.3 | Consent changes are written to the security log |

## Tests that must pass

- `exp_4_opt_in_sets_and_opt_out_clears_consent_fields` (service integration)
- `cl_02_ac2_gate_closed_without_consent` (unit on `bakeoff_gate`: no consent, switches on, gate closed)
- `cl_02_ac2_gate_closed_for_outdated_version` (unit)
- `cl_02_ac2_gate_needs_switch_and_consent` (unit, table of the four combinations per model)
- `cl_02_ac3_opt_out_deletes_eval_records_before_response` (service integration: three records for the user, one for another user; after PUT only the other remains)
- `cl_02_ac3_opt_out_closes_gate_immediately` (service integration: next `bakeoff_gate` read is closed)
- `cl_02_ac3_opt_out_works_while_paused` (service integration)
- `api_exp_2_old_consent_version_409` (service integration: `consent_outdated`)
- `api_exp_2_opt_in_while_paused_409` (service integration: `experiment_unavailable`)
- `api_exp_1_available_false_when_both_off` (service integration)
- `api_exp_1_outdated_version_reads_as_opted_out` (service integration)
- `asvs_v2_4_1_experiments_changes_rate_limited` (service integration: eleventh change in a day gets 429)
- `asvs_v16_3_3_consent_change_logged` (service integration: one event per change, no address)
- `api_exp_2_unknown_field_400` (service integration)

## Edge cases and traps

- Opting out must never fail because of the pilot state, the version or the rate limit's absence of a prior opt-in; only the rate limit and a store outage may refuse it.
- Delete records by every pseudonymous ID the user has had (log key rotation), not only the current one.
- Do not delete `bakeoff_snapshots`; they hold aggregates only (S7 5.13).
- Never `unwrap` the user's optional fields; an old record without them means "not consented".
- The consent text itself lives in the app (S9 7.8, BAKE-7 in T-1007); the API carries only the version.

## Out of scope

- The Settings screen and BAKE-7: T-1007. Kill switches: T-906b. Model calls: T-901, T-904, T-905.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
