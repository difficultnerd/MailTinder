//! The bake-off report body (S7 5.13 API-ADM-10; CR-01a G1 to G5 and G7).
//!
//! Pure assembly: evaluation rows and one query in, the finished report out,
//! already suppressed (STAT-5). No `Clock`, no I/O, no logging (S3 INV-7), and
//! no field of the report holds a per-record value: participant indexes are
//! assigned by the caller and used only inside [`build_report`] (S10 9.4).
//!
//! Suppression runs last, on the finished numbers: suppressing a cell early and
//! then summing its group would leak the hidden value.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, UtcOffset};

use crate::MessageClass;

use super::calibration::calibration;
use super::labels::{label_for, predicted_label, Label, SwipeDirection};
use super::latency::{latency_histogram, nearest_rank};
use super::paired::{
    cluster_bootstrap_diff, mcnemar_exact_p, top_contributor_share, BootstrapMethod,
    ParticipantPair,
};
use super::prices::{price_for, PriceTable};
use super::rates::{JunkConfusion, Rate};
use super::suppress::suppress_cells;
use super::{BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, MIN_CELL_SIZE};

// Calendar dates on the wire, as S7 writes them (`format: date`).
time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

/// The three methods every report covers (CR-01a G1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The free in-process baseline.
    HeaderRules,
    /// Google's classifier.
    Gemini,
    /// TypeSafe Jev.
    Jev,
}

/// Every method, in report order.
pub const METHODS: [Method; 3] = [Method::HeaderRules, Method::Gemini, Method::Jev];

/// The label mapping version the swipes are read through (S7 5.13); the mapping
/// itself is fixed in code (CR-01 1.6).
pub const GROUND_TRUTH_VERSION: &str = "1";

/// The only interval method the report publishes (S7 5.13).
const INTERVAL_METHOD: &str = "wilson_95";

/// The baseline's classifier id (S3, S4 5.6).
const HEADER_RULES_CLASSIFIER_ID: &str = "header_rules@1";

/// The three pairs the report compares, in report order (CR-01a G3).
const PAIRS: [(Method, Method); 3] = [
    (Method::HeaderRules, Method::Gemini),
    (Method::HeaderRules, Method::Jev),
    (Method::Gemini, Method::Jev),
];

impl Method {
    /// The wire name of the method (S7 5.13 `Method`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::HeaderRules => "header_rules",
            Method::Gemini => "gemini",
            Method::Jev => "jev",
        }
    }
}

/// Why a model call did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredictionError {
    /// The call hit the model timeout.
    Timeout,
    /// Any other failure.
    Other,
}

/// One model's answer for one card (domain mirror of a stored prediction).
// nosemgrep: privacy-rust-derive-debug-on-sensitive-struct -- input_tokens is a billed token count, not message text
#[derive(Clone, Debug)]
pub struct Prediction {
    /// The predicted class, absent when the model answered nothing usable.
    pub class: Option<MessageClass>,
    /// The model's own bulk score.
    pub score: Option<u8>,
    /// The confidence the model stated, when it stated one.
    pub confidence: Option<f64>,
    /// How long the call took.
    pub latency_ms: u32,
    /// Input tokens billed for the call.
    pub input_tokens: Option<u32>,
    /// The failure, when the call failed.
    pub error: Option<PredictionError>,
    /// The model version that answered (`gemini@flash-lite`, `jev@1.13.0`).
    pub classifier_id: String,
}

/// Domain mirror of one `classifier_eval` record (T-908b converts from the ports
/// record).
///
/// `participant` is an opaque index assigned by the caller, never the
/// pseudonymous ID itself.
#[derive(Clone, Debug)]
pub struct EvalRow {
    /// Opaque participant index.
    pub participant: u32,
    /// When the swipe landed.
    pub created_at: OffsetDateTime,
    /// The header-rules class and its 0 to 100 bulk score.
    pub header_rules: (MessageClass, u8),
    /// Gemini's answer, `None` when the model was switched off.
    pub gemini: Option<Prediction>,
    /// Jev's answer, `None` when the model was switched off.
    pub jev: Option<Prediction>,
    /// The swipe direction.
    pub direction: SwipeDirection,
    /// Whether the swipe was undone.
    pub undone: bool,
    /// The segment values of the card.
    pub segments: Segments,
    /// Redaction and truncation rules version.
    pub input_version: String,
    /// Prompt and question version.
    pub question_version: String,
    /// Price table version for cost.
    pub price_version: String,
}

/// The segment values of one card (CR-01a G4).
// nosemgrep: privacy-rust-derive-debug-on-sensitive-struct -- text_tokens_bucket is a length bucket label, not message text
#[derive(Clone, Debug)]
pub struct Segments {
    /// `EvalHeaderFacts` names and values.
    pub facts: [(&'static str, bool); 8],
    /// `gmail` or `graph`.
    pub provider: &'static str,
    /// Message age bucket.
    pub age_bucket: &'static str,
    /// Text length bucket.
    pub text_tokens_bucket: &'static str,
    /// Whether the local detector called the text English.
    pub lang_is_english: bool,
}

/// The ADM-10 query.
///
/// S7 requires only `from` and `to`, so every optional field defaults.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReportQuery {
    /// First UTC day, inclusive.
    #[serde(with = "iso_date")]
    pub from: Date,
    /// Last UTC day, inclusive.
    #[serde(with = "iso_date")]
    pub to: Date,
    /// Narrow to one redaction and truncation version.
    #[serde(default)]
    pub input_version: Option<String>,
    /// Narrow to one question version.
    #[serde(default)]
    pub question_version: Option<String>,
    /// Narrow to one price version; never blocks pooling.
    #[serde(default)]
    pub price_version: Option<String>,
    /// Pool records of different input or question versions on purpose.
    #[serde(default)]
    pub pool_versions: bool,
}

/// Why the report could not be built.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ReportError {
    /// The range holds more than one input or question version and the query
    /// did not ask to pool them (CL-04 AC5, STAT-6).
    #[error("more than one version is present and pool_versions is false")]
    VersionsMixed {
        /// Input versions present.
        input: Vec<String>,
        /// Question versions present.
        question: Vec<String>,
    },
}

/// The versions the selected records carry (S7 5.13).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VersionsPresent {
    /// Distinct input versions.
    pub input: Vec<String>,
    /// Distinct question versions.
    pub question: Vec<String>,
    /// Distinct price versions.
    pub price: Vec<String>,
}

/// Who contributed the labels (CR-01a G3).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Participants {
    /// Distinct participants among the labelled rows.
    pub count: Option<u64>,
    /// The largest contributor's share of the labels.
    pub top_contributor_share: Option<Rate>,
}

/// One calibration bin (`confidence_from`, `confidence_to`, `count`,
/// `observed_accuracy`).
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct CalibrationBin {
    /// Inclusive lower confidence bound.
    pub confidence_from: f64,
    /// Exclusive upper confidence bound.
    pub confidence_to: f64,
    /// Points in the bin.
    pub count: Option<u64>,
    /// Observed accuracy in the bin.
    pub observed_accuracy: Option<f64>,
}

/// The calibration curve and its expected calibration error.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Calibration {
    /// Expected calibration error.
    pub ece: Option<f64>,
    /// Ten equal-width bins.
    pub bins: Vec<CalibrationBin>,
}

/// One fixed latency bin; timed-out calls are in no bin.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct LatencyBin {
    /// Inclusive lower bound in milliseconds.
    pub from_ms: u32,
    /// Exclusive upper bound, `None` for the open top bin.
    pub to_ms: Option<u32>,
    /// Answered calls in the bin.
    pub count: Option<u64>,
}

/// Latency percentiles and the fixed histogram.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Latency {
    /// Nearest-rank median.
    pub p50_ms: Option<u32>,
    /// Nearest-rank 95th percentile.
    pub p95_ms: Option<u32>,
    /// Fixed bins, summing to the answered calls.
    pub histogram: Vec<LatencyBin>,
}

/// One method's figures (S7 5.13 `models`).
// nosemgrep: privacy-rust-derive-debug-on-sensitive-struct -- input_tokens is an aggregate token count, not message text
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MethodReport {
    /// Which method.
    pub model: Method,
    /// The model version that answered.
    pub classifier_id: String,
    /// Rows where the method was called.
    pub cards: Option<u64>,
    /// Calls with no error and a class.
    pub valid_answers: Option<u64>,
    /// Accuracy against the labels.
    pub accuracy: Rate,
    /// Precision for junk.
    pub junk_precision: Rate,
    /// Recall for junk.
    pub junk_recall: Rate,
    /// F1 for junk; reported without an interval.
    pub junk_f1: Option<f64>,
    /// Wanted mail predicted junk.
    pub false_junk_rate: Rate,
    /// Calibration curve and ECE.
    pub calibration: Calibration,
    /// Latency percentiles and histogram.
    pub latency: Latency,
    /// Timeouts over cards.
    pub timeout_rate: Rate,
    /// Other errors over cards.
    pub error_rate: Rate,
    /// Input tokens over the valid answers.
    pub input_tokens: Option<u64>,
    /// USD per 1,000 answered messages.
    pub cost_per_1000_messages_usd: Option<f64>,
}

/// The paired accuracy difference `a - b` and its bootstrap interval.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct AccuracyDifference {
    /// Observed `(A - B)`.
    pub value: Option<f64>,
    /// Lower 2.5% of the bootstrap distribution.
    pub lower: Option<f64>,
    /// Upper 97.5% of the bootstrap distribution.
    pub upper: Option<f64>,
}

/// How the paired interval was produced.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Bootstrap {
    /// Resampling method.
    pub method: BootstrapMethod,
    /// Resamples drawn.
    pub resamples: u32,
    /// Seed used.
    pub seed: u64,
    /// Participants available.
    pub participants: Option<u64>,
}

/// One pair of methods on the cards both answered (CR-01a G3).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PairedReport {
    /// First method.
    pub model_a: Method,
    /// Second method.
    pub model_b: Method,
    /// Cards where both gave a valid answer and there is a label.
    pub n_paired: Option<u64>,
    /// Cards only A got right.
    pub only_a_correct: Option<u64>,
    /// Cards only B got right.
    pub only_b_correct: Option<u64>,
    /// Cards both got right.
    pub both_correct: Option<u64>,
    /// Cards neither got right.
    pub neither_correct: Option<u64>,
    /// McNemar's exact two-sided p value.
    pub mcnemar_exact_p: Option<f64>,
    /// The accuracy difference and its interval.
    pub accuracy_difference: AccuracyDifference,
    /// How the interval was produced.
    pub bootstrap: Bootstrap,
}

/// How often two methods agree (five-class, CR-01a G1).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AgreementReport {
    /// First method.
    pub model_a: Method,
    /// Second method.
    pub model_b: Method,
    /// Share of paired calls with the same class.
    pub rate: Rate,
}

/// One segment cell for one method (CR-01a G4).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SegmentReport {
    /// The dimension, e.g. `age_bucket`.
    pub dimension: String,
    /// The dimension value.
    pub value: String,
    /// Which method.
    pub model: Method,
    /// Accuracy within the segment.
    pub accuracy: Rate,
    /// Wanted mail predicted junk within the segment.
    pub false_junk_rate: Rate,
}

/// One non-empty cell of a method's five-class by two-label confusion table.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct ConfusionCell {
    /// Which method.
    pub model: Method,
    /// The predicted class.
    pub class: MessageClass,
    /// The label, `junk` or `wanted`.
    pub label: &'static str,
    /// Cards in the cell.
    pub count: Option<u64>,
}

/// One method's accuracy on one UTC day.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct TrendPoint {
    /// The UTC day.
    #[serde(with = "iso_date")]
    pub date: Date,
    /// Which method.
    pub model: Method,
    /// Accuracy on that day.
    pub accuracy: Rate,
}

/// The API-ADM-10 body (S7 5.13). Every count below `min_cell_size` is `None`.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BakeoffReport {
    /// The query, echoed (including `pool_versions`).
    pub query: ReportQuery,
    /// The versions the selected records carry.
    pub versions_present: VersionsPresent,
    /// The smallest publishable cell.
    pub min_cell_size: u64,
    /// The label mapping version.
    pub ground_truth_version: &'static str,
    /// `wilson_95`.
    pub interval_method: &'static str,
    /// Labelled rows in scope.
    pub labelled_swipes: Option<u64>,
    /// Participant count and largest share.
    pub participants: Participants,
    /// One entry per method, always all three.
    pub models: Vec<MethodReport>,
    /// One entry per pair.
    pub paired: Vec<PairedReport>,
    /// One entry per pair.
    pub agreement: Vec<AgreementReport>,
    /// One entry per method, dimension and value present.
    pub by_segment: Vec<SegmentReport>,
    /// Non-empty cells of every method's confusion table.
    pub confusion: Vec<ConfusionCell>,
    /// One entry per method per day it ran.
    pub trend: Vec<TrendPoint>,
}

/// Build the complete, already suppressed ADM-10 report (T-908a).
///
/// # Errors
///
/// `VersionsMixed` when the range holds more than one input or question version
/// and `query.pool_versions` is false (CL-04 AC5, STAT-6).
pub fn build_report(
    rows: &[EvalRow],
    query: &ReportQuery,
    prices: &PriceTable,
) -> Result<BakeoffReport, ReportError> {
    let selected: Vec<&EvalRow> = rows.iter().filter(|row| in_scope(row, query)).collect();
    let versions_present = versions_present(&selected);
    if !query.pool_versions
        && (versions_present.input.len() > 1 || versions_present.question.len() > 1)
    {
        return Err(ReportError::VersionsMixed {
            input: versions_present.input.clone(),
            question: versions_present.question.clone(),
        });
    }

    let mut report = assemble(&selected, query, prices, versions_present);

    // STAT-5: collect every published count and its complementary groups, then
    // suppress last, on the finished numbers.
    let mut cells = Cells::default();
    walk_report(&mut report, &mut cells);
    let shown = suppress_cells(&cells.values, &cells.groups, report.min_cell_size);
    let mut apply = Apply {
        shown: &shown,
        next: 0,
    };
    walk_report(&mut report, &mut apply);
    debug_assert_eq!(apply.next, shown.len());

    if report.labelled_swipes.is_none() {
        report.participants.count = None;
        report.participants.top_contributor_share = None;
    }
    Ok(report)
}

/// Keep the rows the query asks for: UTC days inside the range and any version
/// filter given (S7 5.13).
fn in_scope(row: &EvalRow, query: &ReportQuery) -> bool {
    let day = row.created_at.to_offset(UtcOffset::UTC).date();
    if day < query.from || day > query.to {
        return false;
    }
    if let Some(version) = &query.input_version {
        if version != &row.input_version {
            return false;
        }
    }
    if let Some(version) = &query.question_version {
        if version != &row.question_version {
            return false;
        }
    }
    if let Some(version) = &query.price_version {
        if version != &row.price_version {
            return false;
        }
    }
    true
}

/// The distinct versions the selected records carry (STAT-6).
fn versions_present(rows: &[&EvalRow]) -> VersionsPresent {
    let mut input = BTreeSet::new();
    let mut question = BTreeSet::new();
    let mut price = BTreeSet::new();
    for row in rows {
        input.insert(row.input_version.clone());
        question.insert(row.question_version.clone());
        price.insert(row.price_version.clone());
    }
    VersionsPresent {
        input: input.into_iter().collect(),
        question: question.into_iter().collect(),
        price: price.into_iter().collect(),
    }
}

/// One method's answer for one row, or `None` when the model was not called.
struct Call<'a> {
    class: Option<MessageClass>,
    confidence: Option<f64>,
    latency_ms: u32,
    input_tokens: Option<u32>,
    error: Option<PredictionError>,
    classifier_id: &'a str,
}

/// The call a method made for a row; header rules always answer, in process.
fn call(row: &EvalRow, method: Method) -> Option<Call<'_>> {
    match method {
        Method::HeaderRules => Some(Call {
            class: Some(row.header_rules.0),
            // The bulk score is the probability that the message is bulk, read
            // as the confidence the calibration curve uses (S7 5.13).
            confidence: Some(f64::from(row.header_rules.1) / 100.0),
            latency_ms: 0,
            input_tokens: None,
            error: None,
            classifier_id: HEADER_RULES_CLASSIFIER_ID,
        }),
        Method::Gemini => row.gemini.as_ref().map(model_call),
        Method::Jev => row.jev.as_ref().map(model_call),
    }
}

/// Convert a stored prediction into a call.
fn model_call(prediction: &Prediction) -> Call<'_> {
    Call {
        class: prediction.class,
        confidence: prediction.confidence,
        latency_ms: prediction.latency_ms,
        input_tokens: prediction.input_tokens,
        error: prediction.error,
        classifier_id: &prediction.classifier_id,
    }
}

/// The class a method answered with, when the answer is valid: no error and a
/// class (STAT-1).
fn answered(row: &EvalRow, method: Method) -> Option<MessageClass> {
    call(row, method)
        .filter(|call| call.error.is_none())
        .and_then(|call| call.class)
}

/// Every dimension and value a row contributes to `by_segment` (CR-01a G4).
fn dimensions(row: &EvalRow) -> Vec<(&'static str, String)> {
    let mut out = Vec::with_capacity(1 + 8 + 4);
    out.push(("header_rules_class", row.header_rules.0.as_str().to_owned()));
    for &(name, value) in &row.segments.facts {
        out.push((name, bool_str(value).to_owned()));
    }
    out.push(("provider", row.segments.provider.to_owned()));
    out.push(("age_bucket", row.segments.age_bucket.to_owned()));
    out.push((
        "text_tokens_bucket",
        row.segments.text_tokens_bucket.to_owned(),
    ));
    out.push((
        "lang_is_english",
        bool_str(row.segments.lang_is_english).to_owned(),
    ));
    out
}

/// A boolean segment value on the wire.
const fn bool_str(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// A label as the confusion table writes it.
const fn label_str(label: Label) -> &'static str {
    match label {
        Label::Junk => "junk",
        Label::Wanted => "wanted",
    }
}

/// A class's position in [`MessageClass::ALL`].
fn class_index(class: MessageClass) -> usize {
    MessageClass::ALL
        .iter()
        .position(|&candidate| candidate == class)
        .unwrap_or(0)
}

/// A label's column in the confusion table: junk first.
const fn label_index(label: Label) -> usize {
    match label {
        Label::Junk => 0,
        Label::Wanted => 1,
    }
}

/// One method's running totals while the rows are read.
#[derive(Default)]
struct Acc {
    cards: u64,
    valid: u64,
    timeouts: u64,
    other_errors: u64,
    labelled: JunkConfusion,
    latencies: Vec<u32>,
    points: Vec<(f64, bool)>,
    tokens: u64,
    cost_sum: f64,
    missing_price: bool,
    ids: BTreeMap<String, u64>,
}

impl Acc {
    /// Record one call; returns the class and label when it counts for accuracy.
    fn add(
        &mut self,
        call: &Call<'_>,
        label: Option<Label>,
        price_version: &str,
        prices: &PriceTable,
        method: Method,
    ) -> Option<(MessageClass, Label)> {
        self.cards += 1;
        *self.ids.entry(call.classifier_id.to_owned()).or_insert(0) += 1;
        match call.error {
            // Errors count against availability, never against accuracy
            // (STAT-1).
            Some(PredictionError::Timeout) => {
                self.timeouts += 1;
                return None;
            }
            Some(PredictionError::Other) => {
                self.other_errors += 1;
                return None;
            }
            None => {}
        }
        let class = call.class?;
        self.valid += 1;
        self.latencies.push(call.latency_ms);
        let tokens = u64::from(call.input_tokens.unwrap_or(0));
        self.tokens += tokens;
        match price_for(prices, price_version, method) {
            Some(price) => self.cost_sum += tokens as f64 * price / 1_000_000.0,
            // A record this report cannot price makes the whole cost unknown.
            None => self.missing_price = true,
        }
        let label = label?;
        let predicted = predicted_label(class);
        self.labelled.add(predicted, label);
        if let Some(confidence) = call.confidence {
            self.points.push((confidence, predicted == label));
        }
        Some((class, label))
    }

    /// USD per 1,000 answered messages; `None` when nothing was answered or a
    /// record could not be priced.
    fn cost_per_1000(&self) -> Option<f64> {
        if self.valid == 0 || self.missing_price {
            None
        } else {
            Some(self.cost_sum / self.valid as f64 * 1000.0)
        }
    }
}

/// One pair's running paired totals.
#[derive(Default)]
struct PairAcc {
    n: u64,
    only_a: u64,
    only_b: u64,
    both: u64,
    neither: u64,
    by_participant: BTreeMap<u32, ParticipantPair>,
}

impl PairAcc {
    /// Record one paired card.
    fn add(&mut self, participant: u32, a_correct: bool, b_correct: bool) {
        self.n += 1;
        match (a_correct, b_correct) {
            (true, true) => self.both += 1,
            (true, false) => self.only_a += 1,
            (false, true) => self.only_b += 1,
            (false, false) => self.neither += 1,
        }
        let entry = self
            .by_participant
            .entry(participant)
            .or_insert(ParticipantPair {
                n: 0,
                a_correct: 0,
                b_correct: 0,
            });
        entry.n += 1;
        if a_correct {
            entry.a_correct += 1;
        }
        if b_correct {
            entry.b_correct += 1;
        }
    }
}

/// Read every number the report publishes out of the rows.
fn assemble(
    rows: &[&EvalRow],
    query: &ReportQuery,
    prices: &PriceTable,
    versions_present: VersionsPresent,
) -> BakeoffReport {
    let mut models: [Acc; 3] = std::array::from_fn(|_| Acc::default());
    let mut segments: [BTreeMap<(String, String), JunkConfusion>; 3] =
        std::array::from_fn(|_| BTreeMap::new());
    let mut trend: BTreeMap<(Date, usize), JunkConfusion> = BTreeMap::new();
    let mut grids = [[[0_u64; 2]; 5]; 3];
    let mut pairs: [PairAcc; 3] = std::array::from_fn(|_| PairAcc::default());
    // (same class, both answered) per pair.
    let mut agreement = [(0_u64, 0_u64); 3];
    let mut labels_by_participant: BTreeMap<u32, u64> = BTreeMap::new();
    let mut labelled_swipes = 0_u64;

    for row in rows {
        let label = label_for(row.direction, row.undone);
        let day = row.created_at.to_offset(UtcOffset::UTC).date();
        if label.is_some() {
            labelled_swipes += 1;
            *labels_by_participant.entry(row.participant).or_insert(0) += 1;
        }

        for (index, &method) in METHODS.iter().enumerate() {
            let Some(call) = call(row, method) else {
                continue;
            };
            let Some((class, actual)) =
                models[index].add(&call, label, &row.price_version, prices, method)
            else {
                continue;
            };
            let predicted = predicted_label(class);
            grids[index][class_index(class)][label_index(actual)] += 1;
            for (dimension, value) in dimensions(row) {
                segments[index]
                    .entry((dimension.to_owned(), value))
                    .or_default()
                    .add(predicted, actual);
            }
            trend
                .entry((day, index))
                .or_default()
                .add(predicted, actual);
        }

        for (slot, &(a, b)) in PAIRS.iter().enumerate() {
            let (Some(a_class), Some(b_class)) = (answered(row, a), answered(row, b)) else {
                continue;
            };
            agreement[slot].1 += 1;
            if a_class == b_class {
                agreement[slot].0 += 1;
            }
            let Some(label) = label else {
                continue;
            };
            pairs[slot].add(
                row.participant,
                predicted_label(a_class) == label,
                predicted_label(b_class) == label,
            );
        }
    }

    let model_reports = METHODS
        .iter()
        .enumerate()
        .map(|(index, &method)| model_report(method, &models[index]))
        .collect();

    let mut by_segment = Vec::new();
    for (index, &method) in METHODS.iter().enumerate() {
        for ((dimension, value), confusion) in &segments[index] {
            by_segment.push(SegmentReport {
                dimension: dimension.clone(),
                value: value.clone(),
                model: method,
                accuracy: confusion.accuracy(),
                false_junk_rate: confusion.false_junk_rate(),
            });
        }
    }

    let mut confusion = Vec::new();
    for (index, &method) in METHODS.iter().enumerate() {
        for (class_slot, &class) in MessageClass::ALL.iter().enumerate() {
            for (label_slot, label) in [Label::Junk, Label::Wanted].into_iter().enumerate() {
                let count = grids[index][class_slot][label_slot];
                // One cell per non-empty combination (S7 5.13).
                if count > 0 {
                    confusion.push(ConfusionCell {
                        model: method,
                        class,
                        label: label_str(label),
                        count: Some(count),
                    });
                }
            }
        }
    }

    let trend_points = trend
        .into_iter()
        .map(|((date, index), confusion)| TrendPoint {
            date,
            model: METHODS[index],
            accuracy: confusion.accuracy(),
        })
        .collect();

    let paired = PAIRS
        .iter()
        .enumerate()
        .map(|(slot, &(model_a, model_b))| paired_report(model_a, model_b, &pairs[slot]))
        .collect();

    let agreement_reports = PAIRS
        .iter()
        .enumerate()
        .map(|(slot, &(model_a, model_b))| AgreementReport {
            model_a,
            model_b,
            rate: Rate::of(agreement[slot].0, agreement[slot].1),
        })
        .collect();

    let shares: Vec<u64> = labels_by_participant.values().copied().collect();
    let participants = Participants {
        count: Some(labels_by_participant.len() as u64),
        top_contributor_share: top_contributor_share(&shares),
    };

    BakeoffReport {
        query: query.clone(),
        versions_present,
        min_cell_size: MIN_CELL_SIZE,
        ground_truth_version: GROUND_TRUTH_VERSION,
        interval_method: INTERVAL_METHOD,
        labelled_swipes: Some(labelled_swipes),
        participants,
        models: model_reports,
        paired,
        agreement: agreement_reports,
        by_segment,
        confusion,
        trend: trend_points,
    }
}

/// One method's figures, before suppression.
fn model_report(method: Method, acc: &Acc) -> MethodReport {
    let (bins, ece) = calibration(&acc.points);
    let mut latencies = acc.latencies.clone();
    latencies.sort_unstable();
    MethodReport {
        model: method,
        classifier_id: dominant_classifier_id(&acc.ids),
        cards: Some(acc.cards),
        valid_answers: Some(acc.valid),
        accuracy: acc.labelled.accuracy(),
        junk_precision: acc.labelled.junk_precision(),
        junk_recall: acc.labelled.junk_recall(),
        junk_f1: acc.labelled.junk_f1(),
        false_junk_rate: acc.labelled.false_junk_rate(),
        calibration: Calibration {
            ece,
            bins: bins
                .into_iter()
                .map(|bin| CalibrationBin {
                    confidence_from: bin.confidence_from,
                    confidence_to: bin.confidence_to,
                    count: Some(bin.count),
                    observed_accuracy: bin.observed_accuracy,
                })
                .collect(),
        },
        latency: Latency {
            p50_ms: nearest_rank(&latencies, 0.5),
            p95_ms: nearest_rank(&latencies, 0.95),
            histogram: latency_histogram(&latencies)
                .into_iter()
                .map(|bin| LatencyBin {
                    from_ms: bin.from_ms,
                    to_ms: bin.to_ms,
                    count: Some(bin.count),
                })
                .collect(),
        },
        timeout_rate: Rate::of(acc.timeouts, acc.cards),
        error_rate: Rate::of(acc.other_errors, acc.cards),
        input_tokens: Some(acc.tokens),
        cost_per_1000_messages_usd: acc.cost_per_1000(),
    }
}

/// The classifier id that answered most, ties going to the smaller id; empty
/// when the method never ran.
fn dominant_classifier_id(ids: &BTreeMap<String, u64>) -> String {
    let mut best: Option<(&str, u64)> = None;
    for (id, &count) in ids {
        if best.map_or(true, |(_, best_count)| count > best_count) {
            best = Some((id.as_str(), count));
        }
    }
    best.map(|(id, _)| id.to_owned()).unwrap_or_default()
}

/// One pair's figures, before suppression. Participants are resampled in
/// ascending index order so the bootstrap is reproducible.
fn paired_report(model_a: Method, model_b: Method, acc: &PairAcc) -> PairedReport {
    let parts: Vec<ParticipantPair> = acc.by_participant.values().copied().collect();
    let bootstrap = cluster_bootstrap_diff(&parts, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    PairedReport {
        model_a,
        model_b,
        n_paired: Some(acc.n),
        only_a_correct: Some(acc.only_a),
        only_b_correct: Some(acc.only_b),
        both_correct: Some(acc.both),
        neither_correct: Some(acc.neither),
        mcnemar_exact_p: Some(mcnemar_exact_p(acc.only_a, acc.only_b)),
        accuracy_difference: AccuracyDifference {
            value: bootstrap.value,
            lower: bootstrap.lower,
            upper: bootstrap.upper,
        },
        bootstrap: Bootstrap {
            method: bootstrap.method,
            resamples: bootstrap.resamples,
            seed: bootstrap.seed,
            participants: Some(u64::from(bootstrap.participants)),
        },
    }
}

/// A published value that has a suppressed form the walk can write.
///
/// Rates, percentiles, counts and costs are all withheld the same way: when a
/// count they are built on is suppressed, the value goes away with it (STAT-5).
trait Withheld {
    /// Replaces the value with its suppressed form.
    fn withheld(&mut self);
}

impl Withheld for Rate {
    fn withheld(&mut self) {
        *self = Rate::suppressed();
    }
}

impl Withheld for Option<f64> {
    fn withheld(&mut self) {
        *self = None;
    }
}

impl Withheld for Option<u32> {
    fn withheld(&mut self) {
        *self = None;
    }
}

impl Withheld for Option<u64> {
    fn withheld(&mut self) {
        *self = None;
    }
}

/// One traversal of every published count and every complementary group of a
/// finished report, in a fixed order.
trait Walk {
    /// Visits one published count and returns its cell index.
    fn cell(&mut self, slot: &mut Option<u64>) -> usize;

    /// Withholds a value whose guard cells are suppressed.
    fn hide<S: Withheld>(&mut self, slot: &mut S, guards: &[usize]);

    /// Suppresses a rate whose guard cells are suppressed.
    fn guard(&mut self, slot: &mut Rate, guards: &[usize]) {
        self.hide(slot, guards);
    }

    /// One complementary group: cells that sum to a published total.
    fn group(&mut self, members: Vec<usize>);

    /// Visits a rate; its `n` is a published count of its own.
    fn rate(&mut self, slot: &mut Rate) -> usize {
        let index = self.cell(&mut slot.n);
        self.guard(slot, &[index]);
        index
    }
}

/// Pass one: every count, in the order the report publishes them.
#[derive(Default)]
struct Cells {
    values: Vec<u64>,
    groups: Vec<Vec<usize>>,
}

impl Walk for Cells {
    fn cell(&mut self, slot: &mut Option<u64>) -> usize {
        self.values.push(slot.unwrap_or(0));
        self.values.len() - 1
    }

    fn hide<S: Withheld>(&mut self, _slot: &mut S, _guards: &[usize]) {}

    fn group(&mut self, members: Vec<usize>) {
        if members.len() < 2 {
            return;
        }
        // A group whose cells are all zero publishes nothing to hide: every
        // cell is below the minimum anyway, and its total is not published
        // either, so the group is left out rather than making the reader's
        // system pin every cell to zero (S7 5.13, STAT-5).
        if members
            .iter()
            .all(|&member| self.values.get(member).copied().unwrap_or(0) == 0)
        {
            return;
        }
        self.groups.push(members);
    }
}

/// Pass two: write the suppressed cells back and drop the rates built on them.
struct Apply<'a> {
    shown: &'a [Option<u64>],
    next: usize,
}

impl Walk for Apply<'_> {
    fn cell(&mut self, slot: &mut Option<u64>) -> usize {
        let index = self.next;
        self.next += 1;
        *slot = self.shown[index];
        index
    }

    fn hide<S: Withheld>(&mut self, slot: &mut S, guards: &[usize]) {
        if guards.iter().any(|&guard| self.shown[guard].is_none()) {
            slot.withheld();
        }
    }

    fn group(&mut self, _members: Vec<usize>) {}
}

/// Walk every published count, rate and complementary group of the report.
///
/// The order is fixed by this function alone, so the collecting walk and the
/// applying walk visit the same cells in the same sequence.
fn walk_report<W: Walk>(report: &mut BakeoffReport, walk: &mut W) {
    let labelled = walk.cell(&mut report.labelled_swipes);
    // The participant count is a count of its own: below the minimum it is
    // withheld, and the largest-contributor share goes with it (STAT-4).
    let participant_count = walk.cell(&mut report.participants.count);
    if let Some(share) = report.participants.top_contributor_share.as_mut() {
        walk.rate(share);
        walk.guard(share, &[labelled, participant_count]);
    }

    for model in &mut report.models {
        let cards = walk.cell(&mut model.cards);
        let valid = walk.cell(&mut model.valid_answers);
        let accuracy = walk.rate(&mut model.accuracy);
        let precision = walk.rate(&mut model.junk_precision);
        let recall = walk.rate(&mut model.junk_recall);
        let false_junk = walk.rate(&mut model.false_junk_rate);
        walk.rate(&mut model.timeout_rate);
        walk.rate(&mut model.error_rate);

        // A published rate gives away its numerator as well as its denominator
        // (`value * n` is the count the rate was built from), so the four junk
        // rates together determine all four confusion counts: the accuracy
        // separates them once any one junk rate is shown beside it. With any of
        // the four denominators withheld, a shown junk rate would hand back the
        // hidden count, so every junk rate goes with the whole set (STAT-5).
        let junk_guards = [accuracy, precision, recall, false_junk];
        walk.guard(&mut model.junk_precision, &junk_guards);
        walk.guard(&mut model.junk_recall, &junk_guards);
        walk.guard(&mut model.false_junk_rate, &junk_guards);

        // Latency, tokens and cost describe the answered calls one record at a
        // time: with the answered-call count suppressed, a single answered call
        // would publish that record's latency, tokens and cost, so all three go
        // with the count they are built on (STAT-5).
        walk.hide(&mut model.latency.p50_ms, &[valid]);
        walk.hide(&mut model.latency.p95_ms, &[valid]);
        walk.hide(&mut model.input_tokens, &[valid]);
        walk.hide(&mut model.cost_per_1000_messages_usd, &[valid]);

        // The junk F1 reads both rates (STAT-5).
        walk.hide(&mut model.junk_f1, &[precision, recall]);

        // The method's published counts nest: every answered call is a card,
        // and the two label columns partition the labelled answers, so
        // `accuracy.n` is `junk_recall.n + false_junk_rate.n`. Each pair is a
        // complementary group, so a suppressed count is never the difference of
        // the shown ones (STAT-5): without these, `false_junk_rate.n` comes back
        // as `accuracy.n - junk_recall.n`.
        walk.group(vec![cards, valid]);
        walk.group(vec![recall, false_junk]);

        // The calibration curve's bins partition the labelled answers the
        // expected calibration error reads, and each populated bin publishes
        // its own count: a hidden populated bin is reconstructible from the
        // error itself, so the error is withheld with the bins that feed it
        // (`accuracy.n` guards it as well, as before) (STAT-5).
        let mut ece_guards = vec![accuracy];
        for bin in &mut model.calibration.bins {
            let populated = bin.count.is_some_and(|count| count > 0);
            let index = walk.cell(&mut bin.count);
            walk.hide(&mut bin.observed_accuracy, &[index]);
            if populated {
                ece_guards.push(index);
            }
        }
        walk.hide(&mut model.calibration.ece, &ece_guards);

        let mut members = Vec::new();
        for bin in &mut model.latency.histogram {
            members.push(walk.cell(&mut bin.count));
        }
        walk.group(members);
    }

    for pair in &mut report.paired {
        // The four counts are the group; `n_paired` is their sum, so publishing
        // it beside them adds no equation of its own (S7 5.13).
        let n_paired = walk.cell(&mut pair.n_paired);
        let only_a = walk.cell(&mut pair.only_a_correct);
        let only_b = walk.cell(&mut pair.only_b_correct);
        let both = walk.cell(&mut pair.both_correct);
        let neither = walk.cell(&mut pair.neither_correct);
        walk.group(vec![only_a, only_b, both, neither]);

        // The bootstrap resamples whole participants, so its participant count
        // is a count of its own: below the minimum it is withheld, and the
        // interval it produced goes with it (STAT-4, STAT-5).
        let participants = walk.cell(&mut pair.bootstrap.participants);

        // The accuracy difference is fixed by the two discordant counts it
        // splits (`a - b` over `n_paired`), so with either of them hidden the
        // difference hands it back; the interval is resampled from the
        // participants, so it also goes with the participant count. McNemar
        // reads the discordant counts and goes with them (STAT-5).
        walk.hide(&mut pair.mcnemar_exact_p, &[only_a, only_b]);
        walk.hide(
            &mut pair.accuracy_difference.value,
            &[n_paired, only_a, only_b],
        );
        walk.hide(
            &mut pair.accuracy_difference.lower,
            &[n_paired, only_a, only_b, participants],
        );
        walk.hide(
            &mut pair.accuracy_difference.upper,
            &[n_paired, only_a, only_b, participants],
        );
    }

    for agreement in &mut report.agreement {
        walk.rate(&mut agreement.rate);
    }

    // A segment dimension's values for one model are one group.
    let mut start = 0;
    while start < report.by_segment.len() {
        let model = report.by_segment[start].model;
        let dimension = report.by_segment[start].dimension.clone();
        let mut end = start + 1;
        while end < report.by_segment.len()
            && report.by_segment[end].model == model
            && report.by_segment[end].dimension == dimension
        {
            end += 1;
        }
        let mut members = Vec::new();
        let mut junk_members = Vec::new();
        for segment in &mut report.by_segment[start..end] {
            let index = walk.rate(&mut segment.accuracy);
            let false_junk = walk.rate(&mut segment.false_junk_rate);
            // A suppressed segment cell suppresses both of its rates (STAT-5).
            walk.guard(&mut segment.false_junk_rate, &[index]);
            members.push(index);
            junk_members.push(false_junk);
        }
        // The dimension's segment values partition the method's own labelled
        // answers, and the false-junk denominators partition the method's own
        // false-junk denominator, so each set is a complementary group of its
        // own: a hidden cell is the difference of the published total and the
        // other segments (STAT-5).
        walk.group(members);
        walk.group(junk_members);
        start = end;
    }

    // A class row (two labels) and a label column (five classes) are groups.
    let mut cells: Vec<(Method, MessageClass, &'static str, usize)> = Vec::new();
    for &method in &METHODS {
        for cell in report
            .confusion
            .iter_mut()
            .filter(|cell| cell.model == method)
        {
            let index = walk.cell(&mut cell.count);
            cells.push((method, cell.class, cell.label, index));
        }
    }
    for &method in &METHODS {
        let mut rows: Vec<(MessageClass, Vec<usize>)> = Vec::new();
        let mut columns: Vec<(&'static str, Vec<usize>)> = Vec::new();
        let mut table = Vec::new();
        for &(cell_method, class, label, index) in &cells {
            if cell_method != method {
                continue;
            }
            push_member(&mut rows, class, index);
            push_member(&mut columns, label, index);
            table.push(index);
        }
        for (_, members) in rows {
            walk.group(members);
        }
        for (_, members) in columns {
            walk.group(members);
        }
        // The whole table sums to `accuracy.n`, which is published: without
        // this group a cell whose row and column hold no other published cell
        // comes straight back as `accuracy.n` minus the other column (STAT-5).
        walk.group(table);
    }

    // A day's trend rows for the three methods are one group.
    let mut days: Vec<(Date, Vec<usize>)> = Vec::new();
    for point in &mut report.trend {
        let index = walk.rate(&mut point.accuracy);
        match days.last_mut() {
            Some((date, members)) if *date == point.date => members.push(index),
            _ => days.push((point.date, vec![index])),
        }
    }
    for (_, members) in days {
        walk.group(members);
    }
}

/// Add a cell to the group keyed by `key`, creating it when it is new.
fn push_member<K: PartialEq>(groups: &mut Vec<(K, Vec<usize>)>, key: K, index: usize) {
    for (existing, members) in groups.iter_mut() {
        if *existing == key {
            members.push(index);
            return;
        }
    }
    groups.push((key, vec![index]));
}
