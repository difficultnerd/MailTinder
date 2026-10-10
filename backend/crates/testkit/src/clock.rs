//! A virtual clock for deterministic tests.

use std::sync::{Arc, Mutex};

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

/// A clock that reads another clock and adds an offset moved on demand (T-1101c).
///
/// The e2e `api` runs on real time: `fake-google` checks provider-token expiry
/// against it, and an OAuth claim must agree with whoever verifies it, so a
/// journey may not *stop* the clock the way [`VirtualClock`] does. What it may
/// do is jump it: the offset starts at zero, so the clock reads real time
/// until `/internal/test/advance-clock` moves the offset forward. That is how a
/// journey passes a queued job's due time or the 24-hour deletion horizon
/// without waiting for real time to elapse (S10 6.3).
pub struct OffsetClock {
    inner: Arc<dyn Clock>,
    offset: Mutex<Duration>,
}

impl OffsetClock {
    pub fn new(inner: Arc<dyn Clock>) -> Self {
        Self {
            inner,
            offset: Mutex::new(Duration::ZERO),
        }
    }

    /// Move the clock `d` further ahead of the inner clock.
    pub fn advance(&self, d: Duration) {
        let mut offset = self
            .offset
            .lock()
            .unwrap_or_else(|_| panic!("clock poisoned"));
        *offset += d;
    }

    /// How far ahead of the inner clock this clock reads.
    pub fn offset(&self) -> Duration {
        *self
            .offset
            .lock()
            .unwrap_or_else(|_| panic!("clock poisoned"))
    }
}

impl Clock for OffsetClock {
    fn now(&self) -> OffsetDateTime {
        self.inner.now() + self.offset()
    }
}
