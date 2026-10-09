# T-1112b: local job runner for e2e mode (delivers due jobs, honours undo)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 150 lines of code plus tests | T-500b, T-500c |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

A `JobScheduler` for e2e mode that holds queued jobs in memory and a background loop that delivers each job to a configured URL when it is due, so a reject actually reaches `unsub`. Tested against a small local test server; it does not need `unsub`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/local_runner.rs` | `#[cfg(feature = "testkit")]`: `LocalJobRunner` implementing the scheduler port: enqueue, cancel (undo), a loop delivering due jobs with the test bearer token to `UNSUB_BASE_URL` + the internal route, retry with the production back-off table scaled by `MT_E2E_TIME_SCALE` |
| Change | `backend/crates/api/src/lib.rs` | `pub mod local_runner;` under the feature |
| Change | `backend/crates/api/src/startup_e2e.rs` | Apply the `MT_E2E_UNSUB_DELAY_S` override to the tunables in e2e mode only |
| Create | `backend/crates/api/tests/local_runner.rs` | Tests below, using a tiny `axum` server as the target |

## Behaviour

1. Delivery goes only to the loopback target in the configuration; any other host is refused at construction.
2. A due job is delivered once; a cancelled job never; a 5xx or timeout is retried with scaled back-off; a 4xx marks the job failed without retry.
3. The undo window is NOT the runner's to set: the api computes `due_at = now + tunables.unsub_delay` (`backend/crates/domain/src/swipe.rs`, `tunables.rs`) and passes it to the scheduler port. In e2e mode only, `startup_e2e.rs` applies an override read from `MT_E2E_UNSUB_DELAY_S` where the tunables are built (default: the production value); the runner just delivers when `due_at` passes (its clock may be sped up with `MT_E2E_TIME_SCALE`, test only).

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `local_runner_delivers_a_due_job_once`
- `local_runner_does_not_deliver_a_cancelled_job`
- `local_runner_retries_after_5xx_with_scaled_backoff`
- `local_runner_does_not_retry_a_4xx`
- `local_runner_refuses_a_non_loopback_target`
- `e2e_unsub_delay_override_changes_due_at` (set `MT_E2E_UNSUB_DELAY_S=2`: a reject schedules `due_at` two seconds ahead; unset: the production delay)

## Out of scope

Wiring into the api startup (T-1112c), `unsub` itself, the demo scripts.

## Done when

Tests pass, belt green, PR body shows the test output. Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
