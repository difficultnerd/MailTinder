# T-1112: unsub test configuration and a local job runner, so reject -> unsubscribe works in the demo

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 350 lines of code plus tests | T-500b, T-500c, T-701, T-702, T-703, T-708 |

**Read only these spec sections:** `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` and `backend/crates/api/src/startup_e2e.rs` (the pattern), `backend/crates/unsub/src/main.rs` and `runner.rs` (what `unsub` does and which route it serves), `backend/crates/testkit` (`FakeJobScheduler`, `FakeCallerVerifier`), `backend/crates/unsub-testbed` (the local unsubscribe sites). Nothing else is needed.

## Goal

Today a reject queues an unsubscribe job in the api's fake scheduler and nothing ever runs it, because `unsub` has no test configuration. This task makes the whole reject -> unsubscribe path work locally, with fakes only: `unsub` runs in an e2e mode like `api` does, and a small local runner delivers each queued job to `unsub` when it is due. The owner needs this to try the app on a phone with everything working.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/unsub/src/startup_e2e.rs` | `#[cfg(feature = "testkit")]`: `unsub` e2e wiring (Firestore emulator store, `testkit` fakes for keys, secrets and caller verification, `fake-google` for Gmail, `unsub-testbed` reachable through a loopback-only egress) |
| Change | `backend/crates/unsub/src/main.rs` | Use `startup_e2e` only when built with `testkit` AND `MT_E2E=1`; otherwise exactly today's production wiring; `MT_E2E=1` without the feature refuses to start (same rule as `api`) |
| Create | `backend/crates/api/src/local_runner.rs` | `#[cfg(feature = "testkit")]`: a `JobScheduler` that keeps queued jobs in memory and a background loop that, when a job is due, POSTs it to `UNSUB_BASE_URL`'s internal route with the test caller identity that `unsub`'s e2e verifier accepts; honours cancel (undo) |
| Change | `backend/crates/api/src/startup_e2e.rs` | Use the local runner instead of `FakeJobScheduler` when `UNSUB_BASE_URL` is set; keep `FakeJobScheduler` otherwise |
| Change | `backend/crates/unsub/Cargo.toml`, `backend/crates/api/Cargo.toml` | `testkit` feature wiring (optional dependencies, as in T-500b) |
| Create | `backend/crates/unsub/tests/startup_e2e.rs`, `backend/crates/api/tests/local_runner.rs` | Tests below |

## Behaviour

1. Both services bind **127.0.0.1 only** in e2e mode (same loopback rule as T-500c). The runner talks only to the `unsub` socket from its configuration.
2. The undo window in e2e mode is read from `MT_E2E_UNDO_WINDOW_S` (default the production value; the demo sets 5) so a reject visibly completes in seconds.
3. A due job is delivered once; a 5xx or timeout is retried with the production back-off table scaled by `MT_E2E_TIME_SCALE`; a cancelled job is never delivered. Delivery results update the job record exactly as the real Cloud Tasks path does.
4. After delivery, the one-click POST reaches `unsub-testbed` and the mailto path sends through `fake-google`; the Needs Attention list shows the outcome for senders that cannot be automated (T-705 behaviour, unchanged).
5. Nothing here exists in a production build: no `testkit` feature, no runner, no verifier that accepts the test identity.

## Acceptance criteria (named tests that fail if the behaviour is removed)

- `local_runner_delivers_a_due_job_once` and `local_runner_does_not_deliver_a_cancelled_job`.
- `local_runner_retries_after_a_5xx_with_scaled_backoff`.
- `unsub_e2e_mode_refused_without_testkit_build` (process test, build without the feature) and `unsub_testkit_build_without_e2e_env_behaves_as_production`.
- `unsub_e2e_verifier_accepts_only_the_test_identity` (any other caller is 401/403).
- `reject_then_unsubscribe_reaches_the_testbed` (service-level test with fakes: reject a one-click newsletter, advance the runner, assert the testbed recorded the POST and the job record is `done`).
- Existing `unsub` and `api` tests stay green.

## Out of scope

The nightly confirmation sweeps (T-706/T-707 timing), real Cloud Tasks, any change to production wiring, workflows, CI files or `CLAUDE.md`.

## Done when

Tests pass, the belt is green, and the PR body shows the output of a local run: start the e2e stack, reject a seeded one-click message, and show the testbed's recorded POST. Definition of done in S10 10.4.
