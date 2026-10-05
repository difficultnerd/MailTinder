//! Shared application state for the API service.

use std::sync::Arc;

use ports::Ports;

use crate::config::ApiConfig;
use crate::limits::RateLimiter;

/// Everything a route handler needs.
#[derive(Clone)]
pub struct AppState {
    pub ports: Arc<Ports>,
    pub config: Arc<ApiConfig>,
    pub limits: Arc<RateLimiter>,
}
