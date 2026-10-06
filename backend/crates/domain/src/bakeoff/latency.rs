//! Nearest-rank percentiles and the fixed latency histogram (STAT-8).

use serde::Serialize;

use super::LATENCY_EDGES_MS;

/// The nearest-rank percentile of a sorted slice; `None` when empty.
///
/// `p` is in `(0, 1]`; the rank is `ceil(p * len)`.
#[must_use]
pub fn nearest_rank(sorted_ms: &[u32], p: f64) -> Option<u32> {
    if sorted_ms.is_empty() {
        return None;
    }
    let rank = (p * sorted_ms.len() as f64).ceil();
    let index = (rank as usize).saturating_sub(1).min(sorted_ms.len() - 1);
    Some(sorted_ms[index])
}

/// One latency bin; `to_ms` is `None` for the open top bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LatencyBin {
    /// Inclusive lower bound in milliseconds.
    pub from_ms: u32,
    /// Exclusive upper bound, `None` for the open top bin.
    pub to_ms: Option<u32>,
    /// Answered calls in the bin.
    pub count: u64,
}

/// The fixed latency histogram; counts sum to the answered count. Timeouts are
/// not latencies and are reported separately.
#[must_use]
pub fn latency_histogram(answered_ms: &[u32]) -> Vec<LatencyBin> {
    let mut counts = [0_u64; 5];
    for &value in answered_ms {
        let mut index = 0;
        for edge in &LATENCY_EDGES_MS[1..] {
            if value >= *edge {
                index += 1;
            }
        }
        counts[index] += 1;
    }
    (0..LATENCY_EDGES_MS.len())
        .map(|i| LatencyBin {
            from_ms: LATENCY_EDGES_MS[i],
            to_ms: LATENCY_EDGES_MS.get(i + 1).copied(),
            count: counts[i],
        })
        .collect()
}
