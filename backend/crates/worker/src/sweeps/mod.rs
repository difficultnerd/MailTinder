//! Periodic sweeps the worker runs on every API-INT-2 tick
//! (`POST /internal/v1/sweep`, S7 5.12). T-706 mounts the route and calls
//! [`run_sweeps`] once per run.

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
/// completed by the next run, well inside 24 hours. T-706 adds its other
/// steps to this function.
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
