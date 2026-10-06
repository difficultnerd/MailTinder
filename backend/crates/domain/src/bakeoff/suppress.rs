//! Small-cell suppression with complementary suppression (STAT-5, S7 5.13).

/// Suppress cells below `min_cell` and, when a group can then be solved by
/// subtraction, suppress one more cell until it cannot.
///
/// `cells` are counts; `groups` are index sets that each sum to a published
/// total. A cell is `Some(count)` when it is shown and `None` when suppressed.
/// Rates built on a suppressed count are suppressed by the caller.
#[must_use]
pub fn suppress_cells(cells: &[u64], groups: &[Vec<usize>], min_cell: u64) -> Vec<Option<u64>> {
    let mut out: Vec<Option<u64>> = cells
        .iter()
        .map(|&cell| if cell < min_cell { None } else { Some(cell) })
        .collect();

    loop {
        let mut changed = false;
        for group in groups {
            if group.len() < 2 {
                continue;
            }
            let hidden = group.iter().filter(|&&i| out[i].is_none()).count();
            if hidden != 1 {
                continue;
            }
            // Exactly one cell is hidden, so the others can be recovered by
            // subtraction: hide the smallest shown cell (lowest index on ties).
            let mut victim: Option<(u64, usize)> = None;
            for &index in group {
                if let Some(value) = out[index] {
                    victim = match victim {
                        Some((best, best_index))
                            if best < value || (best == value && best_index < index) =>
                        {
                            Some((best, best_index))
                        }
                        _ => Some((value, index)),
                    };
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
    out
}
