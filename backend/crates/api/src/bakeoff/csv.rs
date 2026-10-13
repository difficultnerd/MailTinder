//! The tidy CSV of a bake-off report (S7 5.13 `CSV`; STAT-7, CL-04 AC7).
//!
//! One row per number, in the JSON's own order, with exactly the JSON's values:
//! a suppressed number is an empty cell and nothing is recomputed. The file name
//! is a server constant, never anything a request supplied (ASVS V5.4.1), and
//! every text cell is escaped so a spreadsheet cannot run it as a formula (ASVS
//! V1.2.1). Numbers are written with `{}`, Rust's shortest round-trip form, so a
//! parsed CSV value equals the JSON value bit for bit.

use domain::bakeoff::rates::Rate;
use domain::bakeoff::report::{BakeoffReport, CalibrationBin, LatencyBin, Method, MethodReport};
use time::macros::format_description;

/// The one header line (S7 5.13).
pub const CSV_HEADER: &str = "section,model,segment,metric,value,lower,upper,n";
/// The fixed file name for API-ADM-10's CSV.
pub const REPORT_FILENAME: &str = "bakeoff-report.csv";
/// The fixed file name for API-ADM-12's CSV.
pub const SNAPSHOT_FILENAME: &str = "bakeoff-snapshot.csv";

/// The calendar-date format the trend rows carry.
const ISO_DATE: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]");

/// The whole report as the tidy table, header line first.
// The row order is S7 5.13's own table, top to bottom; splitting the walk across
// several functions would hide that the CSV's order is the spec's.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn report_to_csv(report: &BakeoffReport) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');

    // summary: the three totals that have no model.
    plain(
        &mut out,
        "summary",
        "",
        "",
        "labelled_swipes",
        report.labelled_swipes,
    );
    plain(
        &mut out,
        "summary",
        "",
        "",
        "participants",
        report.participants.count,
    );
    let suppressed = Rate::suppressed();
    let share = report
        .participants
        .top_contributor_share
        .as_ref()
        .unwrap_or(&suppressed);
    rate_row(&mut out, "summary", "", "", "top_contributor_share", share);

    // models: one block per method, always all three (CR-01a G1).
    for model in &report.models {
        model_rows(&mut out, model);
    }

    // calibration, then latency: the fixed bins of each method.
    for model in &report.models {
        for bin in &model.calibration.bins {
            calibration_row(&mut out, model, bin);
        }
    }
    for model in &report.models {
        for bin in &model.latency.histogram {
            latency_row(&mut out, model, bin);
        }
    }

    // paired and agreement: one row set per pair, `a|b` in the model cell.
    for pair in &report.paired {
        let model = pair_name(pair.model_a, pair.model_b);
        plain(&mut out, "paired", &model, "", "n_paired", pair.n_paired);
        plain(
            &mut out,
            "paired",
            &model,
            "",
            "only_a_correct",
            pair.only_a_correct,
        );
        plain(
            &mut out,
            "paired",
            &model,
            "",
            "only_b_correct",
            pair.only_b_correct,
        );
        plain(
            &mut out,
            "paired",
            &model,
            "",
            "both_correct",
            pair.both_correct,
        );
        plain(
            &mut out,
            "paired",
            &model,
            "",
            "neither_correct",
            pair.neither_correct,
        );
        plain(
            &mut out,
            "paired",
            &model,
            "",
            "mcnemar_exact_p",
            pair.mcnemar_exact_p,
        );
        let difference = &pair.accuracy_difference;
        write_row(
            &mut out,
            "paired",
            &model,
            "",
            "accuracy_difference",
            &number(difference.value),
            &number(difference.lower),
            &number(difference.upper),
            "",
        );
    }
    for agreement in &report.agreement {
        let model = pair_name(agreement.model_a, agreement.model_b);
        rate_row(&mut out, "agreement", &model, "", "rate", &agreement.rate);
    }

    // by_segment: `dimension=value` per method.
    for segment in &report.by_segment {
        let model = segment.model.as_str();
        let cell = format!("{}={}", segment.dimension, segment.value);
        rate_row(
            &mut out,
            "by_segment",
            model,
            &cell,
            "accuracy",
            &segment.accuracy,
        );
        rate_row(
            &mut out,
            "by_segment",
            model,
            &cell,
            "false_junk_rate",
            &segment.false_junk_rate,
        );
    }

    // confusion: `class=<class>,label=<label>`; the comma is why the segment cell
    // is quoted.
    for cell in &report.confusion {
        let segment = format!("class={},label={}", cell.class.as_str(), cell.label);
        plain(
            &mut out,
            "confusion",
            cell.model.as_str(),
            &segment,
            "count",
            cell.count,
        );
    }

    // trend: one row per method per day it ran.
    for point in &report.trend {
        let segment = format!("date={}", date_str(point.date));
        rate_row(
            &mut out,
            "trend",
            point.model.as_str(),
            &segment,
            "accuracy",
            &point.accuracy,
        );
    }

    out
}

/// One method's own figures (S7 5.13 `models`).
fn model_rows(out: &mut String, model: &MethodReport) {
    let name = model.model.as_str();
    plain(out, "models", name, "", "cards", model.cards);
    plain(
        out,
        "models",
        name,
        "",
        "valid_answers",
        model.valid_answers,
    );
    rate_row(out, "models", name, "", "accuracy", &model.accuracy);
    rate_row(
        out,
        "models",
        name,
        "",
        "junk_precision",
        &model.junk_precision,
    );
    rate_row(out, "models", name, "", "junk_recall", &model.junk_recall);
    plain(out, "models", name, "", "junk_f1", model.junk_f1);
    rate_row(
        out,
        "models",
        name,
        "",
        "false_junk_rate",
        &model.false_junk_rate,
    );
    plain(out, "models", name, "", "ece", model.calibration.ece);
    plain(out, "models", name, "", "p50_ms", model.latency.p50_ms);
    plain(out, "models", name, "", "p95_ms", model.latency.p95_ms);
    rate_row(out, "models", name, "", "timeout_rate", &model.timeout_rate);
    rate_row(out, "models", name, "", "error_rate", &model.error_rate);
    plain(out, "models", name, "", "input_tokens", model.input_tokens);
    plain(
        out,
        "models",
        name,
        "",
        "cost_per_1000_messages_usd",
        model.cost_per_1000_messages_usd,
    );
}

/// One calibration bin: its count and its observed accuracy.
fn calibration_row(out: &mut String, model: &MethodReport, bin: &CalibrationBin) {
    let segment = format!("bin={}-{}", bin.confidence_from, bin.confidence_to);
    plain(
        out,
        "calibration",
        model.model.as_str(),
        &segment,
        "count",
        bin.count,
    );
    plain(
        out,
        "calibration",
        model.model.as_str(),
        &segment,
        "observed_accuracy",
        bin.observed_accuracy,
    );
}

/// One latency bin: its count. The open top bin's upper bound reads `inf`.
fn latency_row(out: &mut String, model: &MethodReport, bin: &LatencyBin) {
    let upper = bin
        .to_ms
        .map_or_else(|| "inf".to_owned(), |ms| ms.to_string());
    let segment = format!("bin={}-{}", bin.from_ms, upper);
    plain(
        out,
        "latency",
        model.model.as_str(),
        &segment,
        "count",
        bin.count,
    );
}

/// The model cell of a pair, `a|b` (S7 5.13).
fn pair_name(a: Method, b: Method) -> String {
    format!("{}|{}", a.as_str(), b.as_str())
}

/// A row with a value and no bounds or denominator (a count, a percentile, a
/// cost).
fn plain<S: Cell>(
    out: &mut String,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    value: S,
) {
    let value = value.cell();
    write_row(out, section, model, segment, metric, &value, "", "", "");
}

/// A rate row: value, Wilson bounds and denominator.
fn rate_row(
    out: &mut String,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    rate: &Rate,
) {
    write_row(
        out,
        section,
        model,
        segment,
        metric,
        &number(rate.value),
        &number(rate.lower),
        &number(rate.upper),
        &count(rate.n),
    );
}

/// Append one row: the four text cells escaped, the four number cells as they
/// are (S7 5.13). Eight cells and the buffer: one argument fewer would mean a
/// struct no caller reuses.
#[allow(clippy::too_many_arguments)]
fn write_row(
    out: &mut String,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    value: &str,
    lower: &str,
    upper: &str,
    n: &str,
) {
    out.push_str(&csv_cell(section));
    out.push(',');
    out.push_str(&csv_cell(model));
    out.push(',');
    out.push_str(&csv_cell(segment));
    out.push(',');
    out.push_str(&csv_cell(metric));
    for cell in [value, lower, upper, n] {
        out.push(',');
        out.push_str(cell);
    }
    out.push('\n');
}

/// A formula-safe CSV cell (S7 5.13, ASVS V1.2.1).
///
/// Quoted when it holds a comma, a quote, a carriage return or a newline, with
/// inner quotes doubled; prefixed with `'` when it starts with `=`, `+`, `-`,
/// `@`, a tab or a carriage return, so a spreadsheet reads it as text. Numbers
/// never go through here: they are written with `{}` and are never prefixed.
#[must_use]
pub fn csv_cell(s: &str) -> String {
    let formula = matches!(s.chars().next(), Some('=' | '+' | '-' | '@' | '\t' | '\r'));
    let quote = s.chars().any(|c| matches!(c, ',' | '"' | '\r' | '\n'));
    if !quote {
        return if formula {
            format!("'{s}")
        } else {
            s.to_owned()
        };
    }
    // The quote comes first so the field's comma is inside it, and the formula
    // prefix goes inside the quoting: a reader sees `'=A1,B1`, not a field that
    // ends at its own comma.
    let mut out = String::with_capacity(s.len() + 4);
    out.push('"');
    if formula {
        out.push('\'');
    }
    for c in s.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A number cell, empty when the value is suppressed.
fn count(value: Option<u64>) -> String {
    value.map_or_else(String::new, |n| n.to_string())
}

/// A real-number cell, empty when the value is suppressed. `{}` is Rust's
/// shortest round-trip form, so the CSV's digits are the JSON's.
fn number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |x| x.to_string())
}

/// Every number a report cell can carry; the conversion to text happens in one
/// place so a suppressed value is one empty cell everywhere.
trait Cell {
    fn cell(self) -> String;
}

impl Cell for Option<u64> {
    fn cell(self) -> String {
        count(self)
    }
}

impl Cell for Option<u32> {
    fn cell(self) -> String {
        self.map_or_else(String::new, |n| n.to_string())
    }
}

impl Cell for Option<f64> {
    fn cell(self) -> String {
        number(self)
    }
}

/// The trend cell's day.
fn date_str(date: time::Date) -> String {
    date.format(ISO_DATE).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// ASVS V1.2.1 (S7 5.13): a text cell a spreadsheet would run as a formula is
    /// prefixed, a cell holding a delimiter or a quote is quoted, and a number
    /// cell is neither prefixed nor quoted.
    #[test]
    fn asvs_v1_2_1_csv_cells_formula_escaped() {
        assert_eq!(csv_cell("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_cell("-1"), "'-1");
        assert_eq!(csv_cell("+1"), "'+1");
        assert_eq!(csv_cell("@cmd"), "'@cmd");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        // A formula that also needs quoting gets both, the quote first so
        // the comma stays inside the field.
        assert_eq!(csv_cell("=A1,B1"), "\"'=A1,B1\"");
        // Tab and carriage return are formula starts.
        assert_eq!(csv_cell("\tx"), "'\tx");
        assert_eq!(csv_cell("\rx"), "\"'\rx\"");
        // Numbers are written as numbers, never through the text escape: a
        // negative value and a negative accuracy difference stay numeric.
        assert_eq!(number(Some(-0.5)), "-0.5");
        assert_eq!(number(Some(-1.0)), "-1");
        assert_eq!(count(Some(12)), "12");
        assert_eq!(count(None), "");
        assert_eq!(number(None), "");
    }
}
