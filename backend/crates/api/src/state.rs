//! Shared application state for the API service.

use std::sync::Arc;

use ports::{InviteMailer, Ports};

use crate::config::ApiConfig;
use crate::limits::RateLimiter;
use crate::tokens::TokenService;

/// Everything a route handler needs.
#[derive(Clone)]
pub struct AppState {
    pub ports: Arc<Ports>,
    pub config: Arc<ApiConfig>,
    pub limits: Arc<RateLimiter>,
    /// Refresh-token storage and the in-memory access-token cache (T-503).
    pub tokens: Arc<TokenService>,
    /// Sends the invite email from the admin's own mailbox (T-505).
    pub invite_mailer: Arc<dyn InviteMailer>,
    /// The bake-off models (T-901); empty until T-904 and T-905 wire them.
    pub classifiers: crate::classify::ClassifierSet,
    /// The per-request bake-off gate; a stand-in for T-902 consent AND the
    /// T-906b switch, closed here and opened only by integration tests.
    pub bakeoff_gate: crate::classify::BakeoffGate,
    /// National holiday defaults, injectable for delivery checks.
    pub business_calendar: Arc<domain::delivery::BusinessCalendar>,
    /// The tunables the services apply (S2 `UNSUB_DELAY` and friends). The
    /// production defaults, except for the e2e due-delay override
    /// ([`crate::startup::runtime_tunables`]).
    pub tunables: domain::Tunables,
}
