//! The unsubscribe job lifecycle as pure functions (S3 "State machines:
//! `UnsubscribeJob`"). No I/O, no provider types, no target URL, mailto address
//! or token on the job (S5 JOB-1; targets are encrypted in the record, T-201b).
//!
//! Cloud Tasks (`maxAttempts` 4) is the only retry layer; the job never
//! re-queues itself (S3, spec audit M19). There is no `awaiting_session` status
//! (passkey lock dropped). `now` always arrives as an argument.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::DomainError;
use crate::tunables::Tunables;

/// The lifecycle status of an unsubscribe job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Sent,
    Cancelled,
    NeedsAttention,
    Failed,
    Expired,
}

impl JobStatus {
    /// True for the terminal statuses: `Sent`, `Cancelled`, `NeedsAttention`,
    /// `Failed`, `Expired`.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Sent
                | JobStatus::Cancelled
                | JobStatus::NeedsAttention
                | JobStatus::Failed
                | JobStatus::Expired
        )
    }
}

/// How the unsubscribe is delivered. The page handler is v2; never match with
/// `_` so a new variant forces a decision everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobMethod {
    OneClick,
    Mailto,
}

/// The lifecycle fields of a job; T-201b's `JobRecord` carries these plus
/// storage fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobState {
    pub status: JobStatus,
    /// Cloud Tasks retry count of the delivery that last claimed it.
    pub attempts: u32,
    pub due_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

impl JobState {
    /// A new queued job: `expires_at = due_at + JOB_TTL` (INV-2).
    pub fn new_queued(due_at: OffsetDateTime, t: &Tunables) -> JobState {
        JobState {
            status: JobStatus::Queued,
            attempts: 0,
            due_at,
            expires_at: due_at + t.job_ttl,
        }
    }
}

/// An event that may change a `JobState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobEvent {
    /// A Cloud Tasks delivery wants to run it.
    Claim { attempt: u32, now: OffsetDateTime },
    /// Undo (SW-05 AC2) or mailbox disconnect (AU-05 AC1).
    Cancel { now: OffsetDateTime },
    /// 2xx, or batched with a sent sibling.
    Sent { now: OffsetDateTime },
    /// 3xx, refused address, non-retryable, or final attempt.
    NeedsAttention { now: OffsetDateTime },
    /// Refresh token invalid when minting (UN-01 AC6).
    TokenRevoked { now: OffsetDateTime },
    /// Sweeper or runner sees `now > expires_at`.
    Expire { now: OffsetDateTime },
}

/// The result of applying an event.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    Changed(JobState),
    /// Duplicate delivery or already terminal.
    NoOp,
}

/// The only way to change a `JobState`. Errors on transitions S3 forbids.
pub fn apply(state: JobState, event: JobEvent, t: &Tunables) -> Result<Applied, DomainError> {
    // Expiry is checked first for non-terminal states: if `now > expires_at`
    // the caller must apply `Expire` instead.
    if !state.status.is_terminal() {
        let now = match event {
            JobEvent::Claim { now, .. }
            | JobEvent::Cancel { now }
            | JobEvent::Sent { now }
            | JobEvent::NeedsAttention { now }
            | JobEvent::TokenRevoked { now }
            | JobEvent::Expire { now } => now,
        };
        if now > state.expires_at && !matches!(event, JobEvent::Expire { .. }) {
            return Err(DomainError::TransitionNotAllowed);
        }
    }

    let next = match (state.status, event) {
        (JobStatus::Queued, JobEvent::Claim { attempt, now }) => {
            let mut s = state;
            s.status = JobStatus::Running;
            s.attempts = attempt;
            s.expires_at = now + t.job_outcome_retention;
            s
        }
        (JobStatus::Running, JobEvent::Claim { attempt, now }) => {
            if attempt > state.attempts {
                // Redelivery after a retryable failure or a crash.
                let mut s = state;
                s.attempts = attempt;
                s.expires_at = now + t.job_outcome_retention;
                s
            } else {
                // Duplicate delivery (UN-01 AC1).
                return Ok(Applied::NoOp);
            }
        }
        (JobStatus::Queued, JobEvent::Cancel { now }) => {
            let mut s = state;
            s.status = JobStatus::Cancelled;
            s.expires_at = now + t.job_outcome_retention;
            s
        }
        (JobStatus::Running, JobEvent::Cancel { .. }) => {
            // The run already won the race (SW-05 AC3).
            return Err(DomainError::TransitionNotAllowed);
        }
        (JobStatus::Running, JobEvent::Sent { now }) => {
            let mut s = state;
            s.status = JobStatus::Sent;
            s.expires_at = now + t.job_outcome_retention;
            s
        }
        (JobStatus::Running, JobEvent::NeedsAttention { now }) => {
            let mut s = state;
            s.status = JobStatus::NeedsAttention;
            s.expires_at = now + t.job_outcome_retention;
            s
        }
        (JobStatus::Running, JobEvent::TokenRevoked { now }) => {
            let mut s = state;
            s.status = JobStatus::Failed;
            s.expires_at = now + t.job_outcome_retention;
            s
        }
        (JobStatus::Queued | JobStatus::Running, JobEvent::Expire { now }) => {
            if now > state.expires_at {
                let mut s = state;
                s.status = JobStatus::Expired;
                s.expires_at = now + t.job_outcome_retention;
                s
            } else {
                return Err(DomainError::TransitionNotAllowed);
            }
        }
        // Any terminal status is a no-op for every event.
        (status, _) if status.is_terminal() => return Ok(Applied::NoOp),
        // Forbidden transitions (e.g. `Sent` from `Queued`).
        _ => return Err(DomainError::TransitionNotAllowed),
    };

    Ok(Applied::Changed(next))
}

/// What a Cloud Tasks delivery should do for a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimDecision {
    Run,
    TooEarly,
    Duplicate,
    AlreadyFinished,
    Expire,
}

/// Decide whether a delivery may run. Terminal gives `AlreadyFinished`;
/// `now > expires_at` gives `Expire`; `now < due_at` gives `TooEarly`;
/// `Running` with `attempt <= attempts` gives `Duplicate`; otherwise `Run`.
pub fn claim_decision(state: &JobState, attempt: u32, now: OffsetDateTime) -> ClaimDecision {
    if state.status.is_terminal() {
        return ClaimDecision::AlreadyFinished;
    }
    if now > state.expires_at {
        return ClaimDecision::Expire;
    }
    if now < state.due_at {
        return ClaimDecision::TooEarly;
    }
    if state.status == JobStatus::Running && attempt <= state.attempts {
        return ClaimDecision::Duplicate;
    }
    ClaimDecision::Run
}

/// After a retryable failure on delivery `attempt` (0-based): `GiveUp` on the
/// final attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureDecision {
    RetryLater,
    GiveUp,
}

/// `GiveUp` when `attempt + 1 >= cloud_tasks_max_attempts`, else `RetryLater`.
/// With `maxAttempts` 4, deliveries 0, 1, 2 retry and delivery 3 gives up
/// (UN-02 AC3: "retries up to 3 times").
pub fn failure_decision(attempt: u32, t: &Tunables) -> FailureDecision {
    if attempt + 1 >= t.cloud_tasks_max_attempts {
        FailureDecision::GiveUp
    } else {
        FailureDecision::RetryLater
    }
}

/// UN-01 AC2: another job of the same list already sent means this one sends
/// nothing. True when any sibling with the same `list_key_hash` has status
/// `Sent`.
pub fn batched_with_sent_sibling(siblings: &[(JobStatus, bool)]) -> bool {
    siblings
        .iter()
        .any(|(status, same_list)| *same_list && *status == JobStatus::Sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn t() -> Tunables {
        Tunables::default()
    }

    fn due() -> OffsetDateTime {
        time::macros::datetime!(2023-11-14 22:13:20 UTC)
    }

    fn queued() -> JobState {
        JobState::new_queued(due(), &t())
    }

    fn running(attempts: u32) -> JobState {
        let mut s = queued();
        match apply(
            s,
            JobEvent::Claim {
                attempt: attempts,
                now: due(),
            },
            &t(),
        ) {
            Ok(Applied::Changed(next)) => {
                s = next;
            }
            _ => panic!("claim should succeed"),
        }
        s
    }

    #[test]
    fn un_01_ac1_duplicate_delivery_is_noop() -> TestResult {
        let s = running(2);
        // Same attempt number is a duplicate.
        assert_eq!(
            apply(
                s,
                JobEvent::Claim {
                    attempt: 2,
                    now: due()
                },
                &t()
            )?,
            Applied::NoOp
        );
        // Lower attempt number is also a duplicate.
        assert_eq!(
            apply(
                s,
                JobEvent::Claim {
                    attempt: 1,
                    now: due()
                },
                &t()
            )?,
            Applied::NoOp
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn un_01_ac1_terminal_job_never_runs_again(
            status in prop_oneof![
                Just(JobStatus::Sent),
                Just(JobStatus::Cancelled),
                Just(JobStatus::NeedsAttention),
                Just(JobStatus::Failed),
                Just(JobStatus::Expired),
            ],
            attempt in 0u32..100,
        ) {
            let mut s = queued();
            s.status = status;
            let now = due();
            let decision = claim_decision(&s, attempt, now);
            prop_assert_eq!(decision, ClaimDecision::AlreadyFinished);
            prop_assert!(
                matches!(
                    apply(s, JobEvent::Claim { attempt, now }, &t()),
                    Ok(Applied::NoOp)
                ),
                "terminal job must be a no-op"
            );
        }
    }

    #[test]
    fn un_01_ac2_sent_sibling_batches() {
        assert!(batched_with_sent_sibling(&[
            (JobStatus::Queued, true),
            (JobStatus::Sent, true),
        ]));
        // A sent sibling of a different list does not batch.
        assert!(!batched_with_sent_sibling(&[
            (JobStatus::Queued, true),
            (JobStatus::Sent, false),
        ]));
        // No sent sibling at all.
        assert!(!batched_with_sent_sibling(&[
            (JobStatus::Queued, true),
            (JobStatus::Running, true),
        ]));
    }

    #[test]
    fn un_01_ac6_token_revoked_fails_job() -> TestResult {
        let s = running(0);
        let now = due();
        match apply(s, JobEvent::TokenRevoked { now }, &t())? {
            Applied::Changed(next) => {
                assert_eq!(next.status, JobStatus::Failed);
                assert!(next.status.is_terminal());
            }
            Applied::NoOp => panic!("token revoked should change the job"),
        }
        Ok(())
    }

    #[test]
    fn un_02_ac3_retry_until_final_attempt() {
        let t = t();
        assert_eq!(failure_decision(0, &t), FailureDecision::RetryLater);
        assert_eq!(failure_decision(1, &t), FailureDecision::RetryLater);
        assert_eq!(failure_decision(2, &t), FailureDecision::RetryLater);
        assert_eq!(failure_decision(3, &t), FailureDecision::GiveUp);
    }

    #[test]
    fn un_02_ac4_redirect_goes_to_needs_attention() -> TestResult {
        let s = running(0);
        let now = due();
        match apply(s, JobEvent::NeedsAttention { now }, &t())? {
            Applied::Changed(next) => {
                assert_eq!(next.status, JobStatus::NeedsAttention);
                assert!(next.status.is_terminal());
            }
            Applied::NoOp => panic!("needs attention should change the job"),
        }
        Ok(())
    }

    #[test]
    fn un_05_ac1_cannot_complete_needs_attention() -> TestResult {
        let s = running(0);
        let now = due();
        let s = match apply(s, JobEvent::NeedsAttention { now }, &t())? {
            Applied::Changed(next) => next,
            Applied::NoOp => panic!("needs attention should change the job"),
        };
        // A needs-attention job is terminal: nothing can run it again.
        assert_eq!(
            apply(s, JobEvent::Claim { attempt: 1, now }, &t())?,
            Applied::NoOp
        );
        assert_eq!(apply(s, JobEvent::Sent { now }, &t())?, Applied::NoOp);
        Ok(())
    }

    proptest! {
        #[test]
        fn un_05_ac2_every_job_terminates_by_expiry(
            status in prop_oneof![
                Just(JobStatus::Queued),
                Just(JobStatus::Running),
            ],
            attempts in 0u32..10,
        ) {
            let mut s = queued();
            s.status = status;
            s.attempts = attempts;
            let now = s.expires_at + Duration::seconds(1);
            match apply(s, JobEvent::Expire { now }, &t()) {
                Ok(Applied::Changed(next)) => {
                    prop_assert!(next.status.is_terminal());
                    prop_assert_eq!(next.status, JobStatus::Expired);
                }
                Ok(Applied::NoOp) => prop_assert!(false, "expiry must move a non-terminal job"),
                Err(_) => prop_assert!(false, "expiry must succeed"),
            }
        }
    }

    #[test]
    fn sw_05_ac2_cancel_queued_job() -> TestResult {
        let s = queued();
        let now = due();
        match apply(s, JobEvent::Cancel { now }, &t())? {
            Applied::Changed(next) => {
                assert_eq!(next.status, JobStatus::Cancelled);
                assert!(next.status.is_terminal());
            }
            Applied::NoOp => panic!("cancel should change a queued job"),
        }
        Ok(())
    }

    #[test]
    fn sw_05_ac3_cancel_after_claim_refused() {
        let s = running(0);
        let now = due();
        assert_eq!(
            apply(s, JobEvent::Cancel { now }, &t()),
            Err(DomainError::TransitionNotAllowed)
        );
    }

    proptest! {
        #[test]
        fn sw_05_ac5_claim_and_cancel_exactly_one_wins(
            now_offset in -120i64..120,
        ) {
            let now = due() + Duration::seconds(now_offset);
            let t = t();

            // Order 1: claim first, then cancel.
            let after_claim = match apply(queued(), JobEvent::Claim { attempt: 0, now }, &t) {
                Ok(Applied::Changed(s)) => s,
                Ok(Applied::NoOp) | Err(_) => queued(),
            };
            let cancel_after_claim = apply(after_claim, JobEvent::Cancel { now }, &t);

            // Order 2: cancel first, then claim.
            let after_cancel = match apply(queued(), JobEvent::Cancel { now }, &t) {
                Ok(Applied::Changed(s)) => s,
                Ok(Applied::NoOp) | Err(_) => queued(),
            };
            let claim_after_cancel = apply(after_cancel, JobEvent::Claim { attempt: 0, now }, &t);

            // Exactly one of the two writers wins: either the claim succeeded
            // and the cancel was refused, or the cancel succeeded and the
            // claim was a no-op. Never both, never neither.
            let claim_won = matches!(cancel_after_claim, Err(DomainError::TransitionNotAllowed))
                && matches!(claim_after_cancel, Ok(Applied::NoOp));
            let cancel_won = matches!(cancel_after_claim, Ok(Applied::NoOp))
                && matches!(claim_after_cancel, Ok(Applied::NoOp));
            prop_assert!(claim_won || cancel_won, "exactly one must win");
        }
    }

    #[test]
    fn inv_2_new_job_has_expires_at_after_due() {
        let s = queued();
        assert!(s.expires_at > s.due_at);
        assert_eq!(s.expires_at, s.due_at + t().job_ttl);
    }

    proptest! {
        #[test]
        fn inv_2_terminal_job_expires_after_retention(
            status in prop_oneof![
                Just(JobStatus::Sent),
                Just(JobStatus::Cancelled),
                Just(JobStatus::NeedsAttention),
                Just(JobStatus::Failed),
                Just(JobStatus::Expired),
            ],
            attempts in 0u32..10,
        ) {
            let mut s = queued();
            // `Cancel` is only valid from `Queued`; the other terminal moves
            // come from `Running`.
            s.status = if status == JobStatus::Cancelled {
                JobStatus::Queued
            } else {
                JobStatus::Running
            };
            s.attempts = attempts;
            // Expiry needs `now > expires_at`; the other terminal moves just
            // need a valid `now`.
            let now = if status == JobStatus::Expired {
                s.expires_at + Duration::seconds(1)
            } else {
                due()
            };
            let event = match status {
                JobStatus::Sent => JobEvent::Sent { now },
                JobStatus::Cancelled => JobEvent::Cancel { now },
                JobStatus::NeedsAttention => JobEvent::NeedsAttention { now },
                JobStatus::Failed => JobEvent::TokenRevoked { now },
                JobStatus::Expired => JobEvent::Expire { now },
                _ => unreachable!(),
            };
            match apply(s, event, &t()) {
                Ok(Applied::Changed(next)) => {
                    prop_assert_eq!(next.expires_at, now + t().job_outcome_retention);
                }
                Ok(Applied::NoOp) => prop_assert!(false, "terminal move must change the job"),
                Err(_) => prop_assert!(false, "terminal move must succeed"),
            }
        }
    }

    proptest! {
        #[test]
        fn job_1_non_terminal_never_outlives_job_ttl(
            status in prop_oneof![
                Just(JobStatus::Queued),
                Just(JobStatus::Running),
            ],
            attempts in 0u32..10,
        ) {
            let mut s = queued();
            s.status = status;
            s.attempts = attempts;
            let now = s.due_at + t().job_ttl + Duration::seconds(1);
            prop_assert_eq!(claim_decision(&s, attempts, now), ClaimDecision::Expire);
        }
    }

    #[test]
    fn claim_too_early_before_due() {
        let s = queued();
        let now = s.due_at - Duration::seconds(1);
        assert_eq!(claim_decision(&s, 0, now), ClaimDecision::TooEarly);
    }

    #[test]
    fn expiry_boundary_at_and_after() -> TestResult {
        let s = queued();
        // Exactly at expires_at is not expired.
        assert_eq!(claim_decision(&s, 0, s.expires_at), ClaimDecision::Run);
        // One second after is expired.
        assert_eq!(
            claim_decision(&s, 0, s.expires_at + Duration::seconds(1)),
            ClaimDecision::Expire
        );
        // Expire at exactly expires_at is refused.
        assert_eq!(
            apply(s, JobEvent::Expire { now: s.expires_at }, &t()),
            Err(DomainError::TransitionNotAllowed)
        );
        // Expire one second after succeeds.
        match apply(
            s,
            JobEvent::Expire {
                now: s.expires_at + Duration::seconds(1),
            },
            &t(),
        )? {
            Applied::Changed(next) => assert_eq!(next.status, JobStatus::Expired),
            Applied::NoOp => panic!("expiry should change the job"),
        }
        Ok(())
    }

    #[test]
    fn redelivery_with_higher_attempt_reclaims() -> TestResult {
        let s = running(0);
        let now = due();
        match apply(s, JobEvent::Claim { attempt: 1, now }, &t())? {
            Applied::Changed(next) => {
                assert_eq!(next.status, JobStatus::Running);
                assert_eq!(next.attempts, 1);
            }
            Applied::NoOp => panic!("redelivery should reclaim the job"),
        }
        Ok(())
    }

    #[test]
    fn sent_only_from_running() {
        let s = queued();
        assert_eq!(
            apply(s, JobEvent::Sent { now: due() }, &t()),
            Err(DomainError::TransitionNotAllowed)
        );
    }

    #[test]
    fn job_method_has_exactly_two_variants() {
        // Compiles only if JobMethod has exactly OneClick and Mailto.
        let _ = [JobMethod::OneClick, JobMethod::Mailto];
    }
}
