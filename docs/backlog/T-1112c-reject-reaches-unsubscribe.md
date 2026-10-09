# T-1112c: wire the runner into the api e2e mode and prove reject -> unsubscribe

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 100 lines of code plus tests | T-1112a, T-1112b |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

In e2e mode the api uses the local runner when `UNSUB_BASE_URL` is set, and a service-level test proves a rejected one-click newsletter ends with the unsub-testbed recording the POST.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/startup_e2e.rs` | Use `LocalJobRunner` instead of `FakeJobScheduler` when `UNSUB_BASE_URL` and `UNSUB_E2E_CALLER_TOKEN` are set; keep `FakeJobScheduler` otherwise |
| Create | `backend/crates/api/tests/reject_unsubscribe_e2e.rs` | The proof below, with fakes in process |

## Behaviour

1. Behaviour with the variables unset is exactly today's.
2. The proof starts `unsub` (e2e mode), the testbed and fake-google in process or as child processes on free loopback ports.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `reject_then_unsubscribe_reaches_the_testbed` (reject a seeded one-click message, wait past the undo window, assert the testbed recorded the POST and the job record is `done`)
- `undo_before_the_window_prevents_the_unsubscribe`
- `api_e2e_without_unsub_env_uses_the_fake_scheduler`

## Out of scope

Nightly confirmation sweeps, the demo scripts.

## Done when

Tests pass, belt green, PR body shows the recorded POST. Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
