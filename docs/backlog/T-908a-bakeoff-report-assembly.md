# T-908a: Bake-off report assembly

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 500 lines of code plus about 350 lines of tests | T-906a, T-907 |

**Read only these spec sections:** S7 5.13 API-ADM-10 (query parameters, response shape and every "Rules for every section" bullet) (`docs/specs/S7-api-contract.md`), `BakeoffReport` schema in `docs/specs/S7-api-contract.openapi.yaml`, S4 5.6 third paragraph (`docs/specs/S4-architecture.md`), CR-01a G1 to G5 and G7 (`docs/change-requests/CR-01a-bakeoff-publishable-stats.md`), S2 CL-04 AC2 to AC5 (`docs/specs/S2-v1-acceptance-criteria.md`), S10 9.4 rows STAT-1, STAT-5, STAT-6, STAT-8 and the "Also covered" bullets (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

Split note: the index's T-908 "Bake-off report, CSV and snapshots" is split into T-908a (this file: pure report assembly in `domain`) and T-908b (endpoints, CSV and snapshots).

## Goal

A pure function turns a list of evaluation rows and a query into the complete, already suppressed ADM-10 report: per-method figures for header rules, Gemini and Jev with Wilson intervals, the paired comparisons, agreement, confusion, calibration, latency, cost, segments and the daily trend. It refuses to pool different question or input versions unless asked. No per-record value can leave it.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/bakeoff/report.rs` | `EvalRow`, `ReportQuery`, `BakeoffReport`, `build_report` |
| Create | `backend/crates/domain/src/bakeoff/prices.rs` | `PriceTable`, `price_for` |
| Change | `backend/crates/domain/src/bakeoff/mod.rs` | `pub mod report; pub mod prices;` |
| Create | `backend/crates/domain/tests/bakeoff_report.rs` | Fixed-stream tests |

## Types and signatures

```rust
// bakeoff/report.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)] #[serde(rename_all = "snake_case")]
pub enum Method { HeaderRules, Gemini, Jev }
pub const METHODS: [Method; 3] = [Method::HeaderRules, Method::Gemini, Method::Jev];
pub const GROUND_TRUTH_VERSION: &str = "1";   // label mapping version (S7 5.13)

#[derive(Clone, Debug)]
pub struct Prediction { pub class: Option<MessageClass>, pub score: Option<u8>, pub confidence: Option<f64>,
    pub latency_ms: u32, pub input_tokens: Option<u32>, pub error: Option<PredictionError>, pub classifier_id: String }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum PredictionError { Timeout, Other }

/// Domain mirror of one classifier_eval record (T-908b converts from the ports record).
/// `participant` is an opaque index assigned by the caller, never the pseudonymous ID itself.
#[derive(Clone, Debug)]
pub struct EvalRow {
    pub participant: u32, pub created_at: OffsetDateTime,
    pub header_rules: (MessageClass, u8),
    pub gemini: Option<Prediction>, pub jev: Option<Prediction>,   // None: model not called (switched off)
    pub direction: SwipeDirection, pub undone: bool,
    pub segments: Segments,
    pub input_version: String, pub question_version: String, pub price_version: String,
}
#[derive(Clone, Debug)]
pub struct Segments { pub facts: [(&'static str, bool); 8],          // EvalHeaderFacts names and values
    pub provider: &'static str, pub age_bucket: &'static str, pub text_tokens_bucket: &'static str, pub lang_is_english: bool }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReportQuery { pub from: time::Date, pub to: time::Date, pub input_version: Option<String>,
    pub question_version: Option<String>, pub price_version: Option<String>, #[serde(default)] pub pool_versions: bool }

#[derive(Debug, PartialEq)]
pub enum ReportError { VersionsMixed { input: Vec<String>, question: Vec<String> } }

/// The ADM-10 body, field names exactly as S7 5.13. Every count below min_cell_size is None.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BakeoffReport { pub query: ReportQuery, pub versions_present: VersionsPresent, pub min_cell_size: u64,
    pub ground_truth_version: &'static str, pub interval_method: &'static str /* "wilson_95" */,
    pub labelled_swipes: Option<u64>, pub participants: Participants, pub models: Vec<MethodReport>,
    pub paired: Vec<PairedReport>, pub agreement: Vec<AgreementReport>, pub by_segment: Vec<SegmentReport>,
    pub confusion: Vec<ConfusionCell>, pub trend: Vec<TrendPoint> }
// MethodReport, PairedReport, AgreementReport, SegmentReport, ConfusionCell, TrendPoint, Participants,
// VersionsPresent: one struct per S7 5.13 JSON object, same field names, every count Option<u64>,
// every rate a T-907 `Rate`, `junk_f1: Option<f64>`, `calibration: { ece: Option<f64>, bins: Vec<CalibrationBin> }`,
// `latency: { p50_ms, p95_ms: Option<u32>, histogram: Vec<LatencyBin> }`, `cost_per_1000_messages_usd: Option<f64>`.

pub fn build_report(rows: &[EvalRow], query: &ReportQuery, prices: &PriceTable) -> Result<BakeoffReport, ReportError>;

// bakeoff/prices.rs
pub struct PriceTable { pub versions: BTreeMap<String, ModelPrices> }
pub struct ModelPrices { pub gemini_usd_per_million_input: f64, pub jev_usd_per_million_input: f64 }
/// Price version "1" [TUNABLE]: Jev 0.042 (research/jev-classifier-evaluation.md); Gemini from the Vertex AI
/// price page for the pinned Flash-Lite model on the day this ships. Header rules always 0.
pub fn default_prices() -> PriceTable;
```

## Algorithm

`build_report(rows, query, prices)`:

1. **Filter.** Keep rows with `query.from <= created_at.date() <= query.to` (UTC) and matching any version filter given.
2. **Versions (STAT-6).** Collect the distinct `input_version`, `question_version`, `price_version` into `versions_present`. If more than one input or question version remains and `!pool_versions`: `Err(VersionsMixed)`. `price_version` never blocks pooling.
3. **Labels.** For each row, `label = label_for(direction, undone)` (T-907). Rows with no label (skips) are dropped from every accuracy figure but kept for availability (error and timeout rates). `labelled_swipes` = rows with a label.
4. **Per method** (`models`, CR-01a G1): for header rules the prediction is `header_rules.0` with confidence `score / 100`, latency 0 measured in process, cost 0, never an error. For Gemini and Jev use the row's prediction; rows where the model was not called (`None`) are excluded from that model entirely.
   - `cards` = rows where the method was called; `valid_answers` = called with no error and a class.
   - Accuracy, junk precision, junk recall, F1, false-junk rate from a T-907 `JunkConfusion` over labelled rows with a valid answer; model errors count against availability, never accuracy (STAT-1).
   - `timeout_rate` = timeouts / cards; `error_rate` = other errors / cards (Wilson).
   - Calibration: `(confidence, predicted_label == label)` per labelled valid answer with a confidence; T-907 `calibration`.
   - Latency: answered calls' latencies; `p50_ms`, `p95_ms` by nearest rank; `histogram` by T-907 (STAT-8).
   - `input_tokens` = sum; cost = Σ over valid answers of `input_tokens × price(model, row.price_version) / 1,000,000`, then `cost_per_1000_messages_usd = cost / answered × 1000` (each record priced by its own `price_version`, STAT-6).
5. **Confusion.** Per method, a 5 by 2 table: predicted class by label; one `ConfusionCell` per non-empty combination.
6. **Paired** (each pair of the three methods: header_rules and gemini, header_rules and jev, gemini and jev): rows where both gave a valid answer and there is a label. Counts `only_a_correct`, `only_b_correct`, `both_correct`, `neither_correct`; `n_paired`; `mcnemar_exact_p` on the two discordant counts; `accuracy_difference` from `cluster_bootstrap_diff` with one `ParticipantPair` per participant, `BOOTSTRAP_RESAMPLES`, `BOOTSTRAP_SEED`; `bootstrap = { method, resamples, seed, participants }`.
7. **Agreement**: for each pair, rows where both answered (labelled or not): rate of identical five-way class `[DEFAULT: S7 does not say five-way or junk/wanted; five-way is the stricter reading]`.
8. **Participants**: `count` = distinct participants among labelled rows; `top_contributor_share` from T-907. Both `None` when `labelled_swipes < MIN_CELL_SIZE`.
9. **Segments** (CL-04 AC4): for each method and each dimension (`header_rules_class`, each of the 8 header facts, `provider`, `age_bucket`, `text_tokens_bucket`, `lang_is_english`) and each value present: `accuracy` and `false_junk_rate` with `n`.
10. **Trend**: per UTC day and method, accuracy with Wilson and `n`.
11. **Suppression (STAT-5)**, as the last step, on the finished report:
    - Every count below `MIN_CELL_SIZE` becomes `None`, and every rate built on it becomes `Rate::suppressed()`.
    - Complementary groups for T-907 `suppress_cells`: per method and dimension, the segment values; per method, each confusion row (one class, two labels) and each confusion column (one label, five classes); per pair, the four paired counts; per day, the three methods' trend `n`; per method, the latency histogram bins.
    - A suppressed segment cell suppresses its `accuracy` and `false_junk_rate` together.
12. Return the report. No field holds a row, a participant index or a pseudonymous ID; `participant` indexes are used only inside the function.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-04 AC2 | Accuracy, confusion, calibration, junk precision and recall, false-junk rate, agreement, latency and cost for header rules, Gemini and Jev; aggregates only |
| CL-04 AC3 | Every rate has a 95% interval; paired McNemar and participant-resampled interval; participant count and top share |
| CL-04 AC4 | Breakdown by header-rules class, header facts, provider, age, text length and language, hiding small segments |
| CL-04 AC5 | Different question or input versions are not pooled unless asked |
| STAT-1 | Skips excluded, undo flips, errors count against availability not accuracy, header rules with cost 0 and score/100 confidence |
| STAT-5 | Every segment, trend day and histogram bin below the minimum is suppressed, with complementary suppression |
| STAT-6 | Mixed versions refused unless pooled; a version filter reports only that version; mixed price versions priced separately |
| STAT-8 | Latency bins sum to answered calls; each trend point has `n` and a Wilson interval |

## Tests that must pass

Build rows with a small helper `row(participant, day, hr_class, gemini, jev, direction, undone)`.

- `stat_1_labels_and_rates_end_to_end` (unit: 20 labelled rows reproducing T-907's TP 6, FP 2, FN 3, TN 9 for Jev; 3 skip rows; 2 Jev timeouts. Jev accuracy 0.75 with n 20, timeout rate 2 of 25 cards)
- `stat_1_undo_flips_label` (unit: a left swipe undone counts as wanted)
- `stat_1_header_rules_is_a_method` (unit: present with cost 0 and calibration from score/100)
- `stat_5_segment_suppression` (unit: a segment with 4 rows is null, and its sibling with the next smallest count is null too)
- `stat_5_trend_and_histogram_suppressed` (unit)
- `stat_5_no_value_recoverable_from_totals` (property: random rows; in every complementary group the null count is 0 or at least 2)
- `stat_6_mixed_question_versions_refused` (unit: `VersionsMixed` with both versions listed)
- `stat_6_pool_versions_allowed` (unit)
- `stat_6_filter_reports_one_version` (unit)
- `stat_6_mixed_price_versions_priced_separately` (unit: two price versions with different prices; cost equals the hand sum)
- `stat_8_trend_points_have_n_and_interval` (unit)
- `cl_04_ac2_report_has_three_methods_and_all_sections` (unit)
- `cl_04_ac3_paired_uses_mcnemar_and_cluster_bootstrap` (unit: six participants; `mcnemar_exact_p` and interval match T-907 called directly)
- `cl_04_ac3_insufficient_participants_nulls_interval` (unit: four participants)
- `cl_04_ac4_segments_cover_every_dimension` (unit)
- `cl_04_ac5_versions_not_pooled_by_default` (unit)
- `bakeoff_report_holds_no_row_level_value` (unit: serialise the report; it contains no participant index list, no per-row latency list and no `created_at` timestamps)

## Edge cases and traps

- Suppress last, on the finished numbers; suppressing early and then summing leaks the hidden value.
- Never output the participant index or anything derived from one participant alone except the suppressed top share.
- Group by UTC day for the trend; the query dates are UTC.
- A model that was switched off for some rows (`None`) is absent from those rows' comparisons; it is not an error.
- Division by zero gives `None`, never `NaN` or `inf` in the output.
- Keep `f64` throughout; JSON serialisation of `None` is `null`.
- The price for Gemini must be filled in `default_prices()` from the official page at the time; tests use their own `PriceTable`.

## Out of scope

- Loading records, routes, CSV, snapshots, rate limits: T-908b. Statistics primitives: T-907.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
