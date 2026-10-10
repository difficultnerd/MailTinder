# T-1101g: End-to-end unsubscribe stack: due-time control and testbed trust

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 300 lines of code plus tests | T-500b, T-1101a, T-1101b |

**Read only these spec sections:** S10 sections 3.3 (including the line that test-only routes must not exist in release builds), 6.2 (testbed and the test egress policy) and 6.3 first paragraph (fake `JobScheduler`, virtual `Clock`) (`docs/specs/S10-test-strategy.md`); `docs/security/asvs-l2-register.md` row V13.4.2; `backend/crates/api/src/startup_e2e.rs` lines 95 to 214 (the existing clock and `MT_E2E_UNSUB_DELAY_S` / `MT_E2E_TIME_SCALE` mechanism); `backend/crates/unsub/src/startup_e2e.rs` (`LoopbackEgress`); `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` "Out of scope"; `docs/backlog/T-1101a-e2e-harness-and-required-check.md` rows on `advance-clock` and the release check. Nothing else is needed.

**Why this task exists (found 2026-10-10).** T-500b deferred three things to "a follow-up, needed by T-1101b's unsubscribe journeys": the `unsub` and `worker` test configurations, and the `advance-clock` route with a virtual clock. No task was ever written for that follow-up. T-1101e (reject-then-undo, reject-and-unsubscribe) blocked on exactly these gaps, reported by the builder on 2026-10-10 00:30 UTC: `unsub` (e2e) cannot reach the testbed over TLS, no virtual clock or `POST /internal/test/advance-clock` exists, the api and unsub processes do not share a clock, and `scripts/e2e.sh` does not pass the e2e environment to them.

## Goal

The e2e stack can run the unsubscribe journeys: `api` and `unsub` run in the e2e test configuration with a way to make a queued unsubscribe job due within seconds, and `unsub` can complete its one-click POST to the testbed over TLS, **with zero change to production behaviour, production policy, the `egress` crate or release binaries**.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/unsub/src/startup_e2e.rs` | `LoopbackEgress` trusts the testbed's per-run CA via `reqwest::ClientBuilder::add_root_certificate` (already `testkit`-gated and literal-loopback-only); read the certificate from `UNSUB_TESTBED_CA_FILE` |
| Change | `backend/crates/unsub-testbed/src/lib.rs` | write the per-run CA **certificate** (public part only) to `UNSUB_TESTBED_CA_FILE` at start (`test_ca_pem(&self) -> &'static [u8]` already exists in-process); expose the HTTPS listener address (`https_addr`) in the port file |
| Change | `scripts/e2e.sh` | export the testbed URL as a literal loopback IP (`https://127.0.0.1:<port>`, because `LoopbackEgress` refuses `localhost`); pass `MT_E2E=1`, `UNSUB_E2E_CALLER_TOKEN`, `UNSUB_TESTBED_URL`, `UNSUB_TESTBED_CA_FILE`, `UNSUB_BASE_URL` and the due-delay setting to the right processes; generate the caller token per run and never print it |
| Change (only if Algorithm step 1 requires it) | `backend/crates/api/src/startup_e2e.rs`, `routes/testkit.rs`, `unsub/src/startup_e2e.rs` | bounded advance route (below) |
| Change | `backend/crates/api/tests/release_routes.rs` and the matching `unsub` test | release and testkit-without-`MT_E2E` builds expose no test routes and no extra roots |
| Change | `docs/security/asvs-l2-register.md` row V13.4.2 and the header comment in `backend/crates/api/src/routes/testkit.rs` | updated in the SAME change if and only if an advance route is added (they currently say advance-clock is not implemented) |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1101g` |

## Types and signatures

```rust
// unsub (testkit feature + MT_E2E=1 only): LoopbackEgress::new(endpoints, ca_pem: Option<&[u8]>) adds exactly that one root.
// Only if step 1 needs it: POST /internal/test/advance-clock {"seconds": u64 (max 600)} -> 204
//   compiled only with feature "testkit"; answers 404 unless MT_E2E=1; requires the e2e caller token (constant-time compare);
//   unsub's copy listens on loopback only.
```

## Algorithm

1. **Prefer the existing mechanism; do not build a clock first.** The api e2e wiring already has `MT_E2E_UNSUB_DELAY_S` / `MT_E2E_TIME_SCALE` (`startup_e2e.rs` 168 to 214), and it deliberately keeps the **real** clock because `fake-google` checks provider-token expiry and OAuth claims against real time and the Firestore emulator uses real time. Make a queued unsubscribe job due after a few real seconds (for example 3 s) through that setting, and amend T-1101e's steps to wait for the due time (poll up to 10 s past it) instead of `advance_clock(6 minutes)`. This satisfies SW-05 AC2 (undo before due sends nothing: undo within the first second, then wait past due, zero requests) and UN-01 AC1 (runs once) without moving time.
2. **Only if step 1 cannot satisfy an acceptance criterion**, add the bounded advance route above. Then: it moves `api`'s and `unsub`'s job clocks together (a call from api to unsub's loopback route); `fake-google` and the Firestore emulator are NOT moved, so journeys must stay inside token and session lifetimes (cap each advance at 10 minutes and never advance past a session or token expiry); state in the PR which checks the route could affect and prove none was relaxed. **No production time, expiry, session or token check may be weakened to make a journey pass.** The route requires the `testkit` feature AND `MT_E2E=1` at runtime (a `testkit` build without `MT_E2E=1` answers 404 and refuses e2e startup, as the invite route already does).
3. **Testbed trust.** The testbed writes only its CA certificate (mode 600, run directory; never a private key). `unsub`'s e2e binary reads it and adds it as one extra root on `LoopbackEgress`'s `reqwest` client. The production `egress` crate, its SSRF rules, allow-list, redirect refusal and its existing `TestOverride` hook are **not changed**. If a journey seems to require changing `egress`, stop and report BLOCKED with the exact reason and which existing hook is insufficient.
4. **Environment wiring** in `scripts/e2e.sh` as in the Files table. Secrets are generated per run and passed via environment or a mode-600 file, never via argv.
5. **Prove the plumbing before the journeys:** one smoke test queues a one-click job through the api, waits for the due time, and sees exactly one POST at the testbed with the fixed body over TLS (T-1101e keeps the full journeys).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| E2E-INFRA AC1 | A queued unsubscribe job becomes due within the e2e delay and the testbed receives exactly one POST |
| E2E-INFRA AC2 | `unsub` completes a TLS one-click POST to the testbed trusting only the testbed's per-run CA |
| E2E-INFRA AC3 | Release builds, and `testkit` builds without `MT_E2E=1`, have no test routes and no extra trust roots |
| E2E-INFRA AC4 | The `egress` crate is unchanged and its existing tests pass unmodified |
| ASVS V13.4.2 | Release builds have no test routes (`invites`, and `advance-clock` if added) |

## Tests that must pass

- `e2e_infra_ac1_job_due_and_posted_once` (end to end, ignored, run by `scripts/e2e.sh`)
- `e2e_infra_ac2_unsub_trusts_only_testbed_ca` (integration)
- `e2e_infra_ac3_release_and_unflagged_testkit_have_no_test_routes_or_roots` (unit, `api` and `unsub`)
- `asvs_v13_4_2_unsub_release_has_no_test_routes` (unit, `unsub`)
- the existing `egress` test suite, unmodified, green
- Run `python3 tools/check_ac_coverage.py` and confirm a deliberately missing `e2e_infra_*` test fails the check (the `E2E-INFRA` story id format is not yet proven to parse).

## Edge cases and traps

- The private key of the test CA never leaves the testbed process; only the certificate is shared.
- Anything added in e2e mode must be compiled out of release builds and refuse to run without `MT_E2E=1`; a runtime flag alone is not enough, a cargo feature alone is not enough (both, and a test asserts it).
- Do not loosen `egress` or `LoopbackEgress` (hosts, redirects, schemes, address ranges); the testbed URL must be a literal loopback IP.
- Constant-time comparison for the caller token; no token, certificate path or Authorization header in logs.
- No real hosts, no real mail.

## Out of scope

- The full journeys (T-1101e); mixed Feed and file (T-1101d, T-1101f); `swipe`/`undo` events (T-1114).
- Cloud Tasks and real scheduling (staging smoke, T-1105).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1), including semgrep and the release-build checks.
- This is `strong` tier and touches a trust boundary: it must be reviewed by a different model vendor than the builder (the factory's independent-reviewer rule) with explicit confirmation that nothing in production `egress` or the production policy changed.
- Definition of done in S10 10.4.

## Buy / borrow / build

Borrow: the TLS library's standard custom-root API (`add_root_certificate`) and the existing e2e due-delay mechanism. Build: only the CA hand-off and, if unavoidable, the bounded advance route. Rejected: a single virtual clock across all processes (fake-google and the emulator use real time), pausing an in-process runtime clock (the services are separate processes), and adding a root-injection feature to `egress` (a second trust-boundary surface in the SSRF crate).
