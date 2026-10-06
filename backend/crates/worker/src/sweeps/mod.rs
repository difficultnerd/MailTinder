//! Sweeps the worker runs on an API-INT-2 tick (`POST /internal/v1/sweep`,
//! S7 5.12). T-706 creates the worker's route and service: it mounts the
//! route and calls [`run_sweeps`] once per run. **Until T-706 merges, this
//! crate has no production caller** — `main.rs` is a stub and nothing serves
//! the route — so these functions are exercised only by tests.
//!
//! # What depends on T-706
//!
//! The 24-hour account-deletion backstop of S2 AU-06 AC1 is only real once
//! T-706's route reaches [`run_sweeps`]. T-706's test
//! `api_int_2_calls_the_deleted_user_sweep` is the gate for that clause
//! (`docs/backlog/T-706-worker-service-and-sweeps.md`).

pub mod deleted_users;

use obs::Pseudonymiser;
use ports::{ServerStore, StoreError};

/// Records read per page of one collection (S4 1: a run is bounded).
pub const SWEEP_BATCH: u32 = 500;

/// What one run removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepCounts {
    /// Records deleted because their user document is gone (T-803 step 11).
    pub deleted_records: u64,
}

/// Every sweep API-INT-2 runs. It currently holds the account-deletion
/// backstop of S2 AU-06 AC1: a deletion the request could not finish is
/// completed by the next run, well inside 24 hours. That is true once the
/// route that calls this function exists (T-706); until then this function
/// has no production caller. T-706 adds its other steps to this function too.
///
/// # Errors
///
/// `StoreError` from a failing sweep. T-706 records the failing step, still
/// runs the others and answers `500` so Cloud Scheduler retries.
pub async fn run_sweeps(
    store: &dyn ServerStore,
    pseudo: &Pseudonymiser,
) -> Result<SweepCounts, StoreError> {
    let deleted_records = deleted_users::sweep_deleted_users(store, SWEEP_BATCH, pseudo).await?;
    Ok(SweepCounts { deleted_records })
}
