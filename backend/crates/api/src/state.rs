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
    /// National holiday defaults, injectable for delivery checks.
    pub business_calendar: Arc<domain::delivery::BusinessCalendar>,
}
