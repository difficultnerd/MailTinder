# T-1101g: End-to-end unsubscribe stack: virtual clock and testbed trust

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 300 lines of code plus tests | T-500b, T-1101a, T-1101b |

**Read only these spec sections:** S10 sections 3.3, 6.2 (testbed and the test egress policy), 6.3 first paragraph (fake `JobScheduler`, virtual `Clock`) and 7 (test-only routes must not exist in release builds) (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` "Out of scope"; `docs/backlog/T-1101a-e2e-harness-and-required-check.md` rows on `advance-clock` and the release check. Nothing else is needed.

**Why this task exists (found 2026-10-10).** T-500b deferred three things to "a follow-up, needed by T-1101b's unsubscribe journeys": the `unsub` and `worker` test configurations, and the `advance-clock` route with a virtual clock. No task was ever written for that follow-up. T-1101e (reject-then-undo, reject-and-unsubscribe) blocked on exactly these gaps, reported by the builder on 2026-10-10 00:30 UTC: `unsub` (e2e) cannot reach the testbed over TLS, no virtual clock or `POST /internal/test/advance-clock` exists, the api and unsub processes do not share a clock, and `scripts/e2e.sh` does not pass the e2e environment to them.

## Goal

The e2e stack can run the unsubscribe journeys: `api` and `unsub` (and `worker` if the journeys need its sweep) run in a test configuration with one shared virtual clock that a test can advance, and `unsub` can complete its one-click POST to the testbed over TLS, **with zero change to production behaviour, policy or release binaries**.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/startup_e2e.rs`, `local_runner.rs`, `routes/testkit.rs` | e2e virtual clock; `POST /internal/test/advance-clock {seconds}` (feature `testkit` only) |
| Change | `backend/crates/unsub/src/startup_e2e.rs` | e2e clock source; test route to advance (feature `testkit` only); trust the testbed CA (below) |
| Change | `backend/crates/unsub-testbed/src/...` | write the per-run test CA certificate (PEM, public part only) to the path in `UNSUB_TESTBED_CA_FILE` at start |
| Change | `backend/crates/egress/src/...` | an e2e-only way to add exactly that one CA to the trust roots, behind a cargo feature that only the `unsub` e2e binary enables |
| Change | `scripts/e2e.sh` | pass `MT_E2E=1`, `UNSUB_E2E_CALLER_TOKEN`, `UNSUB_TESTBED_URL`, `UNSUB_TESTBED_CA_FILE`, `UNSUB_BASE_URL` to the right processes; generate the caller token per run, never print it |
| Change | `backend/crates/api/tests/release_routes.rs` (and the matching `unsub` test) | release builds expose no test clock route and no extra trust roots |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1101g` |

## Types and signatures

```rust
// api: compiled only with feature "testkit"
// POST /internal/test/advance-clock  body {"seconds": u64}  -> 204; requires the e2e caller token
// unsub: same route on its loopback e2e listener; api's route calls unsub's so both clocks move together
// testbed: fn ca_pem(&self) -> String  (already in-process; also written to $UNSUB_TESTBED_CA_FILE)
// egress: #[cfg(feature = "e2e-trust")] pub fn with_extra_root_pem(self, pem: &[u8]) -> Result<Self, EgressError>
```

## Algorithm

1. **One clock.** In e2e mode the api builds its `Clock` from a shared `VirtualClock` (starts at real now, advances only on request). The api's advance route moves it, then calls `unsub`'s test route (loopback, caller token) so due jobs in `unsub` become due at the same virtual instant. Pick the simplest mechanism that keeps the two clocks equal (a call from api to unsub is expected); state it in the PR.
2. **Testbed trust.** The testbed already generates a per-process CA. Write only the CA **certificate** to `$UNSUB_TESTBED_CA_FILE` (mode 600, run directory), never a private key. The `unsub` e2e binary reads it at start and builds its egress client with that one extra root through the e2e-only constructor. The production egress policy (SSRF refusals, allow-list, no redirects) is **unchanged**: the loopback testbed address is allowed only by the existing test policy described in S10 6.2, which is unchanged too.
3. **No shortcut around the test egress policy.** Do not add `test-policy` to a service crate; if the only way to complete (2) is a change the repository forbids in service crates, stop and report BLOCKED with the exact rule, do not weaken the rule.
4. **Environment wiring** in `scripts/e2e.sh` as in the Files table. Secrets (the caller token) are generated per run and passed via environment or mode-600 file, never via argv.
5. **Prove the plumbing before the journeys:** one smoke test enqueues a due-in-5-minutes one-click job through the api, advances the clock 6 minutes, and sees exactly one POST arrive at the testbed with the fixed body (this is the T-1101e journey in miniature; T-1101e keeps the full journeys).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| E2E-INFRA AC1 | Advancing the virtual clock makes a queued unsubscribe job due in `unsub`; the testbed receives it exactly once |
| E2E-INFRA AC2 | `unsub` completes a TLS one-click POST to the testbed trusting only the testbed's per-run CA |
| E2E-INFRA AC3 | A release build of `api` and `unsub` has no test clock route and no extra trust roots |
| E2E-INFRA AC4 | The production SSRF and allow-list behaviour of `egress` is unchanged (its existing tests pass unmodified) |
| ASVS V13.4.2 | Release builds have no test routes (`advance-clock`, `invites`) |

## Tests that must pass

- `e2e_infra_ac1_clock_advance_makes_job_due_once` (end to end, ignored, run by `scripts/e2e.sh`)
- `e2e_infra_ac2_unsub_trusts_only_testbed_ca` (integration)
- `e2e_infra_ac3_release_has_no_test_clock_or_roots` (unit, `api` and `unsub`, features off)
- the existing `egress` test suite, unmodified, green

## Edge cases and traps

- The private key of the test CA never leaves the testbed process; only the certificate is shared.
- A trust root added in e2e mode must not be usable outside e2e: feature-gated at compile time, absent from release builds, asserted by a test (AC3). A runtime flag alone is not enough.
- Do not loosen `egress` (hosts, redirects, schemes, address ranges). A diff that touches those rules fails review.
- The virtual clock must not leak into non-e2e code paths; production uses `production_clock()`.
- Do not print tokens, certificate paths with secrets or the Authorization header in logs.
- No real hosts, no real mail.

## Out of scope

- The full journeys (T-1101e); mixed Feed and file (T-1101d, T-1101f); `swipe`/`undo` events (T-1114).
- Cloud Tasks and real scheduling (staging smoke, T-1105).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1), including semgrep and the release-build checks.
- Because this is `strong` tier and touches a trust boundary, the security reviewer must confirm the egress change loosens nothing (cross-vendor review, rule R2).
- Definition of done in S10 10.4.

## Buy / borrow / build

Borrow: the TLS library's standard custom-root API (no hand-rolled certificate handling). Build: the shared virtual clock and the two test routes (project-specific). Alternative rejected: pausing a single in-process runtime clock (the services are separate processes).
