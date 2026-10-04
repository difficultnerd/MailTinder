# T-706: worker service and sweeps

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | sonnet | about 350 lines of code plus tests | T-701, T-705 |

**Read only these spec sections:** S7 5.12 API-INT-2 and the `[ASSUMES]` line under it (`docs/specs/S7-api-contract.md`), S4 1 row "Scheduled sweeps" and the S4 2 role row for `worker` (`docs/specs/S4-architecture.md`), S3 entity rows `UnsubscribeJob`, `NeedsAttentionItem`, `Session`, the UnsubscribeJob state machine line "any non-terminal --(now > expires_at)--> expired" and INV-2 (`docs/specs/S3-domain-model.md`), S5 rows `jobs`, `needs_attention`, `sessions`, `invites`, `classifier_eval`, `rate_limits` and tests JOB-1, NA-T1, SES-1, INV-T1, RL-1 (`docs/specs/S5-data-inventory.md`), S2 UN-01 AC3, UN-05 AC2, NA-01 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`), S10 6.3 row "Expiry" (`docs/specs/S10-test-strategy.md`), ASVS register row V8.3.1 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

Create the `worker` Cloud Run service with one internal route, `POST /internal/v1/sweep`, that Cloud Scheduler calls every 15 minutes. It expires overdue unsubscribe jobs (raising Needs Attention items so nothing fails silently) and deletes every record whose retention has ended: terminal jobs nobody collected, Needs Attention items, sessions, classifier evaluation records, invites and rate-limit counters. Firestore TTL stays the backstop; this sweeper is the control (S4 1).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/worker/src/main.rs` | Binary: config, `Ports`, bind `PORT` |
| Create | `backend/crates/worker/src/lib.rs` | `pub fn router(state: WorkerState) -> axum::Router` |
| Create | `backend/crates/worker/src/sweep.rs` | `run_sweep` and one function per step |
| Create | `backend/crates/worker/tests/sweep.rs` | Service integration and property tests |
| Change | `backend/Cargo.toml` | Add `crates/worker` if T-001 did not |

## Types and signatures

```rust
// worker/src/sweep.rs
pub const SWEEP_BATCH: u32 = 500;              // per collection per run [TUNABLE]; Firestore free tier is 20,000 writes a day
pub const SWEEP_INTERVAL_MIN: i64 = 15;        // S7 5.12 [TUNABLE]; used only by tests to state JOB-1's bound

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct SweepCounts {
    pub jobs_expired: u64, pub jobs_deleted: u64, pub needs_attention_deleted: u64,
    pub sessions_deleted: u64, pub evals_deleted: u64, pub invites_deleted: u64, pub rate_limits_deleted: u64,
    pub more: bool,                            // some collection hit SWEEP_BATCH; the next run continues
    pub failed_steps: Vec<&'static str>,       // e.g. ["sessions"]; empty on success
}

pub struct WorkerState { pub ports: Ports, pub auth: InternalAuthConfig }
pub async fn run_sweep(state: &WorkerState) -> SweepCounts;

async fn expire_and_purge_jobs(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
async fn purge_needs_attention(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
async fn purge_sessions(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
async fn purge_evals(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
async fn purge_invites(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
async fn purge_rate_limits(ports: &Ports, now: OffsetDateTime, counts: &mut SweepCounts) -> Result<(), SvcError>;
```

Uses `JobRepo::expires_by`, `NeedsAttentionRepo::expires_by`, `SessionRepo::expires_by`, `ClassifierEvalRepo::expires_by`, `InviteRepo::purge_due`, `RateLimitRepo::expires_by` and `Precondition` (T-201b); `verify_internal_caller`, `raise_item`, `job_record::open` (T-701); `JobStatus`, `JobOutcomeCode` (T-106, T-201b, T-701).

## Algorithm

Handler:

1. `verify_internal_caller` with audience `WORKER_AUDIENCE` and caller `WORKER_SCHEDULER_CALLER` (the Cloud Scheduler service account). Failure: `401`, security event `internal_auth_failed`.
2. `counts = run_sweep(state)`. Respond `200` with `counts` as JSON when `failed_steps` is empty, else `500` with the same body (Cloud Scheduler retries and monitoring sees the failure). Counts only, no IDs (S7 5.12).

`run_sweep`: `now = clock.now()` once; run every step in this order; a failing step adds its name to `failed_steps`, logs the error kind (no values) and the next step still runs.

`expire_and_purge_jobs`:

1. `jobs().expires_by(now, SWEEP_BATCH)`; if the result length equals `SWEEP_BATCH`, set `more = true`.
2. For each record:
   - Terminal (`Sent`, `Cancelled`, `NeedsAttention`, `Failed`, `Expired`): its 30-day outcome retention has ended (T-701 reset `expires_at` at the terminal transition). `delete(job_id, Precondition::Matches(version))`; `jobs_deleted += 1`.
   - Non-terminal (`Queued`, `Running`): the job passed `due_at + JOB_TTL`.
     1. Ownership re-check (V8.3.1): load the mailbox; if missing or its `user_id` differs, finish `Expired` without an item and log `job_owner_mismatch` when it differs.
     2. Open `target` and `sender_display` (if present). Link for the item: the target only when `safe_link` accepts it (a one-click https target), else `None`.
     3. Conditional write (`Precondition::Matches(version)`): status `Expired`, `outcome = { code: Expired, at: now }`, `target = None`, `expires_at = now + 30 days`. `PreconditionFailed` means `unsub` or undo changed it; skip this record.
     4. `raise_item(... reason: JobExpired ...)` with the sender display (or "this sender" when the job has none).
     5. Clear `sender_display` (second conditional write; a failure is logged and the next run's terminal branch deletes the record later).
     6. Security event `unsub_job_outcome` (pseudonymous user ID, outcome `expired`) and metric `unsub_outcome` (UN-01 AC3).
     7. `jobs_expired += 1`.
3. Do not cancel the job's Cloud Task; if it is ever delivered, `unsub` sees a terminal job and returns `200` (T-701 step 8).

`purge_needs_attention`: `needs_attention().expires_by(now, SWEEP_BATCH)`, delete each with `Precondition::None` (an item resolved meanwhile is fine).

`purge_sessions`: `sessions().expires_by(now, SWEEP_BATCH)`, delete each with `Precondition::None`. The session's `expires_at` already covers idle and absolute timeouts and the 10-minute `pre_auth` life (T-201b, T-501).

`purge_evals`: `classifier_eval().expires_by(now, SWEEP_BATCH)` (180 days), delete each.

`purge_invites`: `invites().purge_due(now, SWEEP_BATCH)` returns the IDs it deleted (S5 "until used, revoked or expired, then 30 days").

`purge_rate_limits`: `rate_limits().expires_by(now, SWEEP_BATCH)` deletes and returns a count.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-05 AC2 | Every job ends terminal within `JOB_TTL`; overdue jobs become `expired` with a Needs Attention item |
| UN-01 AC3 | The expired outcome is stored on the job and written to the security log |
| NA-01 AC2 | Needs Attention items are deleted after `NEEDS_ATTENTION_TTL` |
| INV-2 | A sweeper deletes expired job and Needs Attention rows |
| JOB-1 | No job record survives its `expires_at` plus the sweep interval |
| NA-T1 | No Needs Attention record survives 30 days |
| SES-1 | Expired sessions are deleted |
| INV-T1 | Invite records follow their retention |
| RL-1 | Rate-limit counters expire |
| EXP-1 | Expired `classifier_eval` records are deleted by the sweeper |
| V8.3.1 | The sweeper re-checks the mailbox owner before raising an item |

## Tests that must pass

- `un_05_ac2_overdue_job_expires_with_needs_attention` (service integration: queued job, clock at `due_at + 1 h + 1 s`; status `expired`, item `job_expired`)
- `un_05_ac2_every_job_terminal_within_job_ttl` (property, `proptest`: random sets of jobs with random deliveries and failures driven through T-701's runner, then the clock advances past `due_at + 1 h` and one sweep runs; every job is terminal)
- `un_01_ac3_expired_outcome_logged_and_stored` (service integration: outcome `expired` on the record and one `unsub_job_outcome` event)
- `na_01_ac2_item_deleted_after_ttl` (service integration)
- `inv_2_sweeper_deletes_expired_jobs` and `inv_2_sweeper_deletes_expired_needs_attention` (service integration)
- `job_1_no_job_survives_expiry_plus_interval` (service integration: terminal job at `expires_at + 15 min` is gone after a sweep; a terminal job one second before its `expires_at` is kept)
- `na_t1_no_item_survives_30_days` (service integration)
- `ses_1_sweeper_deletes_expired_sessions` (service integration)
- `inv_t1_invites_deleted_when_purge_due` (service integration)
- `rl_1_rate_limit_counters_deleted` (service integration)
- `exp_1_sweeper_deletes_expired_eval_records` (service integration)
- `asvs_v8_3_1_sweeper_rechecks_owner` (service integration: job whose mailbox belongs to another user gets no item)
- `api_int_2_rejects_non_scheduler_caller` (service integration: a valid token for the Cloud Tasks account is refused)
- `api_int_2_failed_step_returns_500_and_others_run` (service integration: fake store fails `sessions().expires_by`; the other counts are still non-zero)
- `api_int_2_more_flag_when_batch_full` (service integration: 501 expired items)

## Edge cases and traps

- Use one `now` per sweep from `Clock`; never `SystemTime`.
- The expire write must be conditional on the version you read; a job that `unsub` finished a moment ago must not be overwritten with `Expired`.
- Never delete a non-terminal job without expiring it first and raising its item; deleting it would fail silently (XC-04).
- `worker` holds KMS rights (S4 2) because it decrypts the job target and encrypts item fields. It has no Drive access and no Gmail access; do not mint access tokens here.
- The response holds counts only. Never put IDs, links or names in the body or the log.
- The sweep must be safe to run twice at once (Cloud Scheduler retries): every delete is idempotent and every state change is conditional.

## Out of scope

- Firestore TTL policies and the Cloud Scheduler job: T-1102.
- Deleting collected job outcomes after the History append: T-609.
- Account deletion sweeps (24-hour sweep of a deleted user's records): T-803.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cargo run -p worker` starts and answers `401` to an unauthenticated sweep.
