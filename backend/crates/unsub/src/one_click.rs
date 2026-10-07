//! The one-click sender (T-702): at most one RFC 8058 POST per call (S2 UN-02,
//! S6 section 6).
//!
//! The target is an https URL taken from a DKIM-covered header (T-406), so
//! this sender re-checks it and then hands it to `HttpEgress::one_click_post`,
//! which owns resolution, the refused-address checks, IP pinning, the timeout
//! and the no-redirect rule (T-306). `unsub` never builds its own HTTP client
//! and never mints a mailbox token for this method (UN-01 AC5).
//!
//! Scheme note (S6 section 6, ASVS V12.2.1): `parse_target` admits `http` and
//! `https` and refuses every other scheme. The production `HttpEgress` refuses
//! a non-https target with `SchemeNotAllowed` before any DNS lookup or
//! connection, so a mail-supplied `http` target still ends Needs Attention
//! with zero connections; the scheme is admitted here so the tests can reach
//! the loopback, plain-http `unsub-testbed` through the documented T-306
//! `TestOverride` (S10 6.1: no test reaches a real unsubscribe target).

use async_trait::async_trait;
use domain::{JobMethod, NeedsAttentionReason};
use obs::{op_log, OpLog};
use ports::store::JobOutcomeCode;
use ports::{EgressError, OneClickOutcome, Ports};
use url::Url;

use crate::sender::{ClaimedJob, SendResult, UnsubSender};

/// The route template the TLS-failure op log is filed under (S5).
pub const ONE_CLICK_ROUTE: &str = "unsub.one_click";

/// Stateless: every request goes through the `HttpEgress` port on `Ports`.
pub struct OneClickSender;

/// Why a decrypted target was refused before any network call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetRejected {
    /// The string is not a URL at all.
    NotUrl,
    /// The scheme is neither `http` nor `https`.
    NotHttps,
    /// The URL carries a username or password.
    HasUserinfo,
    /// The URL has no host.
    NoHost,
}

/// Parses and re-checks the decrypted target before any network call.
///
/// # Errors
///
/// Returns the `TargetRejected` reason; the caller raises Needs Attention and
/// makes no request.
pub fn parse_target(raw: &str) -> Result<Url, TargetRejected> {
    let url = Url::parse(raw).map_err(|_| TargetRejected::NotUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(TargetRejected::NotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(TargetRejected::HasUserinfo);
    }
    if url.host_str().is_none() {
        return Err(TargetRejected::NoHost);
    }
    Ok(url)
}

/// Pure mapping from the egress result to the runner's result. Tested on its
/// own (`map_one_click_table`); exhaustive on both enums, no `_` arm.
///
/// One arm per row of the task's mapping table, so a reviewer can line the code
/// up with it: `Ok(Rejected)` and `Err(Tls)` deliberately stay separate rows
/// even though both retry with `HttpRejected` (TLS additionally writes its
/// security event in `send`, not here).
#[allow(clippy::match_same_arms)]
#[must_use]
pub fn map_one_click(result: &Result<OneClickOutcome, EgressError>) -> SendResult {
    match result {
        Ok(OneClickOutcome::Accepted { .. }) => SendResult::Sent {
            code: JobOutcomeCode::OneClickAccepted,
        },
        Ok(OneClickOutcome::Redirected { .. }) => SendResult::NeedsAttention {
            reason: NeedsAttentionReason::OneClickRedirect,
            code: JobOutcomeCode::Redirected,
        },
        Ok(OneClickOutcome::Rejected { .. }) => SendResult::Retryable {
            code: JobOutcomeCode::HttpRejected,
        },
        Ok(OneClickOutcome::TimedOut) | Err(EgressError::Timeout) => SendResult::Retryable {
            code: JobOutcomeCode::TimedOut,
        },
        Err(EgressError::AddressRefused(_) | EgressError::IpLiteralHost) => {
            SendResult::NeedsAttention {
                reason: NeedsAttentionReason::OneClickAddressRefused,
                code: JobOutcomeCode::AddressRefused,
            }
        }
        Err(
            EgressError::SchemeNotAllowed
            | EgressError::CredentialsInUrl
            | EgressError::PortNotAllowed
            | EgressError::HostNotAllowed
            | EgressError::NotPermitted,
        ) => SendResult::NeedsAttention {
            reason: NeedsAttentionReason::OneClickAddressRefused,
            code: JobOutcomeCode::Refused,
        },
        Err(EgressError::DnsFailed | EgressError::Connect | EgressError::ResponseTooLarge) => {
            SendResult::Retryable {
                code: JobOutcomeCode::HttpRejected,
            }
        }
        Err(EgressError::Tls) => SendResult::Retryable {
            code: JobOutcomeCode::HttpRejected,
        },
        // Cannot happen on this path (no delete is ever attempted here); the
        // job still ends Needs Attention and the failure is logged.
        // This arm only maps egress's refusal; no delete call is made here.
        // nosemgrep: mailtinder-no-permanent-delete
        Err(EgressError::PermanentDeleteRefused) => SendResult::NeedsAttention {
            reason: NeedsAttentionReason::UnsubscribeFailed,
            code: JobOutcomeCode::Refused,
        },
    }
}

#[async_trait]
impl UnsubSender for OneClickSender {
    fn method(&self) -> JobMethod {
        JobMethod::OneClick
    }

    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult {
        // 1. Re-check the target. No network call on a failure.
        let Ok(url) = parse_target(job.target.expose()) else {
            return SendResult::NeedsAttention {
                reason: NeedsAttentionReason::OneClickAddressRefused,
                code: JobOutcomeCode::Refused,
            };
        };
        // 2. One request, through the egress port only (UN-01 AC5: no token).
        let outcome = ports.egress.one_click_post(&url).await;
        // 3. A TLS failure is logged as an operation, never with the target.
        if matches!(outcome, Err(EgressError::Tls)) {
            op_log(&OpLog {
                op: ONE_CLICK_ROUTE,
                outcome: "tls_failure",
                status: None,
                latency_ms: None,
            });
        }
        map_one_click(&outcome)
    }
}
