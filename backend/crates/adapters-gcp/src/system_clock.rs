//! The real `Clock`: the only place allowed to read the wall clock.

use ports::Clock;
use time::OffsetDateTime;

/// The real clock. The only `OffsetDateTime::now_utc()` call in the codebase
/// (T-003 Semgrep rule allows this file only).
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}
