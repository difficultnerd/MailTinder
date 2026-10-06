//! Rates with Wilson 95% intervals and the junk confusion counts (STAT-1, STAT-2).

use serde::Serialize;

use super::labels::Label;
use super::Z_95;

/// A proportion with its Wilson 95% interval and sample size.
///
/// `n` is `Some(0)` when the denominator is known but empty; a suppressed rate
/// has every field `None`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rate {
    /// The point estimate, `None` when there is no denominator.
    pub value: Option<f64>,
    /// Wilson 95% lower bound.
    pub lower: Option<f64>,
    /// Wilson 95% upper bound.
    pub upper: Option<f64>,
    /// The denominator.
    pub n: Option<u64>,
}

impl Rate {
    /// `successes` of `n`; `n == 0` gives every field `None` except `n`.
    #[must_use]
    pub fn of(successes: u64, n: u64) -> Rate {
        if n == 0 {
            return Rate {
                value: None,
                lower: None,
                upper: None,
                n: Some(0),
            };
        }
        let (lower, upper) = wilson(successes, n).unwrap_or((0.0, 1.0));
        Rate {
            value: Some(successes as f64 / n as f64),
            lower: Some(lower),
            upper: Some(upper),
            n: Some(n),
        }
    }

    /// A suppressed rate: every field `None`.
    #[must_use]
    pub fn suppressed() -> Rate {
        Rate {
            value: None,
            lower: None,
            upper: None,
            n: None,
        }
    }
}

/// The Wilson 95% interval for `successes` of `n`; `None` when `n == 0`.
#[must_use]
pub fn wilson(successes: u64, n: u64) -> Option<(f64, f64)> {
    if n == 0 {
        return None;
    }
    let nf = n as f64;
    let p = successes as f64 / nf;
    let z = Z_95;
    let denom = 1.0 + z * z / nf;
    let centre = (p + z * z / (2.0 * nf)) / denom;
    let half = z * (p * (1.0 - p) / nf + z * z / (4.0 * nf * nf)).sqrt() / denom;
    let mut lower = (centre - half).max(0.0);
    let mut upper = (centre + half).min(1.0);
    if successes == 0 {
        lower = 0.0;
    }
    if successes == n {
        upper = 1.0;
    }
    Some((lower, upper))
}

/// The junk-versus-wanted confusion counts; positive is junk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JunkConfusion {
    /// Predicted junk, actually junk.
    pub tp: u64,
    /// Predicted junk, actually wanted.
    pub fp: u64,
    /// Predicted wanted, actually junk.
    pub fn_: u64,
    /// Predicted wanted, actually wanted.
    pub tn: u64,
}

impl JunkConfusion {
    /// Record one prediction against its label.
    pub fn add(&mut self, predicted: Label, actual: Label) {
        match (predicted, actual) {
            (Label::Junk, Label::Junk) => self.tp += 1,
            (Label::Junk, Label::Wanted) => self.fp += 1,
            (Label::Wanted, Label::Junk) => self.fn_ += 1,
            (Label::Wanted, Label::Wanted) => self.tn += 1,
        }
    }

    /// Every labelled card.
    #[must_use]
    pub fn n(&self) -> u64 {
        self.tp + self.fp + self.fn_ + self.tn
    }

    /// `(tp + tn) / n`.
    #[must_use]
    pub fn accuracy(&self) -> Rate {
        Rate::of(self.tp + self.tn, self.n())
    }

    /// `tp / (tp + fp)`.
    #[must_use]
    pub fn junk_precision(&self) -> Rate {
        Rate::of(self.tp, self.tp + self.fp)
    }

    /// `tp / (tp + fn)`.
    #[must_use]
    pub fn junk_recall(&self) -> Rate {
        Rate::of(self.tp, self.tp + self.fn_)
    }

    /// `2PR / (P + R)`; `None` when either rate is undefined or `P + R == 0`.
    #[must_use]
    pub fn junk_f1(&self) -> Option<f64> {
        let precision = self.junk_precision().value?;
        let recall = self.junk_recall().value?;
        if precision + recall == 0.0 {
            None
        } else {
            Some(2.0 * precision * recall / (precision + recall))
        }
    }

    /// `fp / (fp + tn)`: wanted mail predicted junk.
    #[must_use]
    pub fn false_junk_rate(&self) -> Rate {
        Rate::of(self.fp, self.fp + self.tn)
    }
}
