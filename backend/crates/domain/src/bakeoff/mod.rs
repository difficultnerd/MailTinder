//! Bake-off statistics: pure, deterministic helpers behind the admin report (T-907).
//!
//! Counts in, counts and rates out; no `Clock`, no I/O, no logging (S3 INV-7).
//! The only randomness is [`rng::SplitMix64`] with a fixed seed, so the same
//! input always gives the same output on every platform.
//!
//! Counts are `u64` and every rate is computed in `f64`; the signatures the task
//! mandates convert counts with `as f64`, so the lossy-cast and naming lints are
//! allowed for this module rather than sprinkled over every item. The spec fixes
//! the item names (`suppress_cells` in `suppress`, `latency_histogram` in
//! `latency`) and the tight doc prose, so those lints are allowed too.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::module_name_repetitions,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    clippy::option_if_let_else
)]

pub mod calibration;
pub mod labels;
pub mod latency;
pub mod paired;
pub mod rates;
pub mod rng;
pub mod suppress;

/// Two-sided 95% normal quantile.
pub const Z_95: f64 = 1.959_963_984_540_054;
/// Smallest publishable cell (S7 5.13 `[TUNABLE]`).
pub const MIN_CELL_SIZE: u64 = 10;
/// Bootstrap resamples for the paired accuracy difference (S7 5.13 example).
pub const BOOTSTRAP_RESAMPLES: u32 = 2000;
/// Seed for the cluster bootstrap (S7 5.13 example).
pub const BOOTSTRAP_SEED: u64 = 20_261_003;
/// Fewest participants a cluster bootstrap can use (S7 5.13 `[TUNABLE]`).
pub const MIN_BOOTSTRAP_PARTICIPANTS: u32 = 5;
/// Calibration bins: ten equal-width bins of 0.1 (`[DEFAULT]`).
pub const CALIBRATION_BINS: usize = 10;
/// Latency bin edges: `[0,100) [100,250) [250,500) [500,1000) [1000,inf)`.
///
/// S7 5.13 also lists 2000 and timeout, but the model timeout is 2,000 ms, so
/// nothing answered lands above it and STAT-8 needs bins that sum to the
/// answered calls; timeouts are reported separately.
pub const LATENCY_EDGES_MS: [u32; 5] = [0, 100, 250, 500, 1000];
