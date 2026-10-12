//! McNemar's exact test, the participant cluster bootstrap and the largest
//! contributor share (STAT-3, STAT-4).

use serde::Serialize;

use super::rates::Rate;
use super::rng::SplitMix64;
use super::{MIN_BOOTSTRAP_PARTICIPANTS, MIN_CELL_SIZE};

/// Exact two-sided McNemar p value from the discordant counts.
///
/// `only_a` and `only_b` are the cards where exactly one of the two models was
/// right. Identical to the two-sided binomial test on `min(only_a, only_b)` of
/// `only_a + only_b` at p = 0.5.
#[must_use]
pub fn mcnemar_exact_p(only_a: u64, only_b: u64) -> f64 {
    let discordant = only_a + only_b;
    if discordant == 0 {
        return 1.0;
    }
    let tail = only_a.min(only_b);
    // Log-space binomial tail so a large discordant count cannot overflow.
    let mut terms = Vec::with_capacity(tail as usize + 1);
    let mut term = -((discordant as f64) * std::f64::consts::LN_2);
    terms.push(term);
    for i in 0..tail {
        term += ((discordant - i) as f64).ln() - ((i + 1) as f64).ln();
        terms.push(term);
    }
    let max = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - max).exp()).sum();
    let log_sum = max + sum.ln();
    (2.0 * log_sum.exp()).min(1.0)
}

/// One participant's paired cards: cards where both models gave a valid answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticipantPair {
    /// Paired cards for this participant.
    pub n: u64,
    /// Cards where model A was right.
    pub a_correct: u64,
    /// Cards where model B was right.
    pub b_correct: u64,
}

/// How a paired accuracy difference interval was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapMethod {
    /// Resampled whole participants, with replacement.
    ClusterByParticipant,
    /// Too few participants to cluster bootstrap.
    InsufficientParticipants,
}

/// A paired accuracy difference and its bootstrap interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BootstrapResult {
    /// Observed `(A - B)` on all paired cards.
    pub value: Option<f64>,
    /// Lower 2.5% of the bootstrap distribution.
    pub lower: Option<f64>,
    /// Upper 97.5% of the bootstrap distribution.
    pub upper: Option<f64>,
    /// How the interval was produced.
    pub method: BootstrapMethod,
    /// Resamples drawn.
    pub resamples: u32,
    /// Seed used.
    pub seed: u64,
    /// Participants available.
    pub participants: u32,
}

/// Cluster bootstrap of the accuracy difference `A - B`, resampling whole
/// participants with replacement so heavy users cannot overstate certainty.
///
/// Draw order is participants inner, resamples outer, or the reference values
/// will not match.
#[must_use]
pub fn cluster_bootstrap_diff(
    parts: &[ParticipantPair],
    resamples: u32,
    seed: u64,
) -> BootstrapResult {
    let participants = parts.len() as u32;
    let total: u64 = parts.iter().map(|p| p.n).sum();
    let value = if total == 0 {
        None
    } else {
        let sum_a: i64 = parts.iter().map(|p| p.a_correct as i64).sum();
        let sum_b: i64 = parts.iter().map(|p| p.b_correct as i64).sum();
        Some((sum_a - sum_b) as f64 / total as f64)
    };
    if participants < MIN_BOOTSTRAP_PARTICIPANTS {
        return BootstrapResult {
            value,
            lower: None,
            upper: None,
            method: BootstrapMethod::InsufficientParticipants,
            resamples,
            seed,
            participants,
        };
    }
    let mut rng = SplitMix64::new(seed);
    let mut differences = Vec::with_capacity(resamples as usize);
    for _ in 0..resamples {
        let mut n: u64 = 0;
        let mut a: i64 = 0;
        let mut b: i64 = 0;
        for _ in 0..participants {
            let index = rng.below(u64::from(participants)) as usize;
            let part = parts[index];
            n += part.n;
            a += part.a_correct as i64;
            b += part.b_correct as i64;
        }
        let difference = if n == 0 {
            0.0
        } else {
            (a - b) as f64 / n as f64
        };
        differences.push(difference);
    }
    differences.sort_by(f64::total_cmp);
    let count = resamples as f64;
    let lower_index = (0.025 * count).floor() as usize;
    let upper_index = (0.975 * count).ceil() as usize - 1;
    BootstrapResult {
        value,
        lower: Some(differences[lower_index]),
        upper: Some(differences[upper_index]),
        method: BootstrapMethod::ClusterByParticipant,
        resamples,
        seed,
        participants,
    }
}

/// The largest participant's share of all labels; `None` when the total is
/// below the minimum cell size.
#[must_use]
pub fn top_contributor_share(labels_per_participant: &[u64]) -> Option<Rate> {
    let total: u64 = labels_per_participant.iter().sum();
    if total < MIN_CELL_SIZE {
        return None;
    }
    let largest = labels_per_participant.iter().copied().max().unwrap_or(0);
    Some(Rate::of(largest, total))
}
