//! T-907 bake-off statistics: known values and properties.
//!
//! Proves STAT-1, STAT-2, STAT-3, STAT-4, STAT-5, STAT-8 and BAKE-6 on the pure
//! functions in `domain::bakeoff`. Every value is a fixed reference from the
//! task, so the tests fail if the behaviour is removed or changed.
#![allow(
    clippy::unreadable_literal,
    clippy::excessive_precision,
    clippy::float_cmp,
    clippy::unnecessary_wraps,
    clippy::too_many_lines,
    clippy::cast_lossless,
    clippy::cast_possible_truncation
)]

use domain::bakeoff::calibration::calibration;
use domain::bakeoff::labels::{label_for, predicted_label, Label, SwipeDirection};
use domain::bakeoff::latency::{latency_histogram, nearest_rank};
use domain::bakeoff::paired::{
    cluster_bootstrap_diff, mcnemar_exact_p, top_contributor_share, BootstrapMethod,
    ParticipantPair,
};
use domain::bakeoff::rates::{wilson, JunkConfusion, Rate};
use domain::bakeoff::rng::SplitMix64;
use domain::bakeoff::suppress::suppress_cells;
use domain::bakeoff::{BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, LATENCY_EDGES_MS, MIN_CELL_SIZE};
use domain::MessageClass;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn close(actual: f64, expected: f64, tolerance: f64) -> bool {
    (actual - expected).abs() <= tolerance
}

fn value(rate: Rate) -> f64 {
    rate.value.unwrap_or(f64::NAN)
}

fn bound(bound: Option<f64>) -> f64 {
    bound.unwrap_or(f64::NAN)
}

#[test]
fn stat_2_wilson_known_values() -> TestResult {
    let cases: [(u64, u64, f64, f64); 9] = [
        (0, 10, 0.000000000, 0.277532800),
        (10, 10, 0.722467200, 1.000000000),
        (1, 1, 0.206549314, 1.000000000),
        (0, 1, 0.000000000, 0.793450686),
        (1, 2, 0.094531206, 0.905468794),
        (5, 10, 0.236593091, 0.763406909),
        (81, 263, 0.255288520, 0.366209577),
        (95, 100, 0.888249531, 0.978456321),
        (7231, 8312, 0.862544763, 0.877007576),
    ];
    for (successes, n, lower, upper) in cases {
        match wilson(successes, n) {
            Some((lo, hi)) => {
                assert!(
                    close(lo, lower, 1e-6),
                    "wilson({successes}/{n}) lower {lo} != {lower}"
                );
                assert!(
                    close(hi, upper, 1e-6),
                    "wilson({successes}/{n}) upper {hi} != {upper}"
                );
            }
            None => panic!("wilson({successes}/{n}) returned None"),
        }
    }
    assert!(wilson(1, 0).is_none());
    Ok(())
}

#[test]
fn stat_1_labels_and_rates() -> TestResult {
    let mut confusion = JunkConfusion::default();
    for _ in 0..6 {
        confusion.add(Label::Junk, Label::Junk);
    }
    for _ in 0..2 {
        confusion.add(Label::Junk, Label::Wanted);
    }
    for _ in 0..3 {
        confusion.add(Label::Wanted, Label::Junk);
    }
    for _ in 0..9 {
        confusion.add(Label::Wanted, Label::Wanted);
    }
    assert_eq!(
        confusion,
        JunkConfusion {
            tp: 6,
            fp: 2,
            fn_: 3,
            tn: 9
        }
    );
    assert_eq!(confusion.n(), 20);

    let accuracy = confusion.accuracy();
    assert_eq!(accuracy.n, Some(20));
    assert!(close(value(accuracy), 0.75, 1e-9));
    assert!(close(bound(accuracy.lower), 0.531299122, 1e-6));
    assert!(close(bound(accuracy.upper), 0.888138299, 1e-6));

    let precision = confusion.junk_precision();
    assert_eq!(precision.n, Some(8));
    assert!(close(value(precision), 0.75, 1e-9));
    assert!(close(bound(precision.lower), 0.409275430, 1e-6));
    assert!(close(bound(precision.upper), 0.928520787, 1e-6));

    let recall = confusion.junk_recall();
    assert_eq!(recall.n, Some(9));
    assert!(close(value(recall), 0.666666667, 1e-9));
    assert!(close(bound(recall.lower), 0.354202136, 1e-6));
    assert!(close(bound(recall.upper), 0.879416182, 1e-6));

    assert!(close(
        confusion.junk_f1().unwrap_or(f64::NAN),
        12.0 / 17.0,
        1e-9
    ));

    let false_junk = confusion.false_junk_rate();
    assert_eq!(false_junk.n, Some(11));
    assert!(close(value(false_junk), 0.181818182, 1e-9));
    assert!(close(bound(false_junk.lower), 0.051367690, 1e-6));
    assert!(close(bound(false_junk.upper), 0.476980562, 1e-6));
    Ok(())
}

#[test]
fn stat_1_zero_denominators() -> TestResult {
    let confusion = JunkConfusion::default();
    let precision = confusion.junk_precision();
    assert!(precision.value.is_none());
    assert_eq!(precision.n, Some(0));
    assert!(confusion.junk_f1().is_none());

    let suppressed = Rate::suppressed();
    assert!(suppressed.value.is_none());
    assert!(suppressed.lower.is_none());
    assert!(suppressed.upper.is_none());
    assert!(suppressed.n.is_none());
    Ok(())
}

#[test]
fn stat_3_mcnemar_known_values() -> TestResult {
    let cases: [(u64, u64, f64); 10] = [
        (0, 0, 1.0),
        (0, 1, 1.0),
        (1, 0, 1.0),
        (1, 1, 1.0),
        (0, 5, 0.0625),
        (2, 8, 0.109375),
        (3, 12, 0.03515625),
        (5, 5, 1.0),
        (40, 60, 0.056887933641),
        (310, 420, 5.31414162363e-05),
    ];
    for (only_a, only_b, expected) in cases {
        let actual = mcnemar_exact_p(only_a, only_b);
        let relative = (actual - expected).abs() / expected.abs();
        assert!(
            relative <= 1e-9,
            "mcnemar({only_a}, {only_b}) = {actual}, want {expected}"
        );
    }
    Ok(())
}

#[test]
fn stat_3_mcnemar_symmetric() -> TestResult {
    for only_a in 0..=20_u64 {
        for only_b in 0..=20_u64 {
            let forward = mcnemar_exact_p(only_a, only_b);
            let backward = mcnemar_exact_p(only_b, only_a);
            assert!(
                close(forward, backward, 1e-15),
                "{only_a},{only_b}: {forward} != {backward}"
            );
            assert!(
                forward > 0.0 && forward <= 1.0,
                "{only_a},{only_b}: {forward} out of range"
            );
        }
    }
    Ok(())
}

#[test]
fn stat_4_splitmix64_reference() -> TestResult {
    let mut seed_zero = SplitMix64::new(0);
    assert_eq!(seed_zero.next_u64(), 0xE220A8397B1DCDAF);
    assert_eq!(seed_zero.next_u64(), 0x6E789E6AA1B965F4);

    let mut seeded = SplitMix64::new(BOOTSTRAP_SEED);
    assert_eq!(seeded.next_u64(), 7_308_962_414_873_527_341);
    assert_eq!(seeded.next_u64(), 10_289_853_421_815_645_738);
    assert_eq!(seeded.next_u64(), 3_973_643_119_324_843_906);

    let mut bounded = SplitMix64::new(1);
    for _ in 0..1000 {
        assert!(bounded.below(7) < 7);
    }
    Ok(())
}

#[test]
fn stat_4_cluster_bootstrap_deterministic() -> TestResult {
    let parts = [
        ParticipantPair {
            n: 120,
            a_correct: 100,
            b_correct: 95,
        },
        ParticipantPair {
            n: 80,
            a_correct: 70,
            b_correct: 60,
        },
        ParticipantPair {
            n: 300,
            a_correct: 260,
            b_correct: 250,
        },
        ParticipantPair {
            n: 40,
            a_correct: 30,
            b_correct: 33,
        },
        ParticipantPair {
            n: 60,
            a_correct: 50,
            b_correct: 45,
        },
        ParticipantPair {
            n: 400,
            a_correct: 350,
            b_correct: 340,
        },
    ];
    let first = cluster_bootstrap_diff(&parts, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    let second = cluster_bootstrap_diff(&parts, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    assert_eq!(first, second);
    assert_eq!(first.method, BootstrapMethod::ClusterByParticipant);
    assert_eq!(first.participants, 6);
    assert_eq!(first.resamples, BOOTSTRAP_RESAMPLES);
    assert_eq!(first.seed, BOOTSTRAP_SEED);
    assert!(close(bound(first.value), 0.037, 1e-12));
    assert!(close(bound(first.lower), 0.01818181818181818, 1e-12));
    assert!(close(bound(first.upper), 0.07272727272727272, 1e-12));
    Ok(())
}

#[test]
fn stat_4_concentration_widens_interval() -> TestResult {
    let concentrated = [
        ParticipantPair {
            n: 800,
            a_correct: 720,
            b_correct: 640,
        },
        ParticipantPair {
            n: 50,
            a_correct: 40,
            b_correct: 41,
        },
        ParticipantPair {
            n: 50,
            a_correct: 41,
            b_correct: 40,
        },
        ParticipantPair {
            n: 50,
            a_correct: 40,
            b_correct: 40,
        },
        ParticipantPair {
            n: 50,
            a_correct: 39,
            b_correct: 41,
        },
    ];
    let even = [
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 161,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 161,
        },
    ];
    let concentrated_result =
        cluster_bootstrap_diff(&concentrated, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    let even_result = cluster_bootstrap_diff(&even, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);

    assert!(close(bound(concentrated_result.value), 0.078, 1e-12));
    assert!(close(bound(concentrated_result.lower), -0.024, 1e-3));
    assert!(close(bound(concentrated_result.upper), 0.0956, 1e-3));

    assert!(close(bound(even_result.value), 0.078, 1e-12));
    assert!(close(bound(even_result.lower), 0.076, 1e-3));
    assert!(close(bound(even_result.upper), 0.08, 1e-3));

    let concentrated_width = bound(concentrated_result.upper) - bound(concentrated_result.lower);
    let even_width = bound(even_result.upper) - bound(even_result.lower);
    assert!(
        concentrated_width > even_width,
        "concentrated width {concentrated_width} not wider than even {even_width}"
    );
    Ok(())
}

#[test]
fn stat_4_insufficient_participants() -> TestResult {
    let parts = [
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 160,
        },
        ParticipantPair {
            n: 200,
            a_correct: 176,
            b_correct: 161,
        },
    ];
    let result = cluster_bootstrap_diff(&parts, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    assert!(close(bound(result.value), 0.07875, 1e-12));
    assert!(result.lower.is_none());
    assert!(result.upper.is_none());
    assert_eq!(result.method, BootstrapMethod::InsufficientParticipants);
    Ok(())
}

#[test]
fn stat_4_top_contributor_share() -> TestResult {
    let share = top_contributor_share(&[900, 50, 50]);
    match share {
        Some(rate) => {
            assert!(close(value(rate), 0.9, 1e-12));
            assert_eq!(rate.n, Some(1000));
        }
        None => panic!("expected a share for 1000 labels"),
    }
    assert!(top_contributor_share(&[5, 4]).is_none());
    Ok(())
}

type SuppressCase = (Vec<u64>, Vec<Vec<usize>>, Vec<Option<u64>>);

#[test]
fn stat_5_suppress_known_cases() -> TestResult {
    let cases: [SuppressCase; 6] = [
        (
            vec![3, 50, 40, 12],
            vec![vec![0, 1, 2, 3]],
            vec![None, Some(50), Some(40), None],
        ),
        (
            vec![3, 4, 50],
            vec![vec![0, 1, 2]],
            vec![None, None, Some(50)],
        ),
        (
            vec![50, 40, 30],
            vec![vec![0, 1, 2]],
            vec![Some(50), Some(40), Some(30)],
        ),
        (
            vec![0, 50, 60],
            vec![vec![0, 1, 2]],
            vec![None, None, Some(60)],
        ),
        (
            vec![5, 20, 20],
            vec![vec![0, 1, 2]],
            vec![None, None, Some(20)],
        ),
        (
            vec![5, 30, 40, 50],
            vec![vec![0, 1], vec![2, 3], vec![0, 2], vec![1, 3]],
            vec![None, None, None, None],
        ),
    ];
    for (cells, groups, expected) in &cases {
        let got = suppress_cells(cells, groups, MIN_CELL_SIZE);
        assert_eq!(&got, expected, "cells {cells:?}");
    }
    Ok(())
}

/// The values a suppressed cell can take in an assignment that reproduces every
/// shown cell and every group total: what a reader holding the report can
/// reconstruct. An empty vector means no group holds the cell, so nothing
/// constrains it; `None` means the enumeration hit its node budget and proved
/// nothing either way.
fn reconstructed_values(
    cells: &[u64],
    groups: &[Vec<usize>],
    out: &[Option<u64>],
    target: usize,
) -> Option<Vec<u64>> {
    let hidden: Vec<usize> = (0..cells.len())
        .filter(|&index| out.get(index).copied().flatten().is_none())
        .collect();
    let equations: Vec<(Vec<usize>, u64)> = groups
        .iter()
        .filter(|group| group.len() >= 2)
        .filter_map(|group| {
            let total: u64 = group.iter().map(|&index| cells[index]).sum();
            let shown: u64 = group.iter().filter_map(|&index| out[index]).sum();
            let positions: Vec<usize> = group
                .iter()
                .filter_map(|&index| hidden.iter().position(|&cell| cell == index))
                .collect();
            if positions.is_empty() {
                None
            } else {
                Some((positions, total - shown))
            }
        })
        .collect();
    let target_position = hidden.iter().position(|&index| index == target)?;
    if !equations
        .iter()
        .any(|(positions, _)| positions.contains(&target_position))
    {
        return Some(Vec::new());
    }
    let upper: Vec<u64> = (0..hidden.len())
        .map(|position| {
            equations
                .iter()
                .filter(|(positions, _)| positions.contains(&position))
                .map(|(_, residual)| *residual)
                .min()
                .unwrap_or(0)
        })
        .collect();
    let mut walker = Reconstruction {
        equations: &equations,
        upper,
        target: target_position,
        values: Vec::new(),
        assigned: vec![0; hidden.len()],
        sums: vec![0; equations.len()],
        nodes: 0,
        budget: 2_000_000,
    };
    walker.walk(0);
    if walker.nodes > walker.budget {
        return None;
    }
    Some(walker.values)
}

/// Backtracking over the assignments a reader could reconstruct, with every
/// equation's partial sum pruned as it goes.
struct Reconstruction<'a> {
    equations: &'a [(Vec<usize>, u64)],
    upper: Vec<u64>,
    target: usize,
    values: Vec<u64>,
    assigned: Vec<u64>,
    sums: Vec<u64>,
    nodes: u64,
    budget: u64,
}

impl Reconstruction<'_> {
    fn walk(&mut self, position: usize) {
        if self.values.len() >= 2 || self.nodes > self.budget {
            return;
        }
        if position == self.upper.len() {
            let complete = self
                .equations
                .iter()
                .enumerate()
                .all(|(index, (_, residual))| self.sums.get(index) == Some(residual));
            if complete {
                let value = self.assigned.get(self.target).copied().unwrap_or(0);
                if !self.values.contains(&value) {
                    self.values.push(value);
                }
            }
            return;
        }
        let reachable = self.upper.get(position).copied().unwrap_or(0);
        for value in 0..=reachable {
            self.nodes += 1;
            if self.nodes > self.budget || self.values.len() >= 2 {
                return;
            }
            if !self.fits(position, value) {
                continue;
            }
            if let Some(slot) = self.assigned.get_mut(position) {
                *slot = value;
            }
            for (index, (positions, _)) in self.equations.iter().enumerate() {
                if positions.contains(&position) {
                    if let Some(sum) = self.sums.get_mut(index) {
                        *sum += value;
                    }
                }
            }
            self.walk(position + 1);
            for (index, (positions, _)) in self.equations.iter().enumerate() {
                if positions.contains(&position) {
                    if let Some(sum) = self.sums.get_mut(index) {
                        *sum -= value;
                    }
                }
            }
        }
    }

    /// True when the value keeps every equation at or below its residual and
    /// leaves the cells not assigned yet able to reach it.
    fn fits(&self, position: usize, value: u64) -> bool {
        self.equations
            .iter()
            .enumerate()
            .all(|(index, (positions, residual))| {
                if !positions.contains(&position) {
                    return true;
                }
                let partial = self.sums.get(index).copied().unwrap_or(0) + value;
                if partial > *residual {
                    return false;
                }
                let remaining: u64 = positions
                    .iter()
                    .filter(|&&member| member > position)
                    .filter_map(|&member| self.upper.get(member).copied())
                    .sum();
                partial + remaining >= *residual
            })
    }
}

/// A reader's reconstruction must not pin a suppressed count down: every
/// suppressed cell that some group holds has at least two possible values.
fn assert_nothing_recovered(
    cells: &[u64],
    groups: &[Vec<usize>],
    out: &[Option<u64>],
) -> TestResult {
    for index in 0..cells.len() {
        if out.get(index).copied().flatten().is_some() {
            continue;
        }
        match reconstructed_values(cells, groups, out, index) {
            Some(values) => assert!(
                values.len() != 1,
                "cells {cells:?} groups {groups:?} out {out:?}: cell {index} is reconstructed as {values:?}"
            ),
            None => {
                return Err(format!(
                    "cells {cells:?} groups {groups:?}: reconstructing cell {index} needed more than the node budget"
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Two partitions of the indices, like a table's rows and its columns.
fn random_groups(rng: &mut SplitMix64, len: usize) -> Vec<Vec<usize>> {
    let cut = 1 + rng.below((len - 1) as u64) as usize;
    let mut groups: Vec<Vec<usize>> = vec![(0..cut).collect(), (cut..len).collect()];
    groups.push((0..len).step_by(2).collect());
    groups.push((1..len).step_by(2).collect());
    groups.retain(|group| group.len() >= 2);
    groups
}

#[test]
fn stat_5_no_cell_recoverable() -> TestResult {
    let mut rng = SplitMix64::new(42);
    let mut exercised = 0_usize;
    for _ in 0..300 {
        let len = 3 + rng.below(5) as usize;
        let cells: Vec<u64> = (0..len).map(|_| rng.below(40)).collect();
        let groups = random_groups(&mut rng, len);
        let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
        for (index, cell) in out.iter().enumerate() {
            if let Some(shown) = cell {
                assert_eq!(*shown, cells[index], "cells {cells:?}");
            }
        }
        // S7 5.13: no group of two or more cells shows its total with exactly
        // one cell suppressed.
        for group in &groups {
            let hidden = group.iter().filter(|&&index| out[index].is_none()).count();
            assert_ne!(hidden, 1, "cells {cells:?} groups {groups:?} out {out:?}");
        }
        if out.iter().all(Option::is_none) {
            continue; // fail closed: nothing is shown at all
        }
        exercised += 1;
        assert_nothing_recovered(&cells, &groups, &out)?;
    }
    assert!(
        exercised > 50,
        "only {exercised} cases ran the reconstruction"
    );
    Ok(())
}

#[test]
fn stat_5_overlapping_totals_are_not_recoverable() -> TestResult {
    // Three intersecting groups of two suppressed cells each: the totals give
    // a + b = 7, b + c = 9 and a + c = 8, so a = 3, b = 4 and c = 5 even though
    // every group hides the two cells the one-group rule looks at.
    let groups = vec![vec![0, 1, 3], vec![1, 2, 3], vec![0, 2, 3]];
    let cells = vec![3, 4, 5, 50];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None, None]);
    assert_nothing_recovered(&cells, &groups, &out)?;
    // A cell no group holds stays shown.
    let cells = vec![3, 4, 5, 50, 60];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None, None, Some(60)]);
    assert_nothing_recovered(&cells, &groups, &out)
}

#[test]
fn stat_5_zero_residual_is_not_recoverable() -> TestResult {
    // A group total that is all shown cells pins every suppressed cell of the
    // group to zero, with two of them suppressed: the one-group rule hides
    // nothing here, so the shown 50 is the leak and has to go with them.
    let cells = vec![0, 0, 50];
    let groups = vec![vec![0, 1, 2]];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None]);
    assert_nothing_recovered(&cells, &groups, &out)?;
    // A cell no group holds stays shown while the leaking group is closed.
    let cells = vec![0, 0, 50, 60];
    let groups = vec![vec![0, 1, 2]];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None, Some(60)]);
    assert_nothing_recovered(&cells, &groups, &out)
}

#[test]
fn stat_5_fail_closed_when_a_total_cannot_be_protected() -> TestResult {
    // The zero total of a fully suppressed group is itself the leak: it gives
    // both of the group's cells as zero, and no further cell suppression can
    // take that equation away. Rather than label a recoverable count as
    // protected, the function suppresses every cell (fail closed); the caller
    // must not publish that group's total beside its siblings.
    let cells = vec![0, 0, 50, 60];
    let groups = vec![vec![0, 1]];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None, None]);
    // The reconstruction the fail-closed answer removes: with the group total
    // published, both suppressed counts are exactly zero.
    assert_eq!(
        reconstructed_values(&cells, &groups, &out, 0),
        Some(vec![0])
    );
    assert_eq!(
        reconstructed_values(&cells, &groups, &out, 1),
        Some(vec![0])
    );
    // The counts no group holds are unrecoverable either way.
    assert_eq!(
        reconstructed_values(&cells, &groups, &out, 2),
        Some(Vec::new())
    );
    // A confusion table whose first row is empty is the same leak in the 2 by 2
    // shape T-908a builds from confusion rows and columns: row and column
    // totals then give every count of the table.
    let cells = vec![0, 0, 40, 50];
    let groups = vec![vec![0, 1], vec![2, 3], vec![0, 2], vec![1, 3]];
    let out = suppress_cells(&cells, &groups, MIN_CELL_SIZE);
    assert_eq!(out, vec![None, None, None, None]);
    assert_eq!(
        reconstructed_values(&cells, &groups, &out, 0),
        Some(vec![0])
    );
    assert_eq!(
        reconstructed_values(&cells, &groups, &out, 2),
        Some(vec![40])
    );
    Ok(())
}

#[test]
fn stat_8_latency_histogram_and_percentiles() -> TestResult {
    let latencies = [
        120_u32, 340, 80, 95, 2000, 510, 260, 999, 1000, 1500, 250, 99,
    ];
    let bins = latency_histogram(&latencies);
    assert_eq!(bins.len(), LATENCY_EDGES_MS.len());
    let counts: Vec<u64> = bins.iter().map(|bin| bin.count).collect();
    assert_eq!(counts, vec![3, 1, 3, 2, 3]);
    assert_eq!(bins.iter().map(|bin| bin.count).sum::<u64>(), 12);
    assert_eq!(bins[0].from_ms, 0);
    assert_eq!(bins[0].to_ms, Some(100));
    assert_eq!(bins[4].from_ms, 1000);
    assert_eq!(bins[4].to_ms, None);

    let mut sorted = latencies;
    sorted.sort_unstable();
    assert_eq!(nearest_rank(&sorted, 0.5), Some(260));
    assert_eq!(nearest_rank(&sorted, 0.95), Some(2000));
    assert!(nearest_rank(&[], 0.5).is_none());
    Ok(())
}

#[test]
fn ece_known_value() -> TestResult {
    let points = [
        (0.95, true),
        (0.92, true),
        (0.91, false),
        (0.85, true),
        (0.75, false),
        (0.72, true),
        (0.55, true),
        (0.05, false),
        (1.0, true),
        (0.1, false),
    ];
    let (bins, ece) = calibration(&points);
    assert_eq!(bins.len(), 10);
    assert!(close(ece.unwrap_or(f64::NAN), 0.2, 1e-12));
    assert_eq!(bins[9].count, 4);
    assert!(close(bound(bins[9].mean_confidence), 0.945, 1e-12));
    assert!(close(bound(bins[9].observed_accuracy), 0.75, 1e-12));
    assert_eq!(bins[1].count, 1);
    assert!(calibration(&[]).1.is_none());
    Ok(())
}

#[test]
fn bake_6_label_mapping() -> TestResult {
    let cases: [(SwipeDirection, bool, Option<Label>); 8] = [
        (SwipeDirection::Left, false, Some(Label::Junk)),
        (SwipeDirection::Left, true, Some(Label::Wanted)),
        (SwipeDirection::Right, false, Some(Label::Wanted)),
        (SwipeDirection::Right, true, Some(Label::Junk)),
        (SwipeDirection::Up, false, Some(Label::Wanted)),
        (SwipeDirection::Up, true, Some(Label::Junk)),
        (SwipeDirection::Down, false, None),
        (SwipeDirection::Down, true, None),
    ];
    for (direction, undone, expected) in cases {
        assert_eq!(label_for(direction, undone), expected, "{direction:?}");
    }
    assert_eq!(predicted_label(MessageClass::List), Label::Junk);
    assert_eq!(predicted_label(MessageClass::BulkNoHeader), Label::Junk);
    assert_eq!(predicted_label(MessageClass::Suspect), Label::Junk);
    assert_eq!(predicted_label(MessageClass::Notice), Label::Wanted);
    assert_eq!(predicted_label(MessageClass::Personal), Label::Wanted);
    Ok(())
}
