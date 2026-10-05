//! Host-name resolution for egress: the `Resolver` trait and the system one.

use std::collections::HashSet;
use std::net::IpAddr;

use async_trait::async_trait;

use ports::EgressError;

/// Resolves a host name to the addresses egress checks and pins.
#[async_trait]
pub trait Resolver: Send + Sync {
    /// Look up `host`. Deduplicated, order kept. `DnsFailed` when no answer.
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, EgressError>;
}

/// Resolves through the system resolver via `tokio`.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, EgressError> {
        let addrs = tokio::net::lookup_host((host, crate::target::ONE_CLICK_PORT))
            .await
            .map_err(|_| EgressError::DnsFailed)?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for addr in addrs {
            let ip = addr.ip();
            if seen.insert(ip) {
                out.push(ip);
            }
        }
        if out.is_empty() {
            return Err(EgressError::DnsFailed);
        }
        Ok(out)
    }
}
