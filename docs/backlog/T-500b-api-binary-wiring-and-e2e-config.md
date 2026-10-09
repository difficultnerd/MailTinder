# T-500b: api binary: production wiring and the end-to-end test configuration

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 450 lines of code plus tests | T-500, T-701, T-205a, T-205b, T-206, T-301 |

**Read only these spec sections:** S4 (service wiring and `Ports`), S6 section on secrets and logging, S10 sections 3.2 (Integration row) and 3.3 (the "`api` and `unsub` built in test configuration" paragraph) (`docs/specs/S10-test-strategy.md`); `backend/crates/unsub/src/main.rs` (the finished model for this task); `backend/crates/api/tests/skeleton.rs` (how `AppState` is built). Nothing else is needed.

## Why this task exists

T-500 promised `backend/crates/api/src/main.rs` ("load config and secrets, build real `Ports`, `obs::init`, bind `$PORT`") but merged with T-001's one-line stub, `fn main() {}`. Nothing tested that the binary starts. `unsub` and `worker` have real mains; `api` does not, so the product cannot be run, and T-1101a's `e2e` check cannot pass (found in the independent review of PR #29). This task finishes what T-500 promised and adds the test configuration S10 3.3 describes.

## Goal

`cargo run -p api` starts a real HTTP service that answers `GET /api/v1/healthz` with 200. Two modes: **production** (real GCP adapters, exactly like `unsub`) and **e2e** (compiled only with the `testkit` feature and switched on at run time), where storage is the Firestore emulator, keys and secrets and the job scheduler are `testkit` fakes, and Google (OAuth, Gmail, Drive) is the local `fake-google` server.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/main.rs` | Replace the stub. Thin: call `startup::run()`, map the result to `ExitCode`, log one line on failure |
| Create | `backend/crates/api/src/startup.rs` | `SetupError`, `build_production_ports`, `run()`; reads config from the environment |
| Create | `backend/crates/api/src/startup_e2e.rs` | `#[cfg(feature = "testkit")]` only: `build_e2e_ports`, `LoopbackEgress` |
| Change | `backend/crates/api/src/config.rs` | `ApiConfig::from_lookup(impl Fn(&str) -> Option<String>)` (testable without touching the process environment) and `from_env()` calling it |
| Change | `backend/crates/api/src/lib.rs` | `pub mod startup;` and, under `testkit`, `pub mod startup_e2e;` |
| Change | `backend/crates/api/Cargo.toml` | Make `testkit` an optional dependency enabled by the `testkit` feature (keep it in `[dev-dependencies]` for tests); add whatever `unsub` already uses (`adapters-gcp`, `adapters-gmail`, `egress`, `tokio` with `rt-multi-thread` and `signal`) |
| Create | `backend/crates/api/tests/startup.rs` | Process and unit tests below |

## Behaviour

1. **Environment (production).** Required: `GOOGLE_CLOUD_PROJECT`, `APP_ORIGIN` (exact origin, `https`, no trailing slash, no path), `GOOGLE_OAUTH_CLIENT_ID`, `UNSUB_BASE_URL`, `UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`. Optional: `PORT` (default 8080). A missing or empty variable is `SetupError::Missing(name)`; an invalid value is `SetupError::Invalid(name)`. Never print or log a value, only the variable name.
2. **Production wiring.** Build `Ports` as `unsub/src/main.rs` does: `MetadataTokenSource` and `GcpHttp`, `ProdEgress::new(Service::Api, ...)` (use the service variant that exists for `api`; if none, add it in `egress` with a test), `FirestoreStore`, `EnvelopeKeyService` over `CloudKms` (key `user-data`), `KmsSystemKeyService` (key `system-fields`), `SecretManagerSecrets`, `CloudTasksScheduler` (queue `unsubscribe`, using `UNSUB_*`), `GoogleCallerVerifier`, `GoogleIdentity`, `GmailProvider`, `DriveAppFolder`, `models: Vec::new()`. Fetch `SecretName::GoogleOAuthClientSecret`, `LogPseudonymHmacKey` (the `rate_key`) and `EmailLookupHmacKey` through `Secrets`. Copy the region, key ring and endpoint constants from `unsub` into a shared place only if that is a small change; otherwise duplicate them with a comment naming `unsub/src/main.rs`.
3. **Service.** `obs::init("api", StdoutSink, SystemClock)`, `obs::register_http_routes(ROUTE_TEMPLATES)`, `AppState` built as `tests/skeleton.rs` builds it, bind `0.0.0.0:$PORT`, serve `build_router(state)` with graceful shutdown on `SIGTERM` (Cloud Run) and `SIGINT`.
4. **Failure.** Any `SetupError` logs exactly one line through `tracing` (`event = "op"`, `route = "api.startup"`, `outcome = "failure"`, `error = <variant name>`), then exits with `ExitCode::FAILURE`. No `unwrap`, `expect`, `println!` or panic anywhere in the startup path.
5. **E2E mode is doubly guarded.** It starts only when the crate was built with the `testkit` feature AND the environment has `MT_E2E=1`. With `MT_E2E=1` in a build without `testkit` the binary refuses to start (`SetupError::E2eNeedsTestkit`). A `testkit` build without `MT_E2E=1` behaves exactly as production. Expose `pub fn e2e_mode_enabled() -> bool` (false when the feature is off) so T-1101a mounts its test-only routes only when it is true.
6. **E2E wiring** (`startup_e2e.rs`). Required: `FIRESTORE_EMULATOR_HOST` (`host:port`), `FAKE_GOOGLE_URL` (`http://127.0.0.1:<port>`), `APP_ORIGIN` (loopback `http` allowed here and nowhere else), `GOOGLE_OAUTH_CLIENT_ID`. Start from `testkit::fake_ports()` and replace: `store` with `FirestoreStore` over `GcpHttp::with_emulator(...)` and a fixed `owner` token (project `demo-mailtinder`); `identity`, `gmail`, `app_folder` with the real `GoogleIdentity`, `GmailProvider`, `DriveAppFolder` pointed at the `fake-google` endpoints (read the route table in `backend/crates/fake-google/src` for the exact paths); `egress` with `LoopbackEgress`, a small `HttpEgress` that allows requests to exactly the `FAKE_GOOGLE_URL` socket and refuses everything else. Keys, system keys, secrets, scheduler and caller verifier stay the `testkit` fakes; seed `FakeSecrets` with generated random 32-byte HMAC keys and the literal client secret `test-only-not-a-secret`. Never enable the `egress` crate's `test-policy` feature (its header forbids it in a service's `[dependencies]`).
7. **No secrets in the repo.** All values come from the environment; nothing real is committed.

## Acceptance criteria

- `api_exits_nonzero_without_required_config` (process test, `env!("CARGO_BIN_EXE_api")`, empty environment): exits non-zero within 5 s; stdout and stderr contain no `panicked`, and none of the values given in a second run where `GOOGLE_OAUTH_CLIENT_ID` is set to `canary-client-id-123` and another variable is left missing (the canary string must not appear).
- `api_config_rejects_bad_app_origin` (unit, `ApiConfig::from_lookup`): trailing slash, a path and `http` for a non-loopback host are `Invalid("APP_ORIGIN")`. (An empty value is `Missing("APP_ORIGIN")`, not `Invalid`: absence and emptiness are the `Missing` case; only a present but malformed origin is `Invalid`.)
- `api_config_reads_port_default_and_override`: unset gives 8080; `PORT=9000` gives 9000; `PORT=abc` is `Invalid("PORT")`.
- `api_e2e_env_refused_without_testkit_build` (`#[cfg(not(feature = "testkit"))]`, process test): `MT_E2E=1` makes the binary exit non-zero.
- `api_e2e_ports_serve_healthz` (`#[cfg(feature = "testkit")]`): `build_e2e_ports` is generic over the store, so the test passes `InMemoryServerStore`; build the router from it and `GET /api/v1/healthz` returns 200 with the standard security headers.
- `api_loopback_egress_allows_only_the_fake_google_socket` (`#[cfg(feature = "testkit")]`): a request to the configured socket reaches a local listener; a request to any other host or port is refused before any connection is opened.
- `api_testkit_build_without_e2e_env_behaves_as_production` (`#[cfg(feature = "testkit")]`): with `MT_E2E` unset, `e2e_mode_enabled()` is false.
- Keep the existing `tests/skeleton.rs` and all other `api` tests green.

## Gotchas

- Cloud Run sends `SIGTERM` and waits 10 s; shut down gracefully within that.
- Do not add a runtime fallback from e2e to production or the reverse; the mode is decided once at start.
- `ProdEgress` resolves DNS and blocks private ranges (T-306). The e2e mode must not weaken it; it simply does not use it.
- Follow `CLAUDE.md`: no `unwrap` or `expect` anywhere, tests included; no blanket lint suppressions.
- Do not edit `.github/workflows/`, `CLAUDE.md` or `tools/apply_branch_protection.sh`.

## Out of scope

- Test-only HTTP routes such as `/internal/test/invites` (T-1101a).
- `unsub` and `worker` test configurations (a follow-up, needed by T-1101b's unsubscribe journeys).
- The `advance-clock` route and a virtual clock in e2e mode (e2e mode uses the system clock).
- Containers and deployment (T-1104).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- **Show it runs.** In the PR body paste the output of: the `testkit` build started with the e2e environment against a running Firestore emulator and `fake-google`, then `curl -si http://127.0.0.1:$PORT/api/v1/healthz` (status line and headers). A PR that does not show this is not done.
- In the PR body add a two-column list: each file this task's table promises, and the line count that landed.
- Definition of done in S10 10.4.
