//! The `mailto:` [`UnsubSender`] (T-703, S2 UN-01 AC4 to AC6, UN-03 AC1 to
//! AC2).
//!
//! A `mailto:` job never trusts its stored target: it re-parses the decrypted
//! URI with the strict parser, spends one unit of the mailbox's daily quota,
//! mints a fresh access token for that mailbox and sends the unsubscribe email
//! from it. The token is used and dropped inside this call; nothing about the
//! send is stored beyond the job outcome. No address, subject, body or provider
//! text ever reaches a log line.

use async_trait::async_trait;
use domain::{next_status, JobMethod, MailboxEvent, MailtoTarget, NeedsAttentionReason, Provider};
use obs::{op_log, OpLog};
use ports::store::{JobOutcomeCode, Precondition};
use ports::{MailError, MailboxCtx, Ports};
use svc_common::mint::{mint_access_token, MintError};

use crate::quota::{take_mailto_quota, QuotaDecision};
use crate::sender::{ClaimedJob, SendResult, UnsubSender};

/// Sends the unsubscribe email for a `mailto:` job.
pub struct MailtoSender;

#[async_trait]
impl UnsubSender for MailtoSender {
    fn method(&self) -> JobMethod {
        JobMethod::Mailto
    }

    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult {
        // 1. Re-validate the target with the strict parser: CR, LF, a header
        // field other than subject/body, more than one address or a non-mailto
        // scheme is refused here even if the stored record was tampered with
        // (V1.3.3, V1.3.11). A refusal sends nothing.
        let Ok(target) = MailtoTarget::parse(job.target.expose()) else {
            return refused();
        };

        // 2. Spend one unit of the per-mailbox daily cap before any send. A
        // store failure retries rather than sending without a cap.
        match take_mailto_quota(ports, &job.job.mailbox_id, ports.clock.now()).await {
            Ok(QuotaDecision::Allowed) => {}
            Ok(QuotaDecision::Exceeded) => return refused(),
            Err(_) => {
                return SendResult::Retryable {
                    code: JobOutcomeCode::HttpRejected,
                }
            }
        }

        // 3. Mint a fresh access token for this job's mailbox (UN-01 AC4,
        // AC5). It is never stored and never read from the job record.
        let ctx = match mint_access_token(ports, &job.job.user_id, &job.job.mailbox_id).await {
            Ok(ctx) => ctx,
            Err(MintError::Revoked) => return SendResult::TokenRevoked,
            Err(MintError::MailboxMissing) => {
                return SendResult::NeedsAttention {
                    reason: NeedsAttentionReason::UnsubscribeFailed,
                    code: JobOutcomeCode::MailboxRemoved,
                }
            }
            Err(MintError::Transient) => {
                return SendResult::Retryable {
                    code: JobOutcomeCode::HttpRejected,
                }
            }
            Err(MintError::Crypto) => {
                // A key failure is not the user's problem and must not ask them
                // to sign in; log route and outcome only, then retry.
                log_mint_crypto_failure();
                return SendResult::Retryable {
                    code: JobOutcomeCode::HttpRejected,
                };
            }
        };

        // 4. Send from the same mailbox. T-404's adapter sends to the address in
        // the URI, applies the "Mail Tinder" label to the sent message and never
        // deletes anything (UN-03 AC2, INV-5); the service never labels again.
        match ports.mail(Provider::Gmail).send_mailto(&ctx, &target).await {
            Ok(()) => SendResult::Sent {
                code: JobOutcomeCode::MailtoSent,
            },
            Err(error) => {
                let result = map_mail_error(&error);
                if matches!(result, SendResult::TokenRevoked) {
                    mark_needs_sign_in(ports, &ctx).await;
                }
                result
            }
        }
    }
}

/// A refusal: the job ends `needs_attention` with `unsubscribe_failed`.
const fn refused() -> SendResult {
    SendResult::NeedsAttention {
        reason: NeedsAttentionReason::UnsubscribeFailed,
        code: JobOutcomeCode::Refused,
    }
}

/// Map a provider error to a `SendResult` (pure, unit tested). The provider's
/// own text is never carried here and never logged.
pub fn map_mail_error(err: &MailError) -> SendResult {
    match err {
        // The token was minted seconds ago, so the grant is gone or the send
        // scope was withdrawn `[DEFAULT]`: end the job and ask the user to sign
        // in again.
        MailError::Unauthorized | MailError::Forbidden => SendResult::TokenRevoked,
        MailError::RateLimited { .. } | MailError::Transient => SendResult::Retryable {
            code: JobOutcomeCode::HttpRejected,
        },
        MailError::NotFound | MailError::Invalid(_) => refused(),
    }
}

/// Move the mailbox to `needs_sign_in` after the provider refused the freshly
/// minted token, the same way `mint_access_token` does for a revoked grant. A
/// lost race is ignored: someone else already changed the mailbox.
async fn mark_needs_sign_in(ports: &Ports, ctx: &MailboxCtx) {
    let Ok(Some(versioned)) = ports.store.mailboxes().get(&ctx.mailbox).await else {
        return;
    };
    let mut record = versioned.record;
    record.status = next_status(record.status, MailboxEvent::TokenInvalid);
    let _ = ports
        .store
        .mailboxes()
        .put(&record, Precondition::Matches(versioned.version))
        .await;
}

/// One `op` line for a key-service failure while minting. No values.
fn log_mint_crypto_failure() {
    op_log(&OpLog {
        op: "unsub.mint",
        outcome: "failure",
        status: None,
        latency_ms: None,
    });
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure mapping and the refusals the sender relies on.
    #![allow(clippy::pedantic)]

    use domain::MailtoError;

    use super::*;

    /// The whole `MailError` to `SendResult` table.
    #[test]
    fn map_mail_error_table() {
        let cases = [
            (MailError::Unauthorized, SendResult::TokenRevoked),
            (MailError::Forbidden, SendResult::TokenRevoked),
            (
                MailError::RateLimited { retry_after_s: 30 },
                SendResult::Retryable {
                    code: JobOutcomeCode::HttpRejected,
                },
            ),
            (
                MailError::Transient,
                SendResult::Retryable {
                    code: JobOutcomeCode::HttpRejected,
                },
            ),
            (MailError::NotFound, refused()),
            (MailError::Invalid("let_slip".to_owned()), refused()),
        ];
        for (error, want) in cases {
            assert_eq!(map_mail_error(&error), want, "mapping for {error:?}");
        }
    }

    /// ASVS V1.3.3: a target over the 2,048-character limit is refused before
    /// any send call `[DEFAULT]`.
    #[test]
    fn asvs_v1_3_3_oversized_mailto_refused() -> Result<(), Box<dyn std::error::Error>> {
        let oversized = format!("mailto:{}@example.com", "a".repeat(2100));
        match MailtoTarget::parse(&oversized) {
            Err(MailtoError::TooLong) => Ok(()),
            other => Err(format!("expected TooLong, got {other:?}").into()),
        }
    }

    /// ASVS V1.3.11: CR or LF anywhere is refused, raw and percent-encoded.
    #[test]
    fn asvs_v1_3_11_crlf_in_mailto_refused() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            "mailto:unsub@example.com?subject=Remove\r\nInjected: x",
            "mailto:unsub@example.com?subject=Remove%0D%0AInjected: x",
            "mailto:unsub@example.com?body=stop%0Aextra",
            "mailto:unsub@example.com?body=stop%0Dextra",
            "mailto:bad\r\naddr@example.com",
        ];
        for uri in cases {
            if MailtoTarget::parse(uri).is_ok() {
                return Err(format!("CR or LF not refused: {uri}").into());
            }
        }
        Ok(())
    }

    /// ASVS V1.3.11: `cc` and `bcc` are never accepted, so no other recipient
    /// can be added.
    #[test]
    fn asvs_v1_3_11_cc_bcc_refused() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            "mailto:unsub@example.com?cc=attacker@example.org",
            "mailto:unsub@example.com?bcc=attacker@example.org",
            "mailto:unsub@example.com?subject=hi&cc=attacker@example.org",
        ];
        for uri in cases {
            if MailtoTarget::parse(uri).is_ok() {
                return Err(format!("cc or bcc not refused: {uri}").into());
            }
        }
        Ok(())
    }
}
