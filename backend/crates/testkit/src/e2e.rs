//! Control values the e2e harness sets through the api's testkit routes
//! (T-1101c).
//!
//! The api process serves one e2e stack at a time, so these live in the process.
//! Only the testkit routes write them, so every other build reads `None` and
//! behaves exactly as before.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The override cell. `None` means "use the configured value".
fn cell() -> &'static Mutex<Option<Duration>> {
    static CELL: OnceLock<Mutex<Option<Duration>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The queued-unsubscribe delay a testkit route overrode, if any.
pub fn unsub_delay() -> Option<Duration> {
    *cell()
        .lock()
        .unwrap_or_else(|_| panic!("e2e control state poisoned"))
}

/// Override the delay a queued unsubscribe's due time is planned with; `None`
/// restores the value the api started with.
pub fn set_unsub_delay(delay: Option<Duration>) {
    *cell()
        .lock()
        .unwrap_or_else(|_| panic!("e2e control state poisoned")) = delay;
}
