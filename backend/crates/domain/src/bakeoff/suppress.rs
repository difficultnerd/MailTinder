//! Small-cell suppression with complementary suppression (STAT-5, S7 5.13).
//!
//! `suppress_cells` is a confidentiality control, not a presentation filter: the
//! admin report (T-908a) publishes every shown cell beside the total of every
//! group it belongs to, so a suppressed count must stay unrecoverable from those
//! totals — alone, jointly across intersecting groups, and in the light of the
//! nonnegativity and integrality of counts.

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
///    system, not one group at a time. Reading `x` for the suppressed counts
///    and `r[g]` for a group's total minus its shown cells, a reader holds
///    `sum(x[j], j in g) == r[g]` for every group, every `x[j]` a non-negative
///    integer. A suppressed count is left shown only when an alternative
///    assignment is exhibited: one that meets every equation, keeps every
///    count a non-negative integer and gives that count a different value. A
///    range the equations settle is not enough, and neither is the range of the
///    real solutions: two counts summing to one are one and zero, never a half
///    each, so integrality is part of the information the reader holds. Three
///    intersecting groups of two suppressed cells each recover all three counts
///    although pass 2 hides nothing, and a zero residual pins every suppressed
///    cell of its group to zero even when there are two of them. One more cell
///    is suppressed per recovered count, and pass 3 repeats to a fixed point; a
///    search that outruns its node budget counts as recovered, so an
///    unsupported structure is answered by suppressing more.
///
/// If counts are still recovered once no cell is left to suppress, the group
/// structure cannot be protected by suppressing cells: every cell is then
/// suppressed (fail closed) and the caller must not publish a group total whose
/// cells are all suppressed — such a total beside its sibling totals is again a
/// system of equations over the hidden cells. Withholding those totals is the
/// report assembler's side of the contract (T-908a), and its own
/// complementary-suppression step has to keep it.
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
/// of two or more cells, and the fact that counts are not negative integers. A
/// suppressed count is left shown only when an alternative assignment of every
/// suppressed count is exhibited: one that meets every equation, keeps every
/// count a non-negative integer and gives that count a different value. Nothing
/// weaker is treated as protection, so the answer errs towards suppressing
/// more.
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
    // An overflow in the exact arithmetic, or bounds that do not settle, are
    // answered with "fixed", so the result errs towards suppressing more
    // rather than towards leaving a recoverable count in place.
    match System::new(&equations, cells, &hidden) {
        Some(system) => system.recoverable(&hidden),
        None => hidden,
    }
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

/// Valid bounds on every suppressed count: each one holds in every assignment
/// that meets the equations with non-negative counts, so an equal lower and
/// upper bound settles the count. `None` when the exact arithmetic overflows or
/// the propagation does not settle within its pass budget; the caller then
/// treats every suppressed count as recovered.
fn valid_bounds(
    equations: &[(Vec<usize>, i128)],
    columns: usize,
) -> Option<(Vec<i128>, Vec<i128>)> {
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
    Some((lower, upper))
}

/// Nodes one reconstruction search may visit per count before it gives up and
/// the count is treated as recovered.
const SEARCH_NODE_BUDGET: u64 = 20_000;

/// The system a reader solves: one equation per group of two or more cells over
/// the suppressed counts, every one of those counts a non-negative integer.
struct System<'a> {
    equations: &'a [(Vec<usize>, i128)],
    /// The equations each position appears in.
    membership: Vec<Vec<usize>>,
    /// Bounds every solution of the system lies within.
    lower: Vec<i128>,
    upper: Vec<i128>,
    /// The count each position really has. The reader does not hold it; it only
    /// orders the values the search tries.
    truth: Vec<i128>,
}

impl<'a> System<'a> {
    /// `None` when the exact arithmetic overflows or the bounds do not settle,
    /// which the caller answers with "recovered".
    fn new(equations: &'a [(Vec<usize>, i128)], cells: &[u64], hidden: &[usize]) -> Option<Self> {
        let columns = hidden.len();
        let mut membership = vec![Vec::new(); columns];
        for (index, (positions, _)) in equations.iter().enumerate() {
            for &position in positions {
                if let Some(entry) = membership.get_mut(position) {
                    entry.push(index);
                }
            }
        }
        let (lower, upper) = valid_bounds(equations, columns)?;
        let truth = hidden
            .iter()
            .map(|&index| i128::from(cells.get(index).copied().unwrap_or(0)))
            .collect();
        Some(Self {
            equations,
            membership,
            lower,
            upper,
            truth,
        })
    }

    /// The positions whose count the published system fixes, as cell indices.
    ///
    /// A count is only left alone when the search exhibits an assignment that
    /// keeps it but gives it another value.
    fn recoverable(&self, hidden: &[usize]) -> Vec<usize> {
        let columns = hidden.len();
        // Positions the equations alone settle need no search.
        let settled_by_equations = match equality_fixed(self.equations, columns) {
            Some(fixed) => fixed,
            None => (0..columns).collect(),
        };
        let mut recovered = Vec::new();
        for position in 0..columns {
            let constrained = self
                .membership
                .get(position)
                .is_some_and(|entry| !entry.is_empty());
            if !constrained {
                // No group holds it, so nothing about it is published.
                continue;
            }
            let bounded = self.lower.get(position) == self.upper.get(position);
            if bounded
                || settled_by_equations.contains(&position)
                || self.alternative(position).is_none()
            {
                recovered.push(position);
            }
        }
        recovered
            .into_iter()
            .filter_map(|position| hidden.get(position).copied())
            .collect()
    }

    /// A value the position takes under the published system that differs from
    /// the one it really has, when the search exhibits one.
    ///
    /// `None` means either that the search exhausted every assignment and the
    /// equations leave that position its real value alone, or that it ran out
    /// of its node budget. Both answers are the caller's "recovered", so a count
    /// whose safety is unproven is never left shown.
    fn alternative(&self, target: usize) -> Option<i128> {
        self.alternative_with_budget(target, SEARCH_NODE_BUDGET)
    }

    /// `alternative` with an explicit node budget, so a test can show what a
    /// search that gives up answers: "no alternative", which the caller reads
    /// as "recovered" and answers by suppressing more.
    fn alternative_with_budget(&self, target: usize, budget: u64) -> Option<i128> {
        let low = self.lower.get(target).copied()?;
        let high = self.upper.get(target).copied()?;
        let truth = self.truth.get(target).copied().unwrap_or(0);
        let mut search = Search::with_budget(self, budget);
        // Values nearest the real one first: an alternative usually sits beside
        // it, and the node budget bounds the walk over the rest.
        for distance in 1..=high.saturating_sub(low) {
            for candidate in [truth.checked_sub(distance), truth.checked_add(distance)] {
                let Some(candidate) = candidate else {
                    continue;
                };
                if candidate < low || candidate > high {
                    continue;
                }
                if search.feasible(target, candidate) {
                    return Some(candidate);
                }
                if search.exhausted() {
                    return None;
                }
            }
        }
        None
    }
}

/// Backtracking over the assignments a reader could reconstruct, with every
/// equation's partial sum bounded as it goes. `exhausted` marks a walk that
/// stopped on its node budget, so that walk proves nothing either way.
struct Search<'a> {
    equations: &'a [(Vec<usize>, i128)],
    membership: &'a [Vec<usize>],
    lower: &'a [i128],
    upper: &'a [i128],
    /// The positions the equations constrain; the others take any value.
    active: Vec<usize>,
    assigned: Vec<bool>,
    /// Per equation: the partial sum of the counts assigned so far.
    sums: Vec<i128>,
    /// Per equation: the bounds the counts still unassigned can contribute.
    rest_low: Vec<i128>,
    rest_high: Vec<i128>,
    nodes: u64,
    budget: u64,
    exhausted: bool,
}

impl<'a> Search<'a> {
    /// A search that gives up once it has visited `budget` nodes.
    fn with_budget(system: &'a System<'_>, budget: u64) -> Self {
        let active: Vec<usize> = (0..system.membership.len())
            .filter(|&position| {
                system
                    .membership
                    .get(position)
                    .is_some_and(|entry| !entry.is_empty())
            })
            .collect();
        let equations = system.equations.len();
        Self {
            equations: system.equations,
            membership: &system.membership,
            lower: &system.lower,
            upper: &system.upper,
            active,
            assigned: vec![false; system.membership.len()],
            sums: vec![0; equations],
            rest_low: vec![0; equations],
            rest_high: vec![0; equations],
            nodes: 0,
            budget,
            exhausted: false,
        }
    }

    /// True when the walk stopped on the node budget, which makes its last
    /// answer meaningless.
    fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// True when some assignment with `position` set to `value` meets every
    /// equation. A false answer proves "no such assignment" only while the
    /// budget holds.
    fn feasible(&mut self, position: usize, value: i128) -> bool {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.budget {
            self.exhausted = true;
            return false;
        }
        self.reset();
        self.place(position, value);
        if !self.holds() {
            return false;
        }
        self.walk()
    }

    /// Clears the assignments and restores what the unassigned counts can still
    /// contribute.
    fn reset(&mut self) {
        self.assigned.fill(false);
        self.sums.fill(0);
        for (index, (positions, _)) in self.equations.iter().enumerate() {
            let low: i128 = positions
                .iter()
                .filter_map(|&position| self.lower.get(position))
                .sum();
            let high: i128 = positions
                .iter()
                .filter_map(|&position| self.upper.get(position))
                .sum();
            if let Some(slot) = self.rest_low.get_mut(index) {
                *slot = low;
            }
            if let Some(slot) = self.rest_high.get_mut(index) {
                *slot = high;
            }
        }
    }

    /// Assigns `position` and takes it out of the unassigned contribution.
    fn place(&mut self, position: usize, value: i128) {
        let low = self.lower.get(position).copied().unwrap_or(0);
        let high = self.upper.get(position).copied().unwrap_or(0);
        if let Some(flag) = self.assigned.get_mut(position) {
            *flag = true;
        }
        if let Some(entries) = self.membership.get(position) {
            for &index in entries {
                if let Some(sum) = self.sums.get_mut(index) {
                    *sum += value;
                }
                if let Some(slot) = self.rest_low.get_mut(index) {
                    *slot -= low;
                }
                if let Some(slot) = self.rest_high.get_mut(index) {
                    *slot -= high;
                }
            }
        }
    }

    /// Undoes one `place`.
    fn unplace(&mut self, position: usize, value: i128) {
        let low = self.lower.get(position).copied().unwrap_or(0);
        let high = self.upper.get(position).copied().unwrap_or(0);
        if let Some(flag) = self.assigned.get_mut(position) {
            *flag = false;
        }
        if let Some(entries) = self.membership.get(position) {
            for &index in entries {
                if let Some(sum) = self.sums.get_mut(index) {
                    *sum -= value;
                }
                if let Some(slot) = self.rest_low.get_mut(index) {
                    *slot += low;
                }
                if let Some(slot) = self.rest_high.get_mut(index) {
                    *slot += high;
                }
            }
        }
    }

    /// True while every equation can still reach its residual.
    fn holds(&self) -> bool {
        self.equations
            .iter()
            .enumerate()
            .all(|(index, (_, residual))| {
                let sum = self.sums.get(index).copied().unwrap_or(0);
                let low = self.rest_low.get(index).copied().unwrap_or(0);
                let high = self.rest_high.get(index).copied().unwrap_or(0);
                sum + low <= *residual && *residual <= sum + high
            })
    }

    /// True when some completion of the current assignment meets every
    /// equation. Takes the count with the fewest values left first, and stops
    /// on the node budget.
    fn walk(&mut self) -> bool {
        let mut choice: Option<(i128, usize, i128, i128)> = None;
        for &position in &self.active {
            if self.assigned.get(position).copied().unwrap_or(true) {
                continue;
            }
            let Some((low, high)) = self.domain(position) else {
                return false;
            };
            let width = high - low;
            let better = match choice {
                None => true,
                Some((width_best, position_best, _, _)) => {
                    width < width_best || (width == width_best && position < position_best)
                }
            };
            if better {
                choice = Some((width, position, low, high));
            }
        }
        let Some((_, position, low, high)) = choice else {
            // Every constrained count is assigned and every equation holds.
            return true;
        };
        for value in low..=high {
            self.nodes = self.nodes.saturating_add(1);
            if self.nodes > self.budget {
                self.exhausted = true;
                return false;
            }
            self.place(position, value);
            let found = self.walk();
            self.unplace(position, value);
            if found {
                return true;
            }
            if self.exhausted {
                return false;
            }
        }
        false
    }

    /// The values the position can still take without leaving an equation
    /// short, or `None` when the equations leave it none.
    fn domain(&self, position: usize) -> Option<(i128, i128)> {
        let mut low = self.lower.get(position).copied()?;
        let mut high = self.upper.get(position).copied()?;
        let own_low = self.lower.get(position).copied().unwrap_or(0);
        let own_high = self.upper.get(position).copied().unwrap_or(0);
        for &index in self.membership.get(position)? {
            let (_, residual) = self.equations.get(index)?;
            let sum = self.sums.get(index).copied().unwrap_or(0);
            let others_low = self.rest_low.get(index).copied().unwrap_or(0) - own_low;
            let others_high = self.rest_high.get(index).copied().unwrap_or(0) - own_high;
            low = low.max(*residual - sum - others_high);
            high = high.min(*residual - sum - others_low);
        }
        if low > high {
            None
        } else {
            Some((low, high))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{System, SEARCH_NODE_BUDGET};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn stat_5_search_budget_fails_closed() -> TestResult {
        // Two counts summing to 20,000 each have 20,001 assignments, so the
        // left count really can take 9,999. A search that may visit one node
        // cannot exhibit one of them, and its "no alternative" is read as
        // "recovered", which makes the caller suppress more: the pass errs
        // towards hiding a count rather than towards showing a recovered one.
        let equations = vec![(vec![0, 1], 20_000)];
        let cells = vec![10_000, 10_000];
        let hidden = vec![0, 1];
        let system = System::new(&equations, &cells, &hidden).ok_or("the system did not build")?;
        assert_eq!(system.alternative_with_budget(0, 1), None);
        assert_eq!(
            system.alternative_with_budget(0, SEARCH_NODE_BUDGET),
            Some(9_999)
        );
        Ok(())
    }

    #[test]
    fn stat_5_alternative_keeps_every_count_non_negative() -> TestResult {
        // Cell 0 is zero and cell 1 holds the group's 10. The alternative for
        // cell 0 is 1 beside 9, so a zero count is only shown when it cannot
        // take another value.
        let equations = vec![(vec![0, 1], 10)];
        let cells = vec![0, 10];
        let hidden = vec![0, 1];
        let system = System::new(&equations, &cells, &hidden).ok_or("the system did not build")?;
        assert_eq!(
            system.alternative_with_budget(0, SEARCH_NODE_BUDGET),
            Some(1)
        );
        assert_eq!(
            system.alternative_with_budget(1, SEARCH_NODE_BUDGET),
            Some(9)
        );
        Ok(())
    }
}
