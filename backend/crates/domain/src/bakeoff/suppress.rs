//! Small-cell suppression with complementary suppression (STAT-5, S7 5.13).
//!
//! `suppress_cells` is a confidentiality control, not a presentation filter: the
//! admin report (T-908a) publishes every shown cell beside the total of every
//! group it belongs to, so a suppressed count must stay unrecoverable from those
//! totals — alone, jointly across intersecting groups, and in the light of the
//! nonnegativity of counts.

/// Suppress cells below `min_cell` and then whatever else the published totals
/// would reveal.
///
/// `cells` are counts; `groups` are index sets that each sum to a published
/// total (a segment dimension's values for one model, a confusion row, the four
/// paired counts, a day's trend rows). A cell is `Some(count)` when it is shown
/// and `None` when suppressed. Rates built on a suppressed count are suppressed
/// by the caller.
///
/// Three passes:
///
/// 1. every count below `min_cell` is suppressed;
/// 2. while a group of two or more cells has exactly one suppressed cell, the
///    smallest shown cell of that group is suppressed too, to a fixed point
///    (S7 5.13);
/// 3. every suppressed cell is looked for again against the whole published
///    system, not one group at a time. Reading `x` for the suppressed counts and
///    `r[g]` for a group's total minus its shown cells, a reader holds
///    `sum(x[j], j in g) == r[g]` for every group, with `x >= 0`; a suppressed
///    count is recovered when that system leaves it exactly one value. Three
///    intersecting groups of two suppressed cells each recover all three counts
///    although pass 2 hides nothing, and a zero residual pins every suppressed
///    cell of its group to zero even when there are two of them. One more cell
///    is suppressed per recovered count, and pass 3 repeats to a fixed point.
///
/// If counts are still recovered once no cell is left to suppress, the group
/// structure cannot be protected by suppressing cells: every cell is then
/// suppressed (fail closed) and the caller must not publish a group total whose
/// cells are all suppressed — such a total beside its sibling totals is again a
/// system of equations over the hidden cells.
#[must_use]
pub fn suppress_cells(cells: &[u64], groups: &[Vec<usize>], min_cell: u64) -> Vec<Option<u64>> {
    let mut out: Vec<Option<u64>> = cells
        .iter()
        .map(|&cell| if cell < min_cell { None } else { Some(cell) })
        .collect();
    suppress_sole_hidden(groups, &mut out);
    suppress_recoverable(cells, groups, &mut out);
    out
}

/// Pass 2 of S7 5.13: a group that shows its total and all but one of its cells
/// gives that cell as the difference, so the smallest shown cell of the group is
/// suppressed too; repeat to a fixed point.
fn suppress_sole_hidden(groups: &[Vec<usize>], out: &mut [Option<u64>]) {
    loop {
        let mut changed = false;
        for group in groups {
            let members: Vec<usize> = group
                .iter()
                .copied()
                .filter(|&index| index < out.len())
                .collect();
            if members.len() < 2 {
                continue;
            }
            let hidden = members
                .iter()
                .filter(|&&index| out[index].is_none())
                .count();
            if hidden != 1 {
                continue;
            }
            let mut victim: Option<(u64, usize)> = None;
            for &index in &members {
                if let Some(value) = out[index] {
                    victim = Some(smaller(victim, value, index));
                }
            }
            if let Some((_, index)) = victim {
                out[index] = None;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Pass 3: suppress further cells until no suppressed count is fixed by the
/// published system, or until nothing is shown any more (fail closed).
fn suppress_recoverable(cells: &[u64], groups: &[Vec<usize>], out: &mut [Option<u64>]) {
    loop {
        let recovered = recoverable_cells(cells, groups, out);
        if recovered.is_empty() {
            return;
        }
        let mut victim: Option<(u64, usize)> = None;
        for &index in &recovered {
            for group in groups {
                if !group.contains(&index) {
                    continue;
                }
                for &member in group {
                    if let Some(value) = out.get(member).copied().flatten() {
                        victim = Some(smaller(victim, value, member));
                    }
                }
            }
        }
        let victim = victim
            .map(|(_, index)| index)
            .or_else(|| smallest_shown(out));
        match victim {
            Some(index) => out[index] = None,
            // Nothing is shown any more, so the leak is in the group totals,
            // which only the caller can suppress.
            None => return,
        }
    }
}

/// The suppressed cells whose value the published information fixes exactly.
///
/// The information a reader holds is the shown cells, the total of every group
/// of two or more cells, and the fact that counts are not negative.
fn recoverable_cells(cells: &[u64], groups: &[Vec<usize>], out: &[Option<u64>]) -> Vec<usize> {
    let hidden: Vec<usize> = (0..out.len())
        .filter(|&index| out[index].is_none())
        .collect();
    if hidden.is_empty() {
        return Vec::new();
    }
    let equations = published_equations(cells, groups, out, &hidden);
    if equations.is_empty() {
        return Vec::new();
    }
    // An overflow in the exact arithmetic is answered with "fixed", so the
    // result errs towards suppressing more rather than towards leaving a
    // recoverable count in place.
    let mut positions = match equality_fixed(&equations, hidden.len()) {
        Some(fixed) => fixed,
        None => (0..hidden.len()).collect(),
    };
    match bounds_fixed(&equations, hidden.len()) {
        Some(forced) => positions.extend(forced),
        None => positions.extend(0..hidden.len()),
    }
    positions.sort_unstable();
    positions.dedup();
    positions
        .into_iter()
        .filter_map(|position| hidden.get(position).copied())
        .collect()
}

/// One equation per group of two or more cells: the group's suppressed cells sum
/// to the group's total minus its shown cells. Positions index into `hidden`.
fn published_equations(
    cells: &[u64],
    groups: &[Vec<usize>],
    out: &[Option<u64>],
    hidden: &[usize],
) -> Vec<(Vec<usize>, i128)> {
    let mut equations = Vec::new();
    for group in groups {
        if group.len() < 2 {
            continue;
        }
        let total: i128 = group
            .iter()
            .filter_map(|&index| cells.get(index))
            .map(|&cell| i128::from(cell))
            .sum();
        let shown: i128 = group
            .iter()
            .filter_map(|&index| out.get(index).copied().flatten())
            .map(i128::from)
            .sum();
        let positions: Vec<usize> = group
            .iter()
            .filter_map(|&index| hidden.iter().position(|&cell| cell == index))
            .collect();
        if positions.is_empty() {
            continue;
        }
        equations.push((positions, total - shown));
    }
    equations
}

/// Positions whose value the equations alone fix: every solution gives them the
/// same number. `None` when the exact arithmetic overflowed.
fn equality_fixed(equations: &[(Vec<usize>, i128)], columns: usize) -> Option<Vec<usize>> {
    let mut rows: Vec<Vec<Frac>> = equations
        .iter()
        .map(|(positions, residual)| {
            let mut row = vec![Frac::ZERO; columns.saturating_add(1)];
            for &position in positions {
                if let Some(slot) = row.get_mut(position) {
                    *slot = Frac::ONE;
                }
            }
            if let Some(slot) = row.get_mut(columns) {
                *slot = Frac::integer(*residual);
            }
            row
        })
        .collect();
    let (pivots, rank) = reduce(&mut rows, columns)?;
    // An all-zero row with a nonzero right-hand side cannot happen for real
    // counts; if it does, nothing about this table can be trusted.
    for row in rows.iter().skip(rank) {
        let no_coefficients = row.iter().take(columns).all(|entry| entry.is_zero());
        if no_coefficients && row.get(columns).is_some_and(|rhs| !rhs.is_zero()) {
            return Some((0..columns).collect());
        }
    }
    let free: Vec<usize> = (0..columns)
        .filter(|column| !pivots.contains(column))
        .collect();
    Some(
        rows.iter()
            .take(rank)
            .zip(pivots.iter())
            .filter(|(row, _)| {
                !free
                    .iter()
                    .any(|&column| row.get(column).is_some_and(|entry| !entry.is_zero()))
            })
            .map(|(_, &pivot)| pivot)
            .collect(),
    )
}

/// Positions the equations together with the nonnegativity of counts fix. Every
/// bound below holds in every solution, so an equal lower and upper bound is a
/// value a reader derives. `None` when the propagation does not settle within
/// its pass budget; the caller then treats every suppressed cell as fixed.
fn bounds_fixed(equations: &[(Vec<usize>, i128)], columns: usize) -> Option<Vec<usize>> {
    let mut lower = vec![0_i128; columns];
    let mut upper = vec![i128::MAX; columns];
    for (positions, residual) in equations {
        let ceiling = (*residual).max(0);
        for &position in positions {
            if let Some(bound) = upper.get_mut(position) {
                if ceiling < *bound {
                    *bound = ceiling;
                }
            }
        }
    }
    let passes = equations.len().saturating_add(columns).saturating_add(8);
    let mut settled = false;
    for _ in 0..passes {
        let mut changed = false;
        for (positions, residual) in equations {
            let lowest: i128 = positions
                .iter()
                .filter_map(|&position| lower.get(position))
                .sum();
            let highest: i128 = positions
                .iter()
                .filter_map(|&position| upper.get(position))
                .sum();
            for &position in positions {
                let (Some(&low), Some(&high)) = (lower.get(position), upper.get(position)) else {
                    continue;
                };
                let ceiling = (*residual - (lowest - low)).min(high);
                let floor = (*residual - (highest - high)).max(low);
                if let Some(bound) = upper.get_mut(position) {
                    if ceiling < *bound {
                        *bound = ceiling;
                        changed = true;
                    }
                }
                if let Some(bound) = lower.get_mut(position) {
                    if floor > *bound {
                        *bound = floor;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    if !settled {
        return None;
    }
    Some(
        (0..columns)
            .filter(|&position| lower.get(position) == upper.get(position))
            .collect(),
    )
}

/// Gauss–Jordan elimination of a matrix whose last column is the right-hand
/// side. Returns the pivot column of each pivot row and the rank; `None` when
/// the exact arithmetic overflows.
fn reduce(rows: &mut [Vec<Frac>], columns: usize) -> Option<(Vec<usize>, usize)> {
    let mut pivots: Vec<usize> = Vec::new();
    for column in 0..columns {
        let found = (pivots.len()..rows.len()).find(|&row| {
            rows.get(row)
                .and_then(|entries| entries.get(column))
                .is_some_and(|entry| !entry.is_zero())
        });
        let Some(pivot_row) = found else {
            continue;
        };
        rows.swap(pivots.len(), pivot_row);
        let mut pivot_entries = rows.get(pivots.len())?.clone();
        let pivot = pivot_entries.get(column).copied()?;
        for entry in &mut pivot_entries {
            *entry = entry.div(pivot)?;
        }
        for (index, entries) in rows.iter_mut().enumerate() {
            if index == pivots.len() {
                continue;
            }
            let factor = entries.get(column).copied()?;
            if factor.is_zero() {
                continue;
            }
            for (entry, pivot_entry) in entries.iter_mut().zip(pivot_entries.iter()).skip(column) {
                *entry = entry.sub(factor.mul(*pivot_entry)?)?;
            }
        }
        if let Some(slot) = rows.get_mut(pivots.len()) {
            *slot = pivot_entries;
        }
        pivots.push(column);
    }
    let rank = pivots.len();
    Some((pivots, rank))
}

/// The shown cell with the lowest count, the lowest index on ties.
fn smallest_shown(out: &[Option<u64>]) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for (index, cell) in out.iter().enumerate() {
        if let Some(value) = *cell {
            best = Some(smaller(best, value, index));
        }
    }
    best.map(|(_, index)| index)
}

/// Keeps the lower `(count, index)` pair; ties go to the lower index.
fn smaller(best: Option<(u64, usize)>, value: u64, index: usize) -> (u64, usize) {
    match best {
        Some((best_value, best_index))
            if best_value < value || (best_value == value && best_index < index) =>
        {
            (best_value, best_index)
        }
        _ => (value, index),
    }
}

/// An exact rational: a reduced numerator over a positive denominator. Every
/// operation returns `None` on overflow, which the callers treat as "cannot
/// tell", never as "safe to show".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frac {
    num: i128,
    den: i128,
}

impl Frac {
    const ZERO: Self = Self { num: 0, den: 1 };
    const ONE: Self = Self { num: 1, den: 1 };

    fn integer(value: i128) -> Self {
        Self { num: value, den: 1 }
    }

    fn is_zero(self) -> bool {
        self.num == 0
    }

    fn sub(self, other: Self) -> Option<Self> {
        let left = self.num.checked_mul(other.den)?;
        let right = other.num.checked_mul(self.den)?;
        Self::new(left.checked_sub(right)?, self.den.checked_mul(other.den)?)
    }

    fn mul(self, other: Self) -> Option<Self> {
        Self::new(
            self.num.checked_mul(other.num)?,
            self.den.checked_mul(other.den)?,
        )
    }

    fn div(self, other: Self) -> Option<Self> {
        Self::new(
            self.num.checked_mul(other.den)?,
            self.den.checked_mul(other.num)?,
        )
    }

    /// Reduces by the greatest common divisor and keeps the denominator
    /// positive; `None` when the denominator is zero.
    fn new(num: i128, den: i128) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let (num, den) = if den < 0 {
            (num.checked_neg()?, den.checked_neg()?)
        } else {
            (num, den)
        };
        let divisor = i128::try_from(gcd(num.unsigned_abs(), den.unsigned_abs())).ok()?;
        Some(Self {
            num: num / divisor,
            den: den / divisor,
        })
    }
}

/// Greatest common divisor, never zero, so it is always safe to divide by.
fn gcd(left: u128, right: u128) -> u128 {
    let mut a = left;
    let mut b = right;
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a.max(1)
}
