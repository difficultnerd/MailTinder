//! The `Clock` port: the only source of time in the system.

use time::OffsetDateTime;

/// The only source of time. Real implementation in `system_clock.rs`.
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}
