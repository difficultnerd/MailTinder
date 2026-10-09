# T-1112a: unsub service e2e start-up mode (boots against fakes, loopback only)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 130 lines of code plus tests | T-500b, T-500c, T-701, T-708 |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

`unsub` can start in an e2e mode, like `api` does (T-500b/T-500c), so it can run next to the fakes. Nothing else changes. This is step 1 of making reject -> unsubscribe work in the demo.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/unsub/src/startup_e2e.rs` | `#[cfg(feature = "testkit")]`: wiring with the Firestore emulator store, `testkit` fakes for keys and secrets, fake-google for Gmail, a loopback-only egress that allows only the unsub-testbed and fake-google sockets, and a caller verifier that accepts ONLY the test identity (a fixed bearer value read from `UNSUB_E2E_CALLER_TOKEN`; the service REFUSES TO START in e2e mode if the token is unset, empty or shorter than 16 characters) |
| Change | `backend/crates/unsub/src/main.rs` | Use `startup_e2e` only when built with `testkit` AND `MT_E2E=1`; otherwise exactly today's production wiring; `MT_E2E=1` without the feature refuses to start |
| Change | `backend/crates/unsub/Cargo.toml` | `testkit` as an optional dependency behind the feature (same pattern as `api`) |
| Create | `backend/crates/unsub/tests/startup_e2e.rs` | Tests below |

## Behaviour

1. In e2e mode `unsub` binds 127.0.0.1 only and serves its health route.
2. The e2e verifier accepts only the configured test token; any other caller gets 401 or 403.
3. Production wiring is byte-for-byte unchanged when the feature or the environment variable is absent.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `unsub_e2e_serves_health_on_loopback_only`
- `unsub_e2e_verifier_accepts_only_the_test_token`
- `unsub_e2e_refuses_empty_unset_or_short_token` (an empty `Bearer ` header must never match)
- `unsub_e2e_mode_refused_without_testkit_build` (process test, build without the feature)
- `unsub_testkit_build_without_e2e_env_behaves_as_production`
- Existing `unsub` tests stay green

## Out of scope

The job runner (T-1112b), the api side, the demo scripts.

## Done when

Tests pass, the belt is green, and the PR body shows `curl` output from the running e2e `unsub` (health 200, and 401 without the token). Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
