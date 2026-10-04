# T-907: Bake-off statistics library

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 450 lines of code plus about 450 lines of tests | T-101 |

**Read only these spec sections:** CR-01a G2 and G3 (`docs/change-requests/CR-01a-bakeoff-publishable-stats.md`), S7 5.13 API-ADM-10 "Rules for every section" bullets Labels, Intervals, Paired comparison, Participants, Latency histogram, Suppression, Complementary suppression (`docs/specs/S7-api-contract.md`), S4 5.6 paragraph "Label mapping" (`docs/specs/S4-architecture.md`), S10 9.4 rows STAT-1 to STAT-5 and STAT-8, S10 9.2 row BAKE-6 (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

## Goal

A pure, dependency-free statistics module in `domain` that the bake-off report (T-908a) builds on: the swipe-to-label mapping, the junk confusion counts and rates (accuracy, precision, recall, F1, false-junk rate), Wilson 95% intervals, McNemar's exact test, a seeded cluster bootstrap by participant, expected calibration error with fixed bins, nearest-rank percentiles, the fixed latency histogram, the largest-contributor share, and small-cell suppression including complementary suppression. Every function is deterministic and is tested against the known values below.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/bakeoff/mod.rs` | `pub mod labels; pub mod rates; pub mod paired; pub mod calibration; pub mod latency; pub mod suppress; pub mod rng;` and constants |
| Create | `backend/crates/domain/src/bakeoff/labels.rs` | `Label`, `SwipeDirection`, `label_for`, `predicted_label` |
| Create | `backend/crates/domain/src/bakeoff/rates.rs` | `Rate`, `wilson`, `JunkConfusion` |
| Create | `backend/crates/domain/src/bakeoff/paired.rs` | `mcnemar_exact_p`, `cluster_bootstrap_diff`, `top_contributor_share` |
| Create | `backend/crates/domain/src/bakeoff/rng.rs` | `SplitMix64` |
| Create | `backend/crates/domain/src/bakeoff/calibration.rs` | `calibration` (bins and ECE) |
| Create | `backend/crates/domain/src/bakeoff/latency.rs` | `nearest_rank`, `latency_histogram` |
| Create | `backend/crates/domain/src/bakeoff/suppress.rs` | `suppress_cells` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod bakeoff;` |
| Create | `backend/crates/domain/tests/bakeoff_stats.rs` | Known-value and property tests |

## Types and signatures

```rust
// bakeoff/mod.rs
pub const Z_95: f64 = 1.959963984540054;      // two-sided 95% normal quantile
pub const MIN_CELL_SIZE: u64 = 10;            // S7 5.13 [TUNABLE]
pub const BOOTSTRAP_RESAMPLES: u32 = 2000;    // S7 5.13 example
pub const BOOTSTRAP_SEED: u64 = 20261003;     // S7 5.13 example
pub const MIN_BOOTSTRAP_PARTICIPANTS: u32 = 5; // S7 5.13 [TUNABLE]
pub const CALIBRATION_BINS: usize = 10;       // [DEFAULT] equal width 0.1
pub const LATENCY_EDGES_MS: [u32; 5] = [0, 100, 250, 500, 1000]; // bins [0,100) [100,250) [250,500) [500,1000) [1000,inf); S7 5.13 lists 2000 too, see trap 6

// labels.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Label { Junk, Wanted }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum SwipeDirection { Left, Right, Up, Down }
/// Left not undone: Junk. Right or Up not undone: Wanted. Undone flips. Down: None (not a label).
pub fn label_for(direction: SwipeDirection, undone: bool) -> Option<Label>;
/// list, bulk_no_header, suspect: Junk. notice, personal: Wanted.
pub fn predicted_label(class: MessageClass) -> Label;

// rates.rs
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rate { pub value: Option<f64>, pub lower: Option<f64>, pub upper: Option<f64>, pub n: Option<u64> }
impl Rate {
    pub fn of(successes: u64, n: u64) -> Rate;          // n == 0: all None except n = Some(0)
    pub fn suppressed() -> Rate;                         // all None
}
pub fn wilson(successes: u64, n: u64) -> Option<(f64, f64)>;   // None when n == 0

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JunkConfusion { pub tp: u64, pub fp: u64, pub fn_: u64, pub tn: u64 }  // positive = Junk
impl JunkConfusion {
    pub fn add(&mut self, predicted: Label, actual: Label);
    pub fn n(&self) -> u64;
    pub fn accuracy(&self) -> Rate;          // (tp + tn) / n
    pub fn junk_precision(&self) -> Rate;    // tp / (tp + fp)
    pub fn junk_recall(&self) -> Rate;       // tp / (tp + fn)
    pub fn junk_f1(&self) -> Option<f64>;    // 2PR / (P + R); None if P or R undefined or P + R == 0
    pub fn false_junk_rate(&self) -> Rate;   // fp / (fp + tn): wanted mail predicted junk
}

// paired.rs
pub fn mcnemar_exact_p(only_a: u64, only_b: u64) -> f64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticipantPair { pub n: u64, pub a_correct: u64, pub b_correct: u64 }   // paired cards for one participant
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum BootstrapMethod { ClusterByParticipant, InsufficientParticipants }
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BootstrapResult { pub value: Option<f64>, pub lower: Option<f64>, pub upper: Option<f64>,
    pub method: BootstrapMethod, pub resamples: u32, pub seed: u64, pub participants: u32 }
pub fn cluster_bootstrap_diff(parts: &[ParticipantPair], resamples: u32, seed: u64) -> BootstrapResult;
/// Largest participant's share of all labels; None when total < MIN_CELL_SIZE.
pub fn top_contributor_share(labels_per_participant: &[u64]) -> Option<Rate>;

// rng.rs
pub struct SplitMix64 { state: u64 }
impl SplitMix64 { pub fn new(seed: u64) -> Self; pub fn next_u64(&mut self) -> u64; pub fn below(&mut self, n: u64) -> u64; }

// calibration.rs
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CalibrationBin { pub confidence_from: f64, pub confidence_to: f64, pub count: u64,
    pub mean_confidence: Option<f64>, pub observed_accuracy: Option<f64> }
/// points: (confidence in 0..=1, prediction correct). Returns all 10 bins and ECE (None when no points).
pub fn calibration(points: &[(f64, bool)]) -> (Vec<CalibrationBin>, Option<f64>);

// latency.rs
pub fn nearest_rank(sorted_ms: &[u32], p: f64) -> Option<u32>;      // p in (0, 1]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LatencyBin { pub from_ms: u32, pub to_ms: Option<u32>, pub count: u64 }  // to_ms None = open top bin
pub fn latency_histogram(answered_ms: &[u32]) -> Vec<LatencyBin>;  // 5 bins, sum == answered_ms.len()

// suppress.rs
/// cells: counts; groups: index sets that each sum to a published total. Returns Some(count) or None.
pub fn suppress_cells(cells: &[u64], groups: &[Vec<usize>], min_cell: u64) -> Vec<Option<u64>>;
```

## Algorithm

All arithmetic is `f64` on counts converted with `as f64`; no `f32` until the caller stores it.

**Label mapping (BAKE-6).** `label_for`: `(Left, false)` Junk, `(Left, true)` Wanted, `(Right | Up, false)` Wanted, `(Right | Up, true)` Junk, `(Down, _)` None. A prediction is correct when `predicted_label(class) == label`.

**Wilson interval.** For `x` successes in `n > 0` trials, `p = x / n`, `z = Z_95`:

- `denom = 1 + z² / n`
- `centre = (p + z² / (2n)) / denom`
- `half = z × sqrt(p(1 − p) / n + z² / (4n²)) / denom`
- `lower = max(0, centre − half)`, `upper = min(1, centre + half)`; force `lower = 0` exactly when `x == 0` and `upper = 1` exactly when `x == n`.

`Rate::of(x, n)` = `{ value: x/n, lower, upper, n }`.

**Junk confusion.** As in the signatures. A rate whose denominator is 0 has `value: None`.

**McNemar exact (two-sided, binomial).** `b = only_a`, `c = only_b`, `m = b + c`. If `m == 0` return 1.0. Else `k = min(b, c)` and `p = min(1, 2 × Σ_{i=0..k} C(m, i) × 0.5^m)`. Compute in log space so large `m` does not overflow: `t_0 = m × ln 0.5`, `t_{i+1} = t_i + ln(m − i) − ln(i + 1)`, sum with log-sum-exp (`max + ln Σ exp(t_i − max)`), then `exp`.

**SplitMix64** (exact, so results match across platforms), all arithmetic wrapping on `u64`:

```
state = state + 0x9E3779B97F4A7C15
z = state
z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
z = (z ^ (z >> 27)) * 0x94D049BB133111EB
return z ^ (z >> 31)
```

`below(n)` = `((next_u64() as u128 * n as u128) >> 64) as u64` (multiply-shift; no modulo).

**Cluster bootstrap by participant.** Input: one `ParticipantPair` per participant over paired cards only (cards where both models gave a valid answer).

1. `P = parts.len()`, `N = Σ n`, observed `value = (Σ a_correct − Σ b_correct) / N` (`None` if `N == 0`).
2. If `P < MIN_BOOTSTRAP_PARTICIPANTS`: return `lower` and `upper` `None`, method `InsufficientParticipants`.
3. `rng = SplitMix64::new(seed)`. For `r` in `0..resamples`: for `k` in `0..P`: `i = rng.below(P)`; add `parts[i]` to running sums `n, a, b`. Store `d_r = (a − b) as f64 / n as f64` (the integer difference is signed: use `i64`). Draw order matters: participants inner, resamples outer.
4. Sort `d` with `f64::total_cmp`. `lower = d[floor(0.025 × resamples)]`, `upper = d[ceil(0.975 × resamples) − 1]`. With 2,000 resamples: `d[50]` and `d[1949]`.

**Top contributor share.** `total = Σ labels`; if `total < MIN_CELL_SIZE` return `None`; else `Rate::of(max, total)`.

**Calibration and ECE.** 10 bins of width 0.1. Bin index `= min(floor(c × 10), 9)` for confidence `c`; points with non-finite `c` or `c` outside 0 to 1 are skipped (the classifiers already validate). Per bin: `count`, `mean_confidence = Σc / count`, `observed_accuracy = correct / count` (both `None` when empty). `ECE = Σ over non-empty bins (count / N) × |observed_accuracy − mean_confidence|`, `N` = points used.

**Nearest-rank percentile.** `rank = ceil(p × len)`, value `sorted[rank − 1]`; empty input gives `None`.

**Latency histogram.** Bins `[0,100)`, `[100,250)`, `[250,500)`, `[500,1000)`, `[1000, open)`; counts sum to the number of answered calls (STAT-8). Timeouts are not latencies; T-908a reports them as `timeout_rate`.

**Small-cell suppression with complementary suppression.**

1. `out[i] = None` if `cells[i] < min_cell`, else `Some(cells[i])`.
2. Repeat until nothing changes: for each group, if exactly one cell in the group is `None` and the group has at least two cells, set to `None` the smallest still-shown cell in that group (lowest index on ties).
3. Return `out`. Rates built on a suppressed count are suppressed by the caller.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| STAT-1 | Precision, recall, F1 for junk and the false-junk rate match hand-computed values on a fixed stream |
| STAT-2 | Wilson 95% intervals match reference values, including 0 of n, n of n and n = 1, to 1e-6 |
| STAT-3 | McNemar exact p values match binomial reference values for small and large discordant counts |
| STAT-4 | The cluster bootstrap resamples participants, is identical across runs with a fixed seed, widens when one participant dominates, and the top share is suppressed below the minimum |
| STAT-5 | Cells below the minimum are suppressed and no suppressed value can be recovered by subtraction |
| STAT-8 | Latency bins are fixed and sum to the answered count |
| BAKE-6 | The pure label mapping: left not undone is junk, right or up wanted, undo flips, down no label |

## Tests that must pass

Known values (tolerance `1e-9` absolute unless stated; Wilson to `1e-6` per S10):

- `stat_2_wilson_known_values`:

| x | n | lower | upper |
| --- | --- | --- | --- |
| 0 | 10 | 0.000000000 | 0.277532800 |
| 10 | 10 | 0.722467200 | 1.000000000 |
| 1 | 1 | 0.206549314 | 1.000000000 |
| 0 | 1 | 0.000000000 | 0.793450686 |
| 1 | 2 | 0.094531206 | 0.905468794 |
| 5 | 10 | 0.236593091 | 0.763406909 |
| 81 | 263 | 0.255288520 | 0.366209577 |
| 95 | 100 | 0.888249531 | 0.978456321 |
| 7231 | 8312 | 0.862544763 | 0.877007576 |
| any | 0 | None | None |

  (81 of 263 matches the published Newcombe (1998) example, 0.2553 to 0.3662.)

- `stat_1_labels_and_rates` on the fixed stream TP 6, FP 2, FN 3, TN 9 (20 labelled cards):
  accuracy 0.75 with Wilson (0.531299122, 0.888138299), n 20; junk precision 0.75 (0.409275430, 0.928520787), n 8; junk recall 6/9 = 0.666666667 (0.354202136, 0.879416182), n 9; F1 = 12/17 = 0.705882353; false-junk rate 2/11 = 0.181818182 (0.051367690, 0.476980562), n 11.
- `stat_1_zero_denominators` (precision with no junk predictions: `value` None, `n` Some(0); F1 None).
- `stat_3_mcnemar_known_values` (relative tolerance `1e-9`):

| only_a | only_b | p |
| --- | --- | --- |
| 0 | 0 | 1 |
| 0 | 1 | 1 |
| 1 | 0 | 1 |
| 1 | 1 | 1 |
| 0 | 5 | 0.0625 |
| 2 | 8 | 0.109375 |
| 3 | 12 | 0.03515625 |
| 5 | 5 | 1 |
| 40 | 60 | 0.056887933641 |
| 310 | 420 | 5.31414162363e-05 |

- `stat_3_mcnemar_symmetric` (property: `p(b, c) == p(c, b)`, `0 < p <= 1`).
- `stat_4_splitmix64_reference`: seed 0 gives `0xE220A8397B1DCDAF` then `0x6E789E6AA1B965F4`; seed 20261003 gives `7308962414873527341`, `10289853421815645738`, `3973643119324843906`.
- `stat_4_cluster_bootstrap_deterministic` (2,000 resamples, seed 20261003, tolerance `1e-12`):
  - "mixed" participants `(n, a_correct, b_correct)`: (120, 100, 95), (80, 70, 60), (300, 260, 250), (40, 30, 33), (60, 50, 45), (400, 350, 340): value 0.037, lower 0.01818181818181818, upper 0.07272727272727272; run twice, identical bits.
- `stat_4_concentration_widens_interval`:
  - concentrated: (800, 720, 640), (50, 40, 41), (50, 41, 40), (50, 40, 40), (50, 39, 41): value 0.078, lower -0.024, upper 0.0956 (width 0.1196);
  - same totals spread evenly: (200, 176, 160) three times, (200, 176, 161) twice: value 0.078, lower 0.076, upper 0.08 (width 0.004);
  - assert concentrated width > even width.
- `stat_4_insufficient_participants`: the first four "spread evenly" participants above give value 0.07875 (63 of 800), `lower` and `upper` None, method `InsufficientParticipants`.
- `stat_4_top_contributor_share`: `[900, 50, 50]` gives 0.9 with n 1000; `[5, 4]` gives None.
- `stat_5_suppress_known_cases` (min 10):

| cells | groups | expected |
| --- | --- | --- |
| [3, 50, 40, 12] | [[0,1,2,3]] | [None, 50, 40, None] |
| [3, 4, 50] | [[0,1,2]] | [None, None, 50] |
| [50, 40, 30] | [[0,1,2]] | [50, 40, 30] |
| [0, 50, 60] | [[0,1,2]] | [None, None, 60] |
| [5, 20, 20] | [[0,1,2]] | [None, None, 20] |
| [5, 30, 40, 50] (2 by 2 table a b / c d) | rows [[0,1],[2,3]], columns [[0,2],[1,3]] | [None, None, None, None] |

- `stat_5_no_cell_recoverable` (property: random cells and one group; after suppression the number of `None` cells in the group is 0 or at least 2).
- `stat_8_latency_histogram_and_percentiles`: latencies `[120, 340, 80, 95, 2000, 510, 260, 999, 1000, 1500, 250, 99]` give bins 3, 1, 3, 2, 3 (sum 12), p50 260, p95 2000.
- `ece_known_value`: points (0.95, true), (0.92, true), (0.91, false), (0.85, true), (0.75, false), (0.72, true), (0.55, true), (0.05, false), (1.0, true), (0.1, false): ECE 0.2 (tolerance `1e-12`); bin 9 has count 4, mean confidence 0.945, accuracy 0.75; 0.1 falls in bin 1; 1.0 falls in bin 9.
- `bake_6_label_mapping` (unit: all eight `(direction, undone)` combinations).

## Edge cases and traps

1. No randomness from anywhere but `SplitMix64` with the given seed; never `rand`, never `HashMap` iteration order in results.
2. Sort floats with `total_cmp`; `partial_cmp().unwrap()` is banned and panics on NaN.
3. The bootstrap difference uses signed integers before dividing; `u64` subtraction underflows when B beats A.
4. Draw `P` participants per resample, with replacement, in the stated loop order, or the reference values will not match.
5. `floor(c × 10)`: 0.1 × 10 is exactly 1.0 in `f64`, so 0.1 is in bin 1; 1.0 is clamped into bin 9.
6. S7 5.13 lists latency edges "0, 100, 250, 500, 1000, 2000, timeout", but the model timeout is 2,000 ms, so nothing answered lands above 2,000 and STAT-8 needs bins that sum to answered calls. The top bin here is "1000 and over"; timeouts are reported separately. Reported to the spec owner.
7. Calibration for header rules uses `bulk_score / 100` as confidence (S7 5.13). That is the probability of bulk, not confidence in the predicted class; the library takes whatever confidence the caller passes. Reported to the spec owner.
8. Keep everything `pub` and pure; no `Clock`, no I/O, no logging. Coverage floor for `domain` is 90%.

## Out of scope

- Turning records into the report (segments, trend, versions, cost): T-908a. Endpoints, CSV and snapshots: T-908b.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR quotes one reverted-change failure for `stat_4_cluster_bootstrap_deterministic`.
