# T-908b: Bake-off endpoints, CSV and snapshots

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 400 lines of code plus tests | T-504, T-906a, T-908a |

**Read only these spec sections:** S7 5.13 API-ADM-10 (query and `409 versions_mixed` bullets), the "CSV" paragraph, API-ADM-11 to API-ADM-14 (`docs/specs/S7-api-contract.md`), S7 4 rows `versions_mixed`, `snapshot_limit`, `not_acceptable`, S7 6 rows "Bake-off report", "Bake-off snapshots saved", "Bake-off snapshots deleted", `BakeoffQuery`, `BakeoffReport`, `SnapshotSummary`, `Snapshot` schemas in `docs/specs/S7-api-contract.openapi.yaml`, S5 row `bakeoff_snapshots/{id}` and test EXP-3 (`docs/specs/S5-data-inventory.md`), S2 CL-04 AC6 to AC8 (`docs/specs/S2-v1-acceptance-criteria.md`), S10 9.2 row EXP-3 and S10 9.4 row STAT-7 and "Also covered" (`docs/specs/S10-test-strategy.md`), ASVS register rows V1.2.1, V3.2.1, V5.4.1, V8.2.1 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

Split note: the second half of the index's T-908; T-908a builds the report.

## Goal

Admins get the live bake-off report (JSON or CSV), can save it as a snapshot that survives the 180-day expiry and opt-outs, list snapshots, open one (JSON or CSV) and delete one. Saving and deleting need step-up and are logged. The CSV is one tidy table with exactly the JSON's numbers and suppression, a fixed file name and formula-safe cells.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/admin_bakeoff.rs` | API-ADM-10 to API-ADM-14 |
| Create | `backend/crates/api/src/bakeoff/load.rs` | `load_rows` (records to `EvalRow`) |
| Create | `backend/crates/api/src/bakeoff/csv.rs` | `report_to_csv`, `csv_cell` |
| Change | `backend/crates/api/src/routes/mod.rs` | Register: GETs `admin`; POST and DELETE `admin, step-up` |
| Create | `backend/crates/api/tests/admin_bakeoff.rs` | Service integration tests |

## Types and signatures

```rust
// api/src/bakeoff/load.rs
/// Pages through classifier_eval().range(from 00:00 UTC, to + 1 day) and converts each record.
/// Participant indexes are assigned in order of first appearance of each user_pseudo_id; the map is dropped on return.
pub async fn load_rows(app: &AppState, q: &ReportQuery) -> Result<Vec<EvalRow>, ApiError>;

// api/src/bakeoff/csv.rs
pub const CSV_HEADER: &str = "section,model,segment,metric,value,lower,upper,n";
pub const REPORT_FILENAME: &str = "bakeoff-report.csv";
pub const SNAPSHOT_FILENAME: &str = "bakeoff-snapshot.csv";
pub fn report_to_csv(r: &BakeoffReport) -> String;
/// Formula-safe CSV cell: quote when needed (comma, quote, CR, LF), double inner quotes, and prefix "'"
/// when the text starts with '=', '+', '-', '@', tab or CR (S7 5.13). Numbers are written with `{}` (Rust shortest round-trip).
pub fn csv_cell(s: &str) -> String;

// api/src/routes/admin_bakeoff.rs
pub const SNAPSHOT_LIMIT: u64 = 50;              // S7 5.13 [TUNABLE]
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct SaveSnapshot { pub name: String, pub query: ReportQuery }   // name 1 to 100 chars, plain text
#[derive(Serialize)] pub struct SnapshotDto { pub snapshot_id: Uuid, pub name: String, pub created_at: OffsetDateTime,
    pub labelled_swipes: Option<u64>, pub report: BakeoffReport }
#[derive(Serialize)] pub struct SnapshotSummaryDto { pub snapshot_id: Uuid, pub name: String, pub created_at: OffsetDateTime, pub labelled_swipes: Option<u64> }

pub async fn get_report(..);       // GET /admin/bakeoff            (ADM-10)
pub async fn save_snapshot(..);    // POST /admin/bakeoff/snapshots (ADM-11) 201
pub async fn get_snapshot(..);     // GET /admin/bakeoff/snapshots/{id} (ADM-12)
pub async fn list_snapshots(..);   // GET /admin/bakeoff/snapshots  (ADM-13)
pub async fn delete_snapshot(..);  // DELETE /admin/bakeoff/snapshots/{id} (ADM-14) 204
```

`BakeoffSnapshotRecord` (`query_json`, `report_json` as strings) and `BakeoffSnapshotRepo::{list, count}` from T-201b; `build_report`, `ReportQuery`, `BakeoffReport`, `default_prices` from T-908a.

## Algorithm

Content negotiation (ADM-10, ADM-12): `Accept` containing `text/csv` gives CSV; missing, `*/*` or `application/json` gives JSON; anything else `406 not_acceptable`.

ADM-10:

1. Parse the query (`from`, `to` required dates; optional versions; `pool_versions`, default false). `from > to` or a range over 366 days `[DEFAULT]`: `400 invalid_request`.
2. Rate limit 30 per hour per admin (instance memory, T-500 limiter).
3. `rows = load_rows`; `build_report(&rows, &q, &default_prices())`. `VersionsMixed` gives `409 versions_mixed` with `versions_present` in the problem body.
4. JSON: `200` with the report. CSV: `200`, `Content-Type: text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="bakeoff-report.csv"`, body `report_to_csv`.

ADM-11:

1. Admin plus step-up (extractors); rate limit 20 per day per admin (Firestore counter).
2. Validate `name`: 1 to 100 characters after T-602c's `plain_text` cleaning; empty after cleaning is `400`.
3. `bakeoff_snapshots().count() >= SNAPSHOT_LIMIT`: `409 snapshot_limit`.
4. Build the report exactly as ADM-10 (same `409 versions_mixed`). It is already suppressed.
5. Store `BakeoffSnapshotRecord { snapshot_id: new v4, name, created_at: now, labelled_swipes, query_json, report_json }` with `MustNotExist`.
6. Security event `bakeoff_snapshot_saved` (snapshot ID, pseudonymous admin ID). Return `201` with `SnapshotDto`.

ADM-12: load; missing `404`; JSON returns `SnapshotDto` from the stored strings (never recomputed); CSV uses `report_to_csv` on the stored report with filename `bakeoff-snapshot.csv`. Same rate limit as ADM-10 for CSV.

ADM-13: `list(PageRequest)` newest first; sealed cursor (`TokenType::Cursor`) as in T-705; summaries only.

ADM-14: admin plus step-up; rate limit 50 per day; delete (missing is `404`); security event `bakeoff_snapshot_deleted`; `204`.

CSV rows (`report_to_csv`), in this order, one row per number, suppressed values as empty cells:

| section | model | segment | metric | value, lower, upper, n |
| --- | --- | --- | --- | --- |
| `summary` | empty | empty | `labelled_swipes`, `participants`, `top_contributor_share` | value (and interval for the share) |
| `models` | method | empty | `cards`, `valid_answers`, `accuracy`, `junk_precision`, `junk_recall`, `junk_f1`, `false_junk_rate`, `ece`, `p50_ms`, `p95_ms`, `timeout_rate`, `error_rate`, `input_tokens`, `cost_per_1000_messages_usd` | rates with lower, upper, n; plain numbers in value only |
| `calibration` | method | `bin=<from>-<to>` | `count`, `observed_accuracy` | value |
| `latency` | method | `bin=<from_ms>-<to_ms or inf>` | `count` | value |
| `paired` | `a\|b` | empty | `n_paired`, `only_a_correct`, `only_b_correct`, `both_correct`, `neither_correct`, `mcnemar_exact_p`, `accuracy_difference` | value (interval for the difference) |
| `agreement` | `a\|b` | empty | `rate` | value, lower, upper, n |
| `by_segment` | method | `<dimension>=<value>` | `accuracy`, `false_junk_rate` | value, lower, upper, n |
| `confusion` | method | `class=<class>,label=<label>` | `count` | value |
| `trend` | method | `date=<YYYY-MM-DD>` | `accuracy` | value, lower, upper, n |

Every text cell goes through `csv_cell`; the segment cell contains a comma for confusion rows and is therefore quoted.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-04 AC6 | A snapshot holds no pseudonymous IDs, survives expiry and opt-outs, and stays until an admin deletes it |
| CL-04 AC7 | Report and snapshot download as one CSV with the same figures and suppression as JSON |
| CL-04 AC8 | Saving and deleting a snapshot need step-up |
| STAT-7 | Every CSV number equals the JSON value with the same rows suppressed; the file name is fixed ASCII |
| EXP-3 | A snapshot holds no pseudonymous ID, eval ID, corpus value or small cell, and is not recomputed after opt-out or account deletion |
| V1.2.1 | CSV cells are formula-escaped |
| V3.2.1 | CSV is served as an attachment |
| V5.4.1 | The CSV file name is server-fixed, never from query parameters |
| V8.2.1 | ADM-10 to ADM-14 return 403 to non-admins |

## Tests that must pass

- `stat_7_csv_matches_json` (service integration: for one fixed data set, parse the CSV and compare every number to the JSON, for ADM-10 and ADM-12, including suppressed rows as empty cells)
- `stat_7_csv_filename_fixed` (service integration: query values with quotes, CR, LF, `../` and non-ASCII never appear in headers; filename is exactly `bakeoff-report.csv` or `bakeoff-snapshot.csv`)
- `exp_3_snapshot_holds_no_ids` (service integration: stored `report_json` and `query_json` contain no pseudonymous ID, eval ID, canary, corpus address or count below 10)
- `exp_3_snapshot_unchanged_after_opt_out_and_deletion` (service integration: save, opt a user out, delete another account, read back byte-identical)
- `cl_04_ac6_snapshot_survives_eval_expiry` (service integration: advance 181 days and sweep; snapshot unchanged)
- `cl_04_ac7_snapshot_csv_matches_snapshot_json` (service integration)
- `cl_04_ac8_save_and_delete_need_step_up` (service integration: `step_up_required` for each, nothing written)
- `api_adm_10_versions_mixed_409` (service integration: body lists the versions)
- `api_adm_10_not_acceptable_406` (service integration: `Accept: application/xml`)
- `api_adm_11_snapshot_limit_409` (service integration: 50 existing)
- `api_adm_13_lists_newest_first` (service integration)
- `asvs_v1_2_1_csv_cells_formula_escaped` (unit on `csv_cell`: `=SUM(A1)` becomes `'=SUM(A1)`; `-1` text becomes `'-1`; `a,b` becomes `"a,b"`; `say "hi"` becomes `"say ""hi"""`; numbers written as numbers are not text cells and are not prefixed)
- `asvs_v3_2_1_csv_served_as_attachment` (service integration)
- `asvs_v5_4_1_csv_filename_not_from_input` (service integration)
- `asvs_v8_2_1_bakeoff_routes_refused_for_non_admin` (service integration: all five routes, 403, security event)
- `bakeoff_report_aggregates_only` (service integration: response JSON has no array whose length equals the number of records)

## Edge cases and traps

- Negative numbers (an accuracy difference) are numeric cells: write them unquoted and unprefixed; only text cells get the `'` prefix. A test must show both.
- Never build the file name from the snapshot name or query.
- Snapshots are stored as the suppressed report; never store rows, participant maps or pseudonymous IDs, and never recompute a stored snapshot.
- The participant index map in `load_rows` lives only for the request.
- Rate-limit hits are security events (T-500 limiter does this).
- `labelled_swipes` is itself suppressed below the minimum, so it is `Option` in the DTOs.

## Out of scope

- The admin screen and download button: T-1007. Report maths: T-907, T-908a.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
