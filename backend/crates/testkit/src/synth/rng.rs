//! The generator's only random source: an in-tree `SplitMix64` (T-1120a rule 1).
//!
//! No dependency, no wall clock, no environment and no `HashMap` iteration:
//! the same seed, persona and arguments give byte-identical output on every
//! run and machine.

/// A deterministic `SplitMix64` stream.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// A labelled sub-stream, so one seed drives independent pieces (roster,
    /// per-sender sends, per-message signals) without cross-talk.
    pub fn stream(seed: u64, label: &str) -> Self {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in label.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self::new(seed ^ h.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; `0` when `n` is `0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            lo
        } else {
            lo + self.below(hi - lo + 1)
        }
    }

    /// True `per_mille` times out of 1000.
    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille.min(1000)
    }

    /// An index into a slice of `n` elements; `0` when `n` is `0`.
    pub fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.below(u64::try_from(n).unwrap_or(u64::MAX))).unwrap_or(0)
    }

    /// An element of `xs`; panics when `xs` is empty (never called that way).
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.index(xs.len())]
    }

    /// Fisher-Yates, so a permutation is reproducible.
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = self.index(i + 1);
            xs.swap(i, j);
        }
    }
}
