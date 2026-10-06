//! The `UnsubSender` trait: one implementation per unsubscribe method
//! (one-click in T-702, mailto in T-703) (T-701).

use async_trait::async_trait;
use domain::JobMethod;
use obs::Sensitive;
use ports::store::{JobOutcomeCode, JobRecord};
use ports::Ports;

/// A job that has been claimed, with its decrypted target and sender display.
/// The strings are `Sensitive` so they cannot be logged by accident.
pub struct ClaimedJob {
    /// The job record, status `Running` as read at claim time.
    pub job: JobRecord,
    /// The decrypted target: an https URL or a mailto URI.
    pub target: Sensitive<String>,
    /// The decrypted sender display.
    pub sender_display: Sensitive<String>,
}

/// What a send attempt achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendResult {
    /// Delivered: `OneClickAccepted` or `MailtoSent`.
    Sent { code: JobOutcomeCode },
    /// A transient failure; the runner decides whether this was the last try.
    Retryable { code: JobOutcomeCode },
    /// The user must act; the runner raises the item with `reason`.
    NeedsAttention {
        reason: domain::NeedsAttentionReason,
        code: JobOutcomeCode,
    },
    /// The refresh token is gone: the job ends `failed` with a sign-in item.
    TokenRevoked,
}

/// Delivers one unsubscribe method.
#[async_trait]
pub trait UnsubSender: Send + Sync {
    /// The method this sender handles.
    fn method(&self) -> JobMethod;
    /// Attempt the send. The runner maps the result to a terminal state.
    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult;
}
