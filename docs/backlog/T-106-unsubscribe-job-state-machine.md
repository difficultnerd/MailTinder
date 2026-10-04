# T-106: Unsubscribe job state machine

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 300 lines of code plus property tests | T-101 |

**Read only these spec sections:** S3 `UnsubscribeJob` row and "State machines: UnsubscribeJob", INV-2 (`docs/specs/S3-domain-model.md`); S2 UN-01 AC1, AC2, AC3, AC6, UN-02 AC3, AC4, UN-05 AC1, AC2, SW-05 AC2, AC3, AC5 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 6.3 rows "Undo racing the run", "Duplicate delivery", "Batching", "Retry and give up", "Expiry" (`docs/specs/S10-test-strategy.md`); S5 `jobs/{id}` row and JOB-1. Nothing else is needed.

## Goal

The domain gets the job lifecycle as pure functions: statuses and methods, the allowed transitions, the claim decision for a Cloud Tasks delivery (including duplicates and redeliveries), the retry-or-give-up rule (Cloud Tasks is the only retry layer), cancellation by undo or disconnect, expiry, and the `expires_at` rules that keep INV-2 and JOB-1 true. Property tests prove that a claim and an undo can never both win (SW-05 AC5) and that no job loops (UN-05 AC2). The store record (`JobRecord`, `JobOutcome`, `JobOutcomeCode`) is T-201b's; the runner is T-701.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/unsubscribe_job.rs` | `JobStatus`, `JobMethod`, `JobState`, `JobEvent`, `apply`, `claim_decision`, `failure_decision`, helpers |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod unsubscribe_job; pub use unsubscribe_job::*;` |

## Types and signatures

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus { Queued, Running, Sent, Cancelled, NeedsAttention, Failed, Expired }
impl JobStatus { pub fn is_terminal(self) -> bool; }   // Sent, Cancelled, NeedsAttention, Failed, Expired

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobMethod { OneClick, Mailto }               // page handler is v2; never match with `_`

/// The lifecycle fields of a job; T-201b's JobRecord carries these plus storage fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobState {
    pub status: JobStatus,
    pub attempts: u32,                 // Cloud Tasks retry count of the delivery that last claimed it
    pub due_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}
impl JobState {
    /// A new queued job: expires_at = due_at + JOB_TTL (INV-2).
    pub fn new_queued(due_at: OffsetDateTime, t: &Tunables) -> JobState;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobEvent {
    Claim { attempt: u32, now: OffsetDateTime },   // a Cloud Tasks delivery wants to run it
    Cancel { now: OffsetDateTime },                // undo (SW-05 AC2) or mailbox disconnect (AU-05 AC1)
    Sent { now: OffsetDateTime },                  // 2xx, or batched with a sent sibling
    NeedsAttention { now: OffsetDateTime },        // 3xx, refused address, non-retryable, or final attempt
    TokenRevoked { now: OffsetDateTime },          // refresh token invalid when minting (UN-01 AC6)
    Expire { now: OffsetDateTime },                // sweeper or runner sees now > expires_at
}

#[derive(Debug, PartialEq, Eq)]
pub enum Applied { Changed(JobState), NoOp }       // NoOp: duplicate delivery or already terminal

/// The only way to change a JobState. Errors on transitions S3 forbids.
pub fn apply(state: JobState, event: JobEvent, t: &Tunables) -> Result<Applied, DomainError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimDecision { Run, TooEarly, Duplicate, AlreadyFinished, Expire }
pub fn claim_decision(state: &JobState, attempt: u32, now: OffsetDateTime) -> ClaimDecision;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureDecision { RetryLater, GiveUp }
/// After a retryable failure on delivery `attempt` (0-based): GiveUp on the final attempt.
pub fn failure_decision(attempt: u32, t: &Tunables) -> FailureDecision;

/// UN-01 AC2: another job of the same list already sent means this one sends nothing.
pub fn batched_with_sent_sibling(siblings: &[(JobStatus, bool /* same list_key_hash */)]) -> bool;
```

## Algorithm

1. `new_queued`: `status Queued`, `attempts 0`, `expires_at = due_at + t.job_ttl` (S3: hard TTL one hour after `due_at`).
2. `apply(state, event, t)`, transitions from S3:
   - `Claim { attempt, now }`: `Queued` gives `Running` with `attempts = attempt`. `Running` with `attempt > state.attempts` gives `Running` with `attempts = attempt` (redelivery after a retryable failure or a crash). `Running` with `attempt <= state.attempts` is `NoOp` (duplicate delivery, UN-01 AC1). Any terminal status is `NoOp`. Expiry is checked first: if `now > state.expires_at` and not terminal, return `Err(TransitionNotAllowed)` so the caller applies `Expire` instead.
   - `Cancel { now }`: only from `Queued`, giving `Cancelled`. From `Running` it is `Err(TransitionNotAllowed)` (the run already won the race; SW-05 AC3). From a terminal status it is `NoOp`.
   - `Sent`, `NeedsAttention`, `TokenRevoked`: only from `Running`, giving `Sent`, `NeedsAttention`, `Failed` respectively. From anything else `Err(TransitionNotAllowed)`.
   - `Expire { now }`: from `Queued` or `Running` when `now > expires_at`, giving `Expired`. Otherwise `Err(TransitionNotAllowed)` (not yet expired) or `NoOp` (terminal).
   - On every move into a terminal status, set `expires_at = now + t.job_outcome_retention`. S3 keeps the terminal outcome up to 30 days; JOB-1 says nothing survives its `expires_at`; moving `expires_at` satisfies both (same rule as T-701 trap 4).
3. `claim_decision(state, attempt, now)`: terminal gives `AlreadyFinished`; `now > expires_at` gives `Expire`; `now < due_at` gives `TooEarly`; `Running` with `attempt <= attempts` gives `Duplicate`; otherwise `Run`.
4. `failure_decision(attempt, t)`: `GiveUp` when `attempt + 1 >= t.cloud_tasks_max_attempts`, else `RetryLater`. With `maxAttempts` 4, deliveries 0, 1, 2 retry and delivery 3 gives up (UN-02 AC3: "retries up to 3 times"; S3: Cloud Tasks is the only retry layer, the job never re-queues itself).
5. `batched_with_sent_sibling`: true when any sibling with the same list has status `Sent`.
6. The race (SW-05 AC5) is decided by storage: T-606 and T-701 each write their transition conditionally on the version they read. The domain property test models that by applying `Claim` and `Cancel` to the same starting `Queued` state in both orders, where the second writer re-reads and applies its event to the first writer's result.
7. Everything is pure; `now` always comes in as an argument.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-01 AC1 | A due job runs once: a duplicate delivery is a no-op and a terminal job never runs again |
| UN-01 AC2 | A job whose list already has a sent job sends nothing |
| UN-01 AC6 | A revoked refresh token ends the job `failed` |
| UN-02 AC3 | Retryable failures retry until the final Cloud Tasks attempt, then the job ends in Needs Attention |
| UN-02 AC4 | A 3xx or refused address moves a running job to `needs_attention` with no retry |
| UN-05 AC1 | A job that cannot complete ends in `needs_attention` |
| UN-05 AC2 | No job loops: every job reaches a terminal state by `expires_at` |
| SW-05 AC2 | Undo before the run cancels a queued job |
| SW-05 AC3 | Undo after the claim cannot cancel; the run's result stands |
| SW-05 AC5 | A claim and an undo at the due time: exactly one wins, never both, never neither |
| INV-2 | Every job has an `expires_at`, and a terminal job's `expires_at` is reset to the outcome retention |
| JOB-1 | A non-terminal job cannot outlive `due_at + JOB_TTL` |

## Tests that must pass

- `un_01_ac1_duplicate_delivery_is_noop` (unit)
- `un_01_ac1_terminal_job_never_runs_again` (property: any terminal state and any claim gives `NoOp` or `AlreadyFinished`)
- `un_01_ac2_sent_sibling_batches` (unit)
- `un_01_ac6_token_revoked_fails_job` (unit)
- `un_02_ac3_retry_until_final_attempt` (unit: attempts 0, 1, 2 `RetryLater`; 3 `GiveUp`)
- `un_02_ac4_redirect_goes_to_needs_attention` (unit)
- `un_05_ac1_cannot_complete_needs_attention` (unit)
- `un_05_ac2_every_job_terminates_by_expiry` (property: any sequence of events with non-decreasing `now`, ending with `Expire` at `expires_at + 1s`, leaves the job terminal)
- `sw_05_ac2_cancel_queued_job` (unit)
- `sw_05_ac3_cancel_after_claim_refused` (unit)
- `sw_05_ac5_claim_and_cancel_exactly_one_wins` (property: both orders and any `now` around `due_at`; exactly one of {Cancelled, claim refused} or {Running then Sent, cancel refused})
- `inv_2_new_job_has_expires_at_after_due` (unit)
- `inv_2_terminal_job_expires_after_retention` (property: every move into a terminal status sets `expires_at = now + 30 days`)
- `job_1_non_terminal_never_outlives_job_ttl` (property: for any queued or running state, `claim_decision` at `now > due_at + 1h` is `Expire`)
- `claim_too_early_before_due` (unit)

## Edge cases and traps

- A `Running` job is not finished: a redelivery with a higher attempt number must reclaim it (T-701 trap 7), or a crashed run stays stuck until expiry.
- The job never schedules its own retry. There is no "requeue" event and there must not be one (S3, spec audit M19).
- There is no `awaiting_session` status (passkey lock dropped); do not add one.
- `JobMethod` has exactly two variants in v1. A page method is v2.
- Use `time::Duration` and `OffsetDateTime` arithmetic; write boundary tests at exactly `expires_at` (not expired) and one second after (expired).
- Never put a target URL, mailto address or token in `JobState` or `JobEvent`; S5 forbids an access token on a job, and targets are encrypted in the record (T-201b).
- If T-105a merged first and already defined `JobMethod` in this file, keep that definition.

## Out of scope

- The store record, outcome codes and conditional writes: T-201b. Running jobs and Cloud Tasks: T-701, T-702, T-703. Undo service: T-606. Sweeper: T-706. Needs Attention items: T-107 and T-701.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
