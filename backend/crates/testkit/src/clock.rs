//! A virtual clock for deterministic tests.

use std::sync::Mutex;

use ports::Clock;
use time::{Duration, OffsetDateTime};

/// The fixed start time for the virtual clock.
pub const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

/// A clock that advances only when told.
pub struct VirtualClock {
    now: Mutex<OffsetDateTime>,
}

impl VirtualClock {
    pub fn new(start: OffsetDateTime) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    pub fn advance(&self, d: Duration) {
        let mut now = self.now.lock().unwrap_or_else(|_| panic!("clock poisoned"));
        *now += d;
    }

    pub fn set(&self, t: OffsetDateTime) {
        let mut now = self.now.lock().unwrap_or_else(|_| panic!("clock poisoned"));
        *now = t;
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().unwrap_or_else(|_| panic!("clock poisoned"))
    }
}
