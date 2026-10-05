//! Test-only policy overrides, compiled only with the `test-policy` feature.
//!
//! This module never exists in the production build. The `api`, `unsub` and
//! `worker` crates must never enable `test-policy` in their `[dependencies]`;
//! only the egress integration tests enable it (under `[dev-dependencies]`).

use std::net::SocketAddr;
use std::time::Duration;

/// Relaxations a test may grant so it can reach a local server.
#[derive(Clone)]
pub struct TestOverride {
    /// The one loopback socket a test may reach (the testbed).
    pub allow_socket: Option<SocketAddr>,
    /// Permit plain `http` to the test socket.
    pub allow_plain_http_to_socket: bool,
    /// An extra root CA the client trusts (the T-708 test CA).
    pub extra_root_ca_pem: Option<Vec<u8>>,
    /// Allowlisted Google host -> local fake (fake-google).
    pub host_routes: Vec<(&'static str, SocketAddr)>,
    /// The one-click timeout used in tests (250 ms in T-702).
    pub one_click_timeout: Duration,
}
