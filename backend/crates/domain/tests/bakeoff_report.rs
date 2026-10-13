//! T-908a bake-off report assembly: fixed streams and randomised properties.
//!
//! Proves CL-04 AC2 to AC5 and STAT-1, STAT-5, STAT-6 and STAT-8 on the pure
//! report builder in `domain::bakeoff::report`. Every expected value is either a
//! fixed reference from the task or the same T-907 helper called directly on the
//! same counts.
#![allow(
    clippy::unreadable_literal,
    clippy::excessive_precision,
    clippy::float_cmp,
    clippy::unnecessary_wraps,
    clippy::too_many_lines,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

use domain::bakeoff::labels::SwipeDirection;
use domain::bakeoff::paired::{
    cluster_bootstrap_diff, mcnemar_exact_p, BootstrapMethod, ParticipantPair,
};
use domain::bakeoff::prices::{default_prices, ModelPrices, PriceTable};
use domain::bakeoff::rates::Rate;
use domain::bakeoff::report::{
    build_report, BakeoffReport, EvalRow, Method, MethodReport, Prediction, PredictionError,
    ReportError, ReportQuery, SegmentReport, Segments, METHODS,
};
use domain::bakeoff::rng::SplitMix64;
use domain::bakeoff::{BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, MIN_CELL_SIZE};
use domain::MessageClass;
use std::collections::BTreeSet;
use time::{Date, Month, OffsetDateTime, Time};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The eight `EvalHeaderFacts` names the report segments by.
const FACT_NAMES: [&str; 8] = [
    "has_list_unsubscribe",
    "dkim_covers_list_unsubscribe",
    "has_list_unsubscribe_post",
    "has_list_id",
    "has_feedback_id",
    "precedence_bulk",
    "auto_submitted",
    "from_authenticated",
];

/// Every dimension `by_segment` covers (S7 5.13, CR-01a G4).
const DIMENSIONS: [&str; 13] = [
    "header_rules_class",
    "has_list_unsubscribe",
    "dkim_covers_list_unsubscribe",
    "has_list_unsubscribe_post",
    "has_list_id",
    "has_feedback_id",
    "precedence_bulk",
    "auto_submitted",
    "from_authenticated",
    "provider",
    "age_bucket",
    "text_tokens_bucket",
    "lang_is_english",
];

const SEGMENTS: Segments = Segments {
    facts: [
        ("has_list_unsubscribe", false),
        ("dkim_covers_list_unsubscribe", true),
        ("has_list_unsubscribe_post", false),
        ("has_list_id", false),
        ("has_feedback_id", false),
        ("precedence_bulk", false),
        ("auto_submitted", false),
        ("from_authenticated", true),
    ],
    provider: "gmail",
    age_bucket: "1y-5y",
    text_tokens_bucket: "<100",
    lang_is_english: true,
};

/// A calendar day in October 2026.
fn day(number: u8) -> Date {
    Date::from_calendar_date(2026, Month::October, number).unwrap_or(Date::MIN)
}

/// Midnight UTC on a day in October 2026.
fn midnight(number: u8) -> OffsetDateTime {
    day(number).midnight().assume_utc()
}

/// A precise UTC instant, so a test can look for a per-row timestamp in the JSON.
fn instant(number: u8, hour: u8, minute: u8, second: u8) -> OffsetDateTime {
    OffsetDateTime::new_utc(
        day(number),
        Time::from_hms(hour, minute, second).unwrap_or(Time::MIDNIGHT),
    )
}

/// A whole-October query with no version filter.
fn query() -> ReportQuery {
    ReportQuery {
        from: day(1),
        to: day(31),
        input_version: None,
        question_version: None,
        price_version: None,
        pool_versions: false,
    }
}

/// A Jev answer.
fn jev(class: MessageClass, latency_ms: u32, tokens: u32) -> Prediction {
    Prediction {
        class: Some(class),
        score: Some(70),
        confidence: Some(0.9),
        latency_ms,
        input_tokens: Some(tokens),
        error: None,
        classifier_id: "jev@1.13.0".to_owned(),
    }
}

/// A Gemini answer.
fn gemini(class: MessageClass, latency_ms: u32, tokens: u32) -> Prediction {
    Prediction {
        class: Some(class),
        score: Some(70),
        confidence: Some(0.9),
        latency_ms,
        input_tokens: Some(tokens),
        error: None,
        classifier_id: "gemini@flash-lite".to_owned(),
    }
}

/// A Jev call that timed out.
fn timeout() -> Prediction {
    Prediction {
        class: None,
        score: None,
        confidence: None,
        latency_ms: 2000,
        input_tokens: None,
        error: Some(PredictionError::Timeout),
        classifier_id: "jev@1.13.0".to_owned(),
    }
}

/// One evaluation row.
fn row(
    participant: u32,
    day_number: u8,
    hr_class: MessageClass,
    gemini: Option<Prediction>,
    jev: Option<Prediction>,
    direction: SwipeDirection,
    undone: bool,
) -> EvalRow {
    EvalRow {
        participant,
        created_at: midnight(day_number),
        header_rules: (hr_class, 80),
        gemini,
        jev,
        direction,
        undone,
        segments: SEGMENTS,
        input_version: "1".to_owned(),
        question_version: "1".to_owned(),
        price_version: "1".to_owned(),
    }
}

/// The same row in another age bucket.
fn aged(mut row: EvalRow, age_bucket: &'static str) -> EvalRow {
    row.segments.age_bucket = age_bucket;
    row
}

/// Build a report for the whole of October with the shipped price table.
fn report_of(rows: &[EvalRow]) -> Result<BakeoffReport, Box<dyn std::error::Error>> {
    Ok(build_report(rows, &query(), &default_prices())?)
}

/// The report row of one method.
fn model(report: &BakeoffReport, method: Method) -> &MethodReport {
    report
        .models
        .iter()
        .find(|model| model.model == method)
        .unwrap_or_else(|| panic!("{} is always reported", method.as_str()))
}

/// One segment cell.
fn segment<'a>(
    report: &'a BakeoffReport,
    method: Method,
    dimension: &str,
    value: &str,
) -> Option<&'a SegmentReport> {
    report
        .by_segment
        .iter()
        .find(|cell| cell.model == method && cell.dimension == dimension && cell.value == value)
}

/// A rate is published: value and bounds present.
fn shown(rate: Rate) -> bool {
    rate.value.is_some() && rate.lower.is_some() && rate.upper.is_some() && rate.n.is_some()
}

/// A rate is suppressed: every field null.
fn suppressed(rate: Rate) -> bool {
    rate == Rate::suppressed()
}

/// Add the equations one published rate gives a reader: its denominator and,
/// through its value, its numerator (`value * n` is the count it was built
/// from). A suppressed rate adds none.
fn add_rate_equations(
    rows: &mut Vec<([u64; 4], u64)>,
    rate: Rate,
    denom: [u64; 4],
    numer: [u64; 4],
) {
    let (Some(n), Some(value)) = (rate.n, rate.value) else {
        return;
    };
    rows.push((denom, n));
    let numerator = (value * n as f64).round();
    if numerator.is_finite() && numerator >= 0.0 {
        rows.push((numer, numerator as u64));
    }
}

/// Every assignment of the four `JunkConfusion` counts (`tp`, `fp`, `fn`, `tn`)
/// a reader can hold from one method's published rates, over non-negative
/// integers. A hidden count is recoverable exactly when the assignments agree
/// on it.
fn junk_solutions(method: &MethodReport) -> Vec<(u64, u64, u64, u64)> {
    let mut rows: Vec<([u64; 4], u64)> = Vec::new();
    add_rate_equations(&mut rows, method.accuracy, [1, 1, 1, 1], [1, 0, 0, 1]);
    add_rate_equations(&mut rows, method.junk_precision, [1, 1, 0, 0], [1, 0, 0, 0]);
    add_rate_equations(&mut rows, method.junk_recall, [1, 0, 1, 0], [1, 0, 0, 0]);
    add_rate_equations(
        &mut rows,
        method.false_junk_rate,
        [0, 1, 0, 1],
        [0, 1, 0, 0],
    );
    // The largest count the published rates bound the search by; with every rate
    // withheld it is the fallback this test fixes.
    let bound = method
        .accuracy
        .n
        .or(method.junk_precision.n)
        .unwrap_or(16)
        .max(1);
    let mut solutions = Vec::new();
    for tp in 0..=bound {
        for fp in 0..=bound {
            for fn_ in 0..=bound {
                for tn in 0..=bound {
                    let counts = [tp, fp, fn_, tn];
                    if counts.iter().sum::<u64>() > bound {
                        continue;
                    }
                    if rows.iter().all(|(coeff, rhs)| {
                        coeff
                            .iter()
                            .zip(counts)
                            .map(|(coefficient, count)| coefficient * count)
                            .sum::<u64>()
                            == *rhs
                    }) {
                        solutions.push((tp, fp, fn_, tn));
                    }
                }
            }
        }
    }
    solutions
}

#[test]
fn stat_1_labels_and_rates_end_to_end() -> TestResult {
    // T-907's reference stream for Jev: TP 6, FP 2, FN 3, TN 9, plus 3 skips and
    // 2 timeouts. Junk is `list`; wanted is `personal`.
    let mut rows = Vec::new();
    let verdicts = [
        (MessageClass::List, SwipeDirection::Left, 6_u32),
        (MessageClass::List, SwipeDirection::Right, 2),
        (MessageClass::Personal, SwipeDirection::Left, 3),
        (MessageClass::Personal, SwipeDirection::Right, 9),
    ];
    let mut participant = 1;
    for (class, direction, count) in verdicts {
        for _ in 0..count {
            rows.push(row(
                participant,
                4,
                MessageClass::List,
                None,
                Some(jev(class, 150, 500)),
                direction,
                false,
            ));
            participant += 1;
        }
    }
    for _ in 0..3 {
        // Down is not a label: the skip is dropped from every accuracy figure.
        rows.push(row(
            participant,
            4,
            MessageClass::List,
            None,
            Some(jev(MessageClass::List, 150, 500)),
            SwipeDirection::Down,
            false,
        ));
        participant += 1;
    }
    for _ in 0..2 {
        rows.push(row(
            participant,
            4,
            MessageClass::List,
            None,
            Some(timeout()),
            SwipeDirection::Left,
            false,
        ));
        participant += 1;
    }

    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.cards, Some(25));
    assert_eq!(jev.valid_answers, Some(23));
    assert_eq!(report.labelled_swipes, Some(22));
    // Skips are out and the timeouts are out of accuracy but in availability.
    assert_eq!(jev.accuracy.n, Some(20));
    assert_eq!(jev.accuracy.value, Some(0.75));
    assert_eq!(jev.timeout_rate.n, Some(25));
    assert_eq!(jev.timeout_rate.value, Some(0.08));
    assert_eq!(jev.error_rate.value, Some(0.0));
    Ok(())
}

#[test]
fn stat_1_undo_flips_label() -> TestResult {
    // A left swipe undone counts as wanted, so every card is misclassified by a
    // model that answered `list`.
    let rows: Vec<EvalRow> = (0..12)
        .map(|index| {
            row(
                index,
                5,
                MessageClass::List,
                None,
                Some(jev(MessageClass::List, 150, 500)),
                SwipeDirection::Left,
                true,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.accuracy.n, Some(12));
    assert_eq!(jev.accuracy.value, Some(0.0));
    // Every card is labelled wanted, so no card is labelled junk: `junk_recall`
    // has a denominator of zero and is suppressed, and `false_junk_rate` is its
    // complementary sibling under `accuracy.n = junk_recall.n +
    // false_junk_rate.n`, so it is withheld too instead of handing the hidden
    // count back as a subtraction (STAT-5).
    assert!(suppressed(jev.junk_recall));
    assert!(suppressed(jev.false_junk_rate));
    Ok(())
}

#[test]
fn stat_1_header_rules_is_a_method() -> TestResult {
    let rows: Vec<EvalRow> = (0..12)
        .map(|index| {
            row(
                index,
                6,
                MessageClass::List,
                None,
                None,
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    let header = model(&report, Method::HeaderRules);
    assert_eq!(header.classifier_id, "header_rules@1");
    assert_eq!(header.cards, Some(12));
    assert_eq!(header.accuracy.value, Some(1.0));
    assert_eq!(header.cost_per_1000_messages_usd, Some(0.0));
    assert_eq!(header.input_tokens, Some(0));
    // Nothing failed, and latency is measured in process.
    assert_eq!(header.timeout_rate.value, Some(0.0));
    assert_eq!(header.error_rate.value, Some(0.0));
    assert_eq!(header.latency.p50_ms, Some(0));
    // bulk_score / 100 is the confidence the calibration curve reads.
    let ece = header.calibration.ece.unwrap_or(f64::NAN);
    assert!((ece - 0.2).abs() < 1e-9, "ECE is |1.0 - 0.8|: {ece}");
    let bin = header.calibration.bins[8];
    assert_eq!(bin.confidence_from, 0.8);
    assert_eq!(bin.confidence_to, 0.9);
    assert_eq!(bin.count, Some(12));
    assert_eq!(bin.observed_accuracy, Some(1.0));
    Ok(())
}

#[test]
fn stat_5_segment_suppression() -> TestResult {
    // One dimension with 11, 10 and 4 labelled rows: 4 is suppressed, and the
    // next smallest cell goes with it so the total cannot give it back.
    let mut rows = Vec::new();
    let mut participant = 1;
    for (age_bucket, count) in [("1y-5y", 11_u32), ("7d-90d", 10), ("<7d", 4)] {
        for _ in 0..count {
            rows.push(aged(
                row(
                    participant,
                    7,
                    MessageClass::List,
                    None,
                    Some(jev(MessageClass::List, 150, 500)),
                    SwipeDirection::Left,
                    false,
                ),
                age_bucket,
            ));
            participant += 1;
        }
    }
    let report = report_of(&rows)?;
    let small =
        segment(&report, Method::Jev, "age_bucket", "<7d").unwrap_or_else(|| panic!("cell"));
    let next =
        segment(&report, Method::Jev, "age_bucket", "7d-90d").unwrap_or_else(|| panic!("cell"));
    let kept =
        segment(&report, Method::Jev, "age_bucket", "1y-5y").unwrap_or_else(|| panic!("cell"));
    assert!(suppressed(small.accuracy), "4 rows must be suppressed");
    assert!(suppressed(small.false_junk_rate));
    assert!(
        suppressed(next.accuracy),
        "the next smallest cell goes with it"
    );
    assert!(suppressed(next.false_junk_rate));
    assert_eq!(kept.accuracy.n, Some(11));
    assert!(shown(kept.accuracy));
    Ok(())
}

#[test]
fn stat_5_trend_and_histogram_suppressed() -> TestResult {
    // 48 labelled cards on one day. Gemini ran on 3 of them, so its trend point
    // is suppressed and the day's next smallest trend count goes with it. Jev's
    // latencies land in two small bins, which are suppressed.
    let mut rows = Vec::new();
    for index in 0..48_u32 {
        let latency = match index {
            0..=19 => 50,
            20..=34 => 150,
            35..=44 => 300,
            _ => 700,
        };
        let gemini = (index < 3).then(|| gemini(MessageClass::List, 400, 500));
        rows.push(row(
            index,
            8,
            MessageClass::List,
            gemini,
            Some(jev(MessageClass::List, latency, 500)),
            SwipeDirection::Left,
            false,
        ));
    }
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    let bins: Vec<Option<u64>> = jev.latency.histogram.iter().map(|bin| bin.count).collect();
    assert_eq!(bins, vec![Some(20), Some(15), Some(10), None, None]);
    let hidden = report
        .trend
        .iter()
        .filter(|point| point.accuracy.n.is_none())
        .count();
    assert_eq!(hidden, 2, "the small trend cell and its neighbour");
    let gemini_point = report
        .trend
        .iter()
        .find(|point| point.model == Method::Gemini)
        .unwrap_or_else(|| panic!("gemini trend point"));
    assert!(gemini_point.accuracy.n.is_none());
    Ok(())
}

#[test]
fn stat_5_single_answered_call_publishes_no_record_value() -> TestResult {
    // One answered Jev call: the answered-call count is below the minimum, so
    // the latency percentiles, the token sum and the cost that describe that one
    // record are withheld with it (STAT-5).
    let rows = vec![row(
        0,
        25,
        MessageClass::List,
        None,
        Some(jev(MessageClass::List, 150, 500)),
        SwipeDirection::Left,
        false,
    )];
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.cards, None);
    assert_eq!(jev.valid_answers, None);
    assert_eq!(jev.latency.p50_ms, None);
    assert_eq!(jev.latency.p95_ms, None);
    assert_eq!(jev.input_tokens, None);
    assert_eq!(jev.cost_per_1000_messages_usd, None);
    // Header rules answer in process with a latency of zero, and that zero is
    // still a per-record value: it goes with the count too.
    let header = model(&report, Method::HeaderRules);
    assert_eq!(header.latency.p50_ms, None);
    assert_eq!(header.cost_per_1000_messages_usd, None);
    Ok(())
}

#[test]
fn stat_5_suppressed_label_column_is_not_the_difference_of_the_totals() -> TestResult {
    // 12 labelled cards, 10 junk and 2 wanted, every one answered `list`: the
    // junk-recall denominator is 10 and the false-junk denominator is 2, which is
    // below the minimum. The precision would hand the hidden count back on its
    // own: a shown rate publishes its denominator and its numerator (`value *
    // n`), so the accuracy and the precision together separate all four counts.
    // The accuracy is no safer: its numerator is `10 * 12 / 12`, the number of
    // cards the label mapping calls correct, which is the `list`/`junk` cell —
    // the only cell of the table's diagonal — and with it the hidden wanted
    // count comes back as `12 - 10`. The accuracy goes with its cell, which
    // takes the junk rates and the calibration error with it (STAT-5).
    let rows: Vec<EvalRow> = (0..12_u32)
        .map(|index| {
            let direction = if index < 10 {
                SwipeDirection::Left
            } else {
                SwipeDirection::Right
            };
            row(
                index,
                26,
                MessageClass::List,
                None,
                Some(jev(MessageClass::List, 150, 500)),
                direction,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    // The count the accuracy is built on is withheld, so the rate, the rates
    // read beside it and the error they feed all go.
    assert!(suppressed(jev.accuracy));
    assert!(suppressed(jev.junk_precision));
    assert!(suppressed(jev.junk_recall));
    assert!(suppressed(jev.false_junk_rate));
    assert_eq!(jev.junk_f1, None);
    assert_eq!(jev.calibration.ece, None);
    // The confusion cells go with it: no published number adds the hidden
    // wanted count back up.
    let confusion: Vec<Option<u64>> = report
        .confusion
        .iter()
        .filter(|cell| cell.model == Method::Jev)
        .map(|cell| cell.count)
        .collect();
    assert!(
        !confusion.is_empty() && confusion.iter().all(Option::is_none),
        "the confusion cells go with the accuracy: {confusion:?}"
    );
    // Nothing built on those labels is published: no rate the hidden cell could
    // be read off is left standing, in the segments, the day's trend or the
    // calibration curve.
    assert!(report
        .by_segment
        .iter()
        .filter(|cell| cell.model == Method::Jev)
        .all(|cell| suppressed(cell.accuracy) && suppressed(cell.false_junk_rate)));
    assert!(report
        .trend
        .iter()
        .filter(|point| point.model == Method::Jev)
        .all(|point| suppressed(point.accuracy)));
    assert!(jev
        .calibration
        .bins
        .iter()
        .all(|bin| bin.observed_accuracy.is_none()));
    // Nothing published determines the hidden counts: the false-junk
    // denominator takes more than one value across the assignments a reader
    // could hold.
    let solutions = junk_solutions(jev);
    assert!(
        solutions.len() >= 2,
        "the confusion counts must stay ambiguous: {solutions:?}"
    );
    let hidden_wanted: BTreeSet<u64> = solutions.iter().map(|(_, fp, _, tn)| fp + tn).collect();
    assert!(
        hidden_wanted.len() >= 2,
        "the suppressed false-junk denominator is recoverable: {hidden_wanted:?}"
    );
    // The reader's assignments, from the numbers the report still publishes: the
    // two present cells share the labelled answers it shows, and when the
    // accuracy is shown its numerator is the `list`/`junk` cell, which would fix
    // the wanted count as `12 - 10`. Only one split survives if that leaks, so
    // this fails the moment the accuracy comes back.
    let total = jev.valid_answers.unwrap_or_default();
    let numerator = jev
        .accuracy
        .value
        .map(|value| (value * total as f64).round() as u64);
    let mut splits = BTreeSet::new();
    for junk in 0..=total {
        if numerator.is_some_and(|count| count != junk) {
            continue;
        }
        splits.insert((junk, total - junk));
    }
    assert!(
        splits.len() >= 2,
        "the confusion cells must stay ambiguous: {splits:?}"
    );
    Ok(())
}

#[test]
fn stat_5_values_go_with_the_counts_they_are_built_on() -> TestResult {
    // 40 paired cards where the models disagree twice: `n_paired` is published
    // and the discordant counts are below the minimum. McNemar reads those two
    // counts and is withheld with them; the accuracy difference reads the paired
    // cards and stays, beside the count it is computed over (STAT-5).
    let mut rows = Vec::new();
    for index in 0..40_u32 {
        let (gemini_class, jev_class) = match index {
            38 => (MessageClass::List, MessageClass::Personal),
            39 => (MessageClass::Personal, MessageClass::List),
            _ => (MessageClass::List, MessageClass::List),
        };
        rows.push(row(
            index,
            28,
            MessageClass::List,
            Some(gemini(gemini_class, 400, 500)),
            Some(jev(jev_class, 150, 500)),
            SwipeDirection::Left,
            false,
        ));
    }
    let report = report_of(&rows)?;
    let pair = report
        .paired
        .iter()
        .find(|pair| pair.model_a == Method::Gemini && pair.model_b == Method::Jev)
        .unwrap_or_else(|| panic!("gemini and jev are compared"));
    assert_eq!(pair.n_paired, Some(40));
    assert_eq!(pair.only_a_correct, None);
    assert_eq!(pair.only_b_correct, None);
    assert_eq!(pair.mcnemar_exact_p, None);
    // The difference hands the suppressed discordant counts back (`a + b` is
    // known from the paired totals, and `a - b` is the difference), so it goes
    // with them (STAT-5).
    assert_eq!(pair.accuracy_difference.value, None);
    assert_eq!(pair.accuracy_difference.lower, None);
    assert_eq!(pair.accuracy_difference.upper, None);
    assert_eq!(pair.bootstrap.participants, Some(40));
    Ok(())
}

#[test]
fn stat_5_paired_difference_is_withheld_with_the_discordant_counts() -> TestResult {
    // 31 paired cards: 20 both right, 10 neither, 1 only Gemini. Every card is
    // labelled and both models answered all of them, so each model's published
    // correct count is `only + both` and the paired total is their sum: the
    // marginals beside `both` would fix the hidden discordants exactly
    // (`21 - 20` and `20 - 20`). The difference and its interval are withheld
    // with the counts they would hand back, and so are the two agreement cells
    // the marginals would add up with, so no route fixes the discordants
    // (STAT-5).
    let rows: Vec<EvalRow> = (0..31_u32)
        .map(|index| {
            let (gemini_class, jev_class) = match index {
                0 => (MessageClass::List, MessageClass::Personal),
                1..=20 => (MessageClass::List, MessageClass::List),
                _ => (MessageClass::Personal, MessageClass::Personal),
            };
            // Gemini answers `list` on 21 cards, Jev on 20, so only Gemini is
            // right on card 0.
            row(
                index,
                29,
                MessageClass::List,
                Some(gemini(gemini_class, 400, 500)),
                Some(jev(jev_class, 150, 500)),
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    let pair = report
        .paired
        .iter()
        .find(|pair| pair.model_a == Method::Gemini && pair.model_b == Method::Jev)
        .unwrap_or_else(|| panic!("gemini and jev are compared"));
    assert_eq!(pair.n_paired, Some(31));
    assert_eq!(pair.only_a_correct, None);
    assert_eq!(pair.only_b_correct, None);
    assert_eq!(pair.both_correct, None, "the marginals would fix them");
    assert_eq!(pair.neither_correct, None);
    assert_eq!(pair.mcnemar_exact_p, None);
    assert_eq!(pair.accuracy_difference.value, None);
    assert_eq!(pair.accuracy_difference.lower, None);
    assert_eq!(pair.accuracy_difference.upper, None);
    // The marginals the reader still holds, 21 of 31 and 20 of 31, leave more
    // than one split standing: the discordant counts are not fixed. Everything
    // the report does publish about the pair is folded in, so this fails if a
    // cell that would fix the split is shown again.
    let gemini = model(&report, Method::Gemini);
    let jev = model(&report, Method::Jev);
    assert_eq!(gemini.accuracy.value, Some(21.0 / 31.0));
    assert_eq!(jev.accuracy.value, Some(20.0 / 31.0));
    let mut splits = BTreeSet::new();
    for only_a in 0..=31_u64 {
        let Some(both) = 21_u64.checked_sub(only_a) else {
            continue;
        };
        if pair.both_correct.is_some_and(|count| count != both) {
            continue;
        }
        let Some(only_b) = 20_u64.checked_sub(both) else {
            continue;
        };
        let Some(neither) = 31_u64.checked_sub(only_a + only_b + both) else {
            continue;
        };
        if pair.neither_correct.is_some_and(|count| count != neither) {
            continue;
        }
        splits.insert((only_a, only_b));
    }
    assert!(
        splits.len() >= 2,
        "the discordant counts must stay ambiguous: {splits:?}"
    );
    Ok(())
}

#[test]
fn stat_5_calibration_error_is_withheld_with_a_suppressed_bin() -> TestResult {
    // 11 labelled cards, all predicted `personal` and all wrong: ten stated
    // 0.09 and one stated 1.0. The last bin holds a single point and is
    // suppressed, and every wrong point makes the expected calibration error
    // its mean confidence, so the published error would fix that hidden count
    // (the first ten points cannot lift it above 0.1 on their own). The error
    // is withheld with the bin it would hand back (STAT-5).
    let rows: Vec<EvalRow> = (0..11_u32)
        .map(|index| {
            let confidence = if index < 10 { 0.09 } else { 1.0 };
            let mut prediction = jev(MessageClass::Personal, 150, 500);
            prediction.confidence = Some(confidence);
            row(
                index,
                30,
                MessageClass::List,
                None,
                Some(prediction),
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.accuracy.n, Some(11), "the labelled total stays");
    assert_eq!(jev.calibration.bins[0].count, Some(10));
    assert_eq!(jev.calibration.bins[9].count, None, "one point is hidden");
    assert_eq!(jev.calibration.ece, None);
    Ok(())
}

#[test]
fn stat_4_participant_count_below_minimum_is_hidden() -> TestResult {
    // Twelve labels all from one participant: the labels clear the minimum but
    // the participant count itself does not, so the count and the
    // largest-contributor share are withheld with it (STAT-4). Every paired
    // bootstrap resamples those same participants, so its participant count is
    // an independently suppressible count of its own and the interval it
    // produced goes with it.
    let rows: Vec<EvalRow> = (0..12_u32)
        .map(|_| {
            row(
                7,
                27,
                MessageClass::List,
                Some(gemini(MessageClass::List, 400, 500)),
                Some(jev(MessageClass::List, 150, 500)),
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    assert_eq!(report.labelled_swipes, Some(12));
    assert_eq!(report.participants.count, None);
    assert_eq!(
        report.participants.top_contributor_share,
        Some(Rate::suppressed()),
        "the share goes with the count it is read against"
    );
    assert_eq!(report.paired.len(), 3);
    for pair in &report.paired {
        assert_eq!(
            pair.bootstrap.participants, None,
            "one participant must not be published by the paired bootstrap"
        );
    }
    Ok(())
}

#[test]
fn stat_5_segment_denominator_is_not_the_difference_of_the_totals() -> TestResult {
    // 32 Jev answers, all `list`: the gmail segment has 20 labelled cards and
    // 10 wanted, the graph segment 12 and 2. The model's own false-junk
    // denominator is 12 and gmail's is 10, so publishing gmail's beside the
    // model's would hand back graph's suppressed 2 as the difference: the
    // false-junk denominators are a complementary group of their own (STAT-5).
    let mut rows: Vec<EvalRow> = Vec::new();
    for index in 0..32_u32 {
        let (provider, direction) = match index {
            0..=9 => ("gmail", SwipeDirection::Left),
            10..=19 => ("gmail", SwipeDirection::Right),
            20..=29 => ("graph", SwipeDirection::Left),
            _ => ("graph", SwipeDirection::Right),
        };
        let mut evaluation = row(
            index,
            31,
            MessageClass::List,
            None,
            Some(jev(MessageClass::List, 150, 500)),
            direction,
            false,
        );
        evaluation.segments.provider = provider;
        rows.push(evaluation);
    }
    let report = report_of(&rows)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.false_junk_rate.n, Some(12), "the model total stays");
    let gmail = segment(&report, Method::Jev, "provider", "gmail")
        .unwrap_or_else(|| panic!("gmail segment"));
    let graph = segment(&report, Method::Jev, "provider", "graph")
        .unwrap_or_else(|| panic!("graph segment"));
    assert!(suppressed(graph.false_junk_rate));
    assert!(
        suppressed(gmail.false_junk_rate),
        "the sibling goes with the hidden segment cell"
    );
    // The segment accuracy is no safer than its false-junk rate: its numerator
    // is the segment's correct count, and with every prediction `list` the
    // wanted count comes back as `n` minus the numerator (`12 - 10` for graph).
    // The accuracies are withheld with the hidden denominators, while the
    // model's own accuracy stays: its confusion holds no hidden count (STAT-5).
    assert!(suppressed(graph.accuracy));
    assert!(suppressed(gmail.accuracy));
    assert!(shown(jev.accuracy));
    // Every provider denominator is hidden beside the published model total, so
    // the hidden one is ambiguous: any split of 12 into two non-negative counts
    // fits, not the single value 2 the subtraction would give.
    let published: Vec<Option<u64>> = report
        .by_segment
        .iter()
        .filter(|cell| cell.model == Method::Jev && cell.dimension == "provider")
        .map(|cell| cell.false_junk_rate.n)
        .collect();
    assert_eq!(published.len(), 2);
    assert!(published.iter().all(Option::is_none));
    Ok(())
}

/// A group's suppressed cells must number zero or at least two (S7 5.13).
fn assert_group(values: &[Option<u64>]) {
    let hidden = values.iter().filter(|value| value.is_none()).count();
    assert!(
        hidden == 0 || hidden >= 2,
        "a group shows its total with exactly one hidden cell: {values:?}"
    );
}

/// Every complementary group of the report, rebuilt from the published shape.
fn check_groups(report: &BakeoffReport) {
    for method in &report.models {
        let bins: Vec<Option<u64>> = method
            .latency
            .histogram
            .iter()
            .map(|bin| bin.count)
            .collect();
        assert_group(&bins);
        for dimension in DIMENSIONS {
            let cells: Vec<Option<u64>> = report
                .by_segment
                .iter()
                .filter(|cell| cell.model == method.model && cell.dimension == dimension)
                .map(|cell| cell.accuracy.n)
                .collect();
            if cells.len() >= 2 {
                assert_group(&cells);
            }
        }
        for class in MessageClass::ALL {
            let row: Vec<Option<u64>> = report
                .confusion
                .iter()
                .filter(|cell| cell.model == method.model && cell.class == class)
                .map(|cell| cell.count)
                .collect();
            if row.len() >= 2 {
                assert_group(&row);
            }
        }
        for label in ["junk", "wanted"] {
            let column: Vec<Option<u64>> = report
                .confusion
                .iter()
                .filter(|cell| cell.model == method.model && cell.label == label)
                .map(|cell| cell.count)
                .collect();
            if column.len() >= 2 {
                assert_group(&column);
            }
        }
    }
    for pair in &report.paired {
        assert_group(&[
            pair.only_a_correct,
            pair.only_b_correct,
            pair.both_correct,
            pair.neither_correct,
        ]);
    }
    let mut dates: Vec<Date> = report.trend.iter().map(|point| point.date).collect();
    dates.dedup();
    for date in dates {
        let cells: Vec<Option<u64>> = report
            .trend
            .iter()
            .filter(|point| point.date == date)
            .map(|point| point.accuracy.n)
            .collect();
        if cells.len() >= 2 {
            assert_group(&cells);
        }
    }
}

/// A random stream of evaluation rows, derived from one seed.
fn random_stream(seed: u64) -> Vec<EvalRow> {
    let classes = [
        MessageClass::List,
        MessageClass::BulkNoHeader,
        MessageClass::Notice,
        MessageClass::Personal,
        MessageClass::Suspect,
    ];
    let directions = [
        SwipeDirection::Left,
        SwipeDirection::Right,
        SwipeDirection::Up,
        SwipeDirection::Down,
    ];
    let buckets = ["<7d", "7d-90d", "90d-1y", "1y-5y", ">5d"];
    let mut rng = SplitMix64::new(seed);
    let length = rng.below(40);
    let mut rows = Vec::new();
    for _ in 0..length {
        let participant = rng.below(6) as u32;
        let day_number = 1 + rng.below(5) as u8;
        let hr_class = classes[rng.below(5) as usize];
        let direction = directions[rng.below(4) as usize];
        let undone = rng.below(2) == 0;
        let gemini = (rng.below(3) > 0).then(|| gemini(classes[rng.below(5) as usize], 400, 500));
        let jev = (rng.below(4) > 0).then(|| jev(classes[rng.below(5) as usize], 150, 500));
        let bucket = buckets[rng.below(5) as usize];
        rows.push(aged(
            row(
                participant,
                day_number,
                hr_class,
                gemini,
                jev,
                direction,
                undone,
            ),
            bucket,
        ));
    }
    rows
}

#[test]
fn stat_5_no_value_recoverable_from_totals() -> TestResult {
    // A randomised property over random streams: no group shows its total with
    // exactly one suppressed cell, so no hidden count is the difference of the
    // rest. The seed is fixed, so a failure is reproducible.
    for seed in 0..8_u64 {
        let rows = random_stream(seed);
        let report = report_of(&rows)?;
        check_groups(&report);
    }
    Ok(())
}

#[test]
fn stat_6_mixed_question_versions_refused() -> TestResult {
    let rows = mixed_question_rows();
    match build_report(&rows, &query(), &default_prices()) {
        Err(ReportError::VersionsMixed { input, question }) => {
            assert_eq!(input, vec!["1".to_owned()]);
            assert_eq!(question, vec!["1".to_owned(), "2".to_owned()]);
        }
        other => panic!("expected VersionsMixed, got {other:?}"),
    }
    Ok(())
}

#[test]
fn stat_6_pool_versions_allowed() -> TestResult {
    let rows = mixed_question_rows();
    let mut pooled = query();
    pooled.pool_versions = true;
    let report = build_report(&rows, &pooled, &default_prices())?;
    assert!(report.query.pool_versions);
    assert_eq!(
        report.versions_present.question,
        vec!["1".to_owned(), "2".to_owned()]
    );
    assert_eq!(report.labelled_swipes, Some(20));
    Ok(())
}

#[test]
fn stat_6_filter_reports_one_version() -> TestResult {
    let rows = mixed_question_rows();
    let mut filtered = query();
    filtered.question_version = Some("2".to_owned());
    let report = build_report(&rows, &filtered, &default_prices())?;
    assert_eq!(report.versions_present.question, vec!["2".to_owned()]);
    assert_eq!(report.labelled_swipes, Some(10));
    Ok(())
}

/// Ten rows on question version 1 and ten on question version 2.
fn mixed_question_rows() -> Vec<EvalRow> {
    let mut rows = Vec::new();
    for index in 0..20_u32 {
        let mut evaluation = row(
            index,
            9,
            MessageClass::List,
            None,
            Some(jev(MessageClass::List, 150, 500)),
            SwipeDirection::Left,
            false,
        );
        let version = if index < 10 { "1" } else { "2" };
        evaluation.question_version = String::from(version);
        rows.push(evaluation);
    }
    rows
}

#[test]
fn stat_6_mixed_price_versions_priced_separately() -> TestResult {
    // Two price versions, priced by their own tables: 0.01 + 0.02 USD over 20
    // answered cards is 1.5 USD per 1,000 messages.
    let mut rows = Vec::new();
    for index in 0..20_u32 {
        let mut evaluation = row(
            index,
            10,
            MessageClass::List,
            None,
            Some(jev(MessageClass::List, 150, 1000)),
            SwipeDirection::Left,
            false,
        );
        evaluation.price_version = if index < 10 { "1" } else { "2" }.to_owned();
        rows.push(evaluation);
    }
    let mut versions = PriceTable::new();
    for (version, jev_price) in [("1", 1.0), ("2", 2.0)] {
        versions.versions.insert(
            version.to_owned(),
            ModelPrices {
                gemini_usd_per_million_input: 0.5,
                jev_usd_per_million_input: jev_price,
            },
        );
    }
    let report = build_report(&rows, &query(), &versions)?;
    let jev = model(&report, Method::Jev);
    assert_eq!(jev.input_tokens, Some(20000));
    let cost = jev.cost_per_1000_messages_usd.unwrap_or(f64::NAN);
    assert!(
        (cost - 1.5).abs() < 1e-9,
        "two price versions must be priced by their own tables: {cost}"
    );
    assert_eq!(
        report.versions_present.price,
        vec!["1".to_owned(), "2".to_owned()]
    );
    Ok(())
}

#[test]
fn stat_8_trend_points_have_n_and_interval() -> TestResult {
    // Three days, three methods, 30 labelled cards a day; Jev's latencies fill
    // all five bins, so they still sum to the answered calls.
    let mut rows = Vec::new();
    let mut participant = 1;
    for day_number in [11_u8, 12, 13] {
        for index in 0..30_u32 {
            let latency = match index % 5 {
                0 => 50,
                1 => 150,
                2 => 300,
                3 => 700,
                _ => 1_500,
            };
            rows.push(row(
                participant,
                day_number,
                MessageClass::List,
                Some(gemini(MessageClass::List, 400, 500)),
                Some(jev(MessageClass::List, latency, 500)),
                SwipeDirection::Left,
                false,
            ));
            participant += 1;
        }
    }
    let report = report_of(&rows)?;
    assert_eq!(report.trend.len(), 9);
    for point in &report.trend {
        assert_eq!(point.accuracy.n, Some(30), "{}", point.date);
        assert!(shown(point.accuracy));
    }
    let jev = model(&report, Method::Jev);
    let total: u64 = jev
        .latency
        .histogram
        .iter()
        .map(|bin| bin.count.unwrap_or(0))
        .sum();
    assert_eq!(Some(total), jev.valid_answers);
    assert_eq!(total, 90);
    Ok(())
}

#[test]
fn cl_04_ac2_report_has_three_methods_and_all_sections() -> TestResult {
    // 40 junk cards and 20 wanted cards, every method answering `list`.
    let mut rows = Vec::new();
    let mut participant = 1;
    for (direction, count) in [(SwipeDirection::Left, 40_u32), (SwipeDirection::Right, 20)] {
        for _ in 0..count {
            rows.push(row(
                participant,
                14,
                MessageClass::List,
                Some(gemini(MessageClass::List, 400, 500)),
                Some(jev(MessageClass::List, 150, 500)),
                direction,
                false,
            ));
            participant += 1;
        }
    }
    let report = report_of(&rows)?;
    assert_eq!(report.min_cell_size, MIN_CELL_SIZE);
    assert_eq!(report.ground_truth_version, "1");
    assert_eq!(report.interval_method, "wilson_95");
    assert_eq!(report.query, query());
    assert_eq!(report.versions_present.input, vec!["1".to_owned()]);
    assert_eq!(
        report
            .models
            .iter()
            .map(|model| model.model)
            .collect::<Vec<Method>>(),
        METHODS.to_vec()
    );
    for method in METHODS {
        let model = model(&report, method);
        assert_eq!(model.cards, Some(60));
        assert_eq!(model.valid_answers, Some(60));
        assert_eq!(model.accuracy.n, Some(60));
        assert_eq!(model.accuracy.value, Some(40.0 / 60.0));
        // Every card is predicted junk: precision is 40 of 60, recall is 1.
        assert_eq!(model.junk_precision.n, Some(60));
        assert_eq!(model.junk_precision.value, Some(40.0 / 60.0));
        assert_eq!(model.junk_recall.n, Some(40));
        assert_eq!(model.junk_recall.value, Some(1.0));
        assert!(model.junk_f1.is_some());
        assert_eq!(model.false_junk_rate.value, Some(1.0));
        assert!(model.calibration.ece.is_some());
        assert_eq!(model.latency.histogram.len(), 5);
        assert!(model.cost_per_1000_messages_usd.is_some());
        assert!(model.classifier_id.contains('@'), "{}", model.classifier_id);
    }
    assert_eq!(report.paired.len(), 3);
    assert_eq!(report.agreement.len(), 3);
    for agreement in &report.agreement {
        assert_eq!(agreement.rate.value, Some(1.0));
        assert_eq!(agreement.rate.n, Some(60));
    }
    assert!(report.confusion.iter().any(|cell| cell.count.is_some()));
    assert!(report
        .by_segment
        .iter()
        .any(|cell| cell.model == Method::Jev));
    assert!(report.trend.iter().any(|point| point.model == Method::Jev));
    Ok(())
}

#[test]
fn cl_04_ac3_paired_uses_mcnemar_and_cluster_bootstrap() -> TestResult {
    // Twelve participants, four paired cards each: equal discordant counts and
    // twelve in every cell of the four-way split.
    let (rows, parts, only_a, only_b) = paired_stream(12, 4);
    let report = report_of(&rows)?;
    let pair = report
        .paired
        .iter()
        .find(|pair| pair.model_a == Method::Gemini && pair.model_b == Method::Jev)
        .unwrap_or_else(|| panic!("gemini and jev are compared"));
    let expected = cluster_bootstrap_diff(&parts, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    assert_eq!(pair.only_a_correct, Some(only_a));
    assert_eq!(pair.only_b_correct, Some(only_b));
    assert_eq!(pair.n_paired, Some(48));
    assert_eq!(pair.mcnemar_exact_p, Some(mcnemar_exact_p(only_a, only_b)));
    assert_eq!(pair.accuracy_difference.value, expected.value);
    assert_eq!(pair.accuracy_difference.lower, expected.lower);
    assert_eq!(pair.accuracy_difference.upper, expected.upper);
    assert_eq!(pair.bootstrap.method, BootstrapMethod::ClusterByParticipant);
    assert_eq!(pair.bootstrap.resamples, BOOTSTRAP_RESAMPLES);
    assert_eq!(pair.bootstrap.seed, BOOTSTRAP_SEED);
    assert_eq!(pair.bootstrap.participants, Some(12));
    Ok(())
}

#[test]
fn cl_04_ac3_insufficient_participants_nulls_interval() -> TestResult {
    let (rows, _, _, _) = paired_stream(4, 8);
    let report = report_of(&rows)?;
    let pair = report
        .paired
        .iter()
        .find(|pair| pair.model_a == Method::Gemini && pair.model_b == Method::Jev)
        .unwrap_or_else(|| panic!("gemini and jev are compared"));
    assert_eq!(
        pair.bootstrap.method,
        BootstrapMethod::InsufficientParticipants
    );
    // Four participants is below the minimum, like every other count.
    assert_eq!(pair.bootstrap.participants, None);
    // The discordant counts are below the minimum too, so the difference that
    // would hand them back is withheld with them (STAT-5).
    assert_eq!(pair.accuracy_difference.value, None);
    assert_eq!(pair.accuracy_difference.lower, None);
    assert_eq!(pair.accuracy_difference.upper, None);
    Ok(())
}

/// `participants` participants with `each` paired cards apiece. Every fourth
/// card goes only to Gemini, only to Jev, to both, or to neither, so each
/// discordant count is `participants` and every count stays publishable.
fn paired_stream(participants: u32, each: u32) -> (Vec<EvalRow>, Vec<ParticipantPair>, u64, u64) {
    let mut rows = Vec::new();
    let mut parts = Vec::new();
    for participant in 0..participants {
        let mut a_correct = 0;
        let mut b_correct = 0;
        for index in 0..each {
            let a_right = index % 4 == 0 || index % 4 == 2;
            let b_right = index % 4 == 1 || index % 4 == 2;
            if a_right {
                a_correct += 1;
            }
            if b_right {
                b_correct += 1;
            }
            let class = |right: bool| {
                if right {
                    MessageClass::List
                } else {
                    MessageClass::Personal
                }
            };
            rows.push(row(
                participant,
                20,
                MessageClass::List,
                Some(gemini(class(a_right), 400, 500)),
                Some(jev(class(b_right), 150, 500)),
                SwipeDirection::Left,
                false,
            ));
        }
        parts.push(ParticipantPair {
            n: u64::from(each),
            a_correct,
            b_correct,
        });
    }
    let only_a = u64::from(participants);
    let only_b = u64::from(participants);
    (rows, parts, only_a, only_b)
}

#[test]
fn cl_04_ac4_segments_cover_every_dimension() -> TestResult {
    let rows: Vec<EvalRow> = (0..15_u32)
        .map(|index| {
            row(
                index,
                21,
                MessageClass::List,
                Some(gemini(MessageClass::List, 400, 500)),
                Some(jev(MessageClass::List, 150, 500)),
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    let report = report_of(&rows)?;
    for method in METHODS {
        let mut dimensions: Vec<String> = report
            .by_segment
            .iter()
            .filter(|cell| cell.model == method)
            .map(|cell| cell.dimension.clone())
            .collect();
        dimensions.sort();
        dimensions.dedup();
        let mut expected: Vec<String> = DIMENSIONS.iter().map(|name| (*name).to_owned()).collect();
        expected.sort();
        assert_eq!(dimensions, expected, "{}", method.as_str());
    }
    // Every header fact is a dimension of its own.
    for name in FACT_NAMES {
        assert!(
            report
                .by_segment
                .iter()
                .any(|cell| cell.dimension == name && cell.model == Method::Jev),
            "{name} is missing"
        );
    }
    Ok(())
}

#[test]
fn cl_04_ac5_versions_not_pooled_by_default() -> TestResult {
    let mut rows: Vec<EvalRow> = (0..20_u32)
        .map(|index| {
            row(
                index,
                23,
                MessageClass::List,
                None,
                Some(jev(MessageClass::List, 150, 500)),
                SwipeDirection::Left,
                false,
            )
        })
        .collect();
    for evaluation in &mut rows {
        evaluation.input_version = "3".to_owned();
    }
    let report = build_report(&rows, &query(), &default_prices())?;
    assert_eq!(report.versions_present.input, vec!["3".to_owned()]);
    assert!(!report.query.pool_versions);

    // One more input version and the call is refused, naming both versions.
    for evaluation in rows.iter_mut().take(5) {
        evaluation.input_version = "4".to_owned();
    }
    match build_report(&rows, &query(), &default_prices()) {
        Err(ReportError::VersionsMixed { input, .. }) => {
            assert_eq!(input, vec!["3".to_owned(), "4".to_owned()]);
        }
        other => panic!("expected VersionsMixed, got {other:?}"),
    }

    // Asked to pool, the same records are reported together.
    let mut pooled = query();
    pooled.pool_versions = true;
    let report = build_report(&rows, &pooled, &default_prices())?;
    assert_eq!(report.labelled_swipes, Some(20));
    assert_eq!(
        report.versions_present.input,
        vec!["3".to_owned(), "4".to_owned()]
    );
    Ok(())
}

#[test]
fn bakeoff_report_holds_no_row_level_value() -> TestResult {
    let mut rows = Vec::new();
    for _ in 0..20_u32 {
        let mut evaluation = row(
            424242,
            22,
            MessageClass::List,
            Some(gemini(MessageClass::List, 400, 500)),
            Some(jev(MessageClass::List, 150, 500)),
            SwipeDirection::Left,
            false,
        );
        evaluation.created_at = instant(22, 13, 45, 7);
        rows.push(evaluation);
    }
    let report = report_of(&rows)?;
    let json = serde_json::to_string(&report)?;
    assert!(json.contains("wilson_95"));
    assert!(
        !json.contains("424242"),
        "the participant index must not leave"
    );
    assert!(!json.contains("13:45:07"), "no per-row timestamp");
    assert!(!json.contains("eval_id"));
    let mut numbers = Vec::new();
    let mut keys = Vec::new();
    collect(&serde_json::from_str(&json)?, &mut numbers, &mut keys);
    assert!(!numbers.contains(&424242.0));
    for forbidden in ["created_at", "participant", "user_pseudo_id", "latency_ms"] {
        assert!(
            !keys.iter().any(|key| key == forbidden),
            "{forbidden} is a per-record field"
        );
    }
    Ok(())
}

/// Every number and every object key in a JSON document.
fn collect(value: &serde_json::Value, numbers: &mut Vec<f64>, keys: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                keys.push(key.clone());
                collect(inner, numbers, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect(item, numbers, keys);
            }
        }
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_f64() {
                numbers.push(value);
            }
        }
        _ => {}
    }
}
