# T-1112d: run the unsub no-testkit start-up refusal test in CI

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | one CI step plus its revert-check evidence | T-1112a |

**Read only these spec sections:** `docs/backlog/T-1112a-unsub-e2e-startup-mode.md` (what was built), the T-1112a independent review finding F1 that this task closes, `.github/workflows/ci.yml` (the `e2e` job), and `backend/crates/unsub/tests/startup_e2e.rs` (the test). Nothing else is needed.

## Why this task exists

T-1112a's acceptance criterion `unsub_e2e_mode_refused_without_testkit_build` (S10 3.3, behaviour 3) is a process test gated `#[cfg(not(feature = "testkit"))]`: it runs the real `unsub` binary, built **without** the `testkit` feature and with `MT_E2E=1`, and asserts it refuses to start instead of falling into production wiring. Every CI gate that compiles `unsub`'s tests uses `--all-features`, which turns `testkit` on, so the whole `no_testkit` module is compiled out and the control never runs. A regression that dropped the `#[cfg(not(feature = "testkit"))]` refusal arm in `backend/crates/unsub/src/main.rs` would pass every required check. This is the PR's main safety property, so it needs a gate that actually runs it.

T-1112a was forbidden from editing `.github/workflows/`, so the step is raised here instead (T-1112a review finding F1). It mirrors the `-p api --no-default-features` steps T-1101a added to the `e2e` job for `api_e2e_env_refused_without_testkit_build`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `.github/workflows/ci.yml` | In the `e2e` job, beside the `-p api --no-default-features` steps (the lines that run `--test release_routes` and `--test startup`), add `cargo test --locked -p unsub --no-default-features --test startup_e2e`. The `rust` job's `cargo test --all-features` cannot run it (it enables `testkit`), so the step must build `unsub` with its default (empty) feature set |

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- The new step runs `unsub_e2e_mode_refused_without_testkit_build` and passes, so the CI log shows the control executed.
- Removing the `#[cfg(not(feature = "testkit"))]` refusal arm in `backend/crates/unsub/src/main.rs` makes the new step fail (S10 10.4: show it once in the pull request).
- `cd backend && cargo test --locked -p unsub --no-default-features --test startup_e2e` passes locally with the default feature set.

## Notes

- Keep the step per-package, like the `api` ones; do not widen it to the workspace.
- The test needs no Firestore emulator, Java or `fake-google`: it only asserts a non-zero, non-panicking refusal, so it can sit beside the `api` steps before `scripts/e2e.sh`.
- Do not edit `CLAUDE.md`.

## Done when

The new step is green in CI, and the pull request shows it failing when the refusal arm is removed (S10 10.4).
