//! The `Clock` abstraction for `obs`: the only way log lines get a timestamp.
//!
//! `obs` must not read the wall clock itself (S10 rule 2: time comes only from
//! the Clock port). The real implementation lives in the adapters
//! (`adapters-gcp::system_clock::SystemClock`); tests inject a fixed clock.

use std::sync::Arc;

use time::OffsetDateTime;

/// The only source of time for log lines. Real implementation in
/// `adapters-gcp::system_clock::SystemClock`.
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

/// A clock that always reports a fixed instant (for tests and deterministic
/// output).
#[derive(Clone, Copy)]
pub struct FixedClock(pub OffsetDateTime);

impl Default for FixedClock {
    fn default() -> Self {
        Self(time::macros::datetime!(2026-01-01 00:00 UTC))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

/// Convenience: wrap a clock in an `Arc<dyn Clock>`.
pub fn arc(clock: impl Clock + 'static) -> Arc<dyn Clock> {
    Arc::new(clock)
}
