# T-1101a: End-to-end harness, first journey and the `e2e` required check

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 450 lines of code plus tests | M6, M7, T-205a, T-205b, T-206, T-708, T-1001a, T-1002b |

**Read only these spec sections:** S10 sections 3.2 (Integration row), 3.3, 4.2 (control API sentence), 10.1 and 10.3 (`docs/specs/S10-test-strategy.md`); S7 section 3.4 (invite token in the URL fragment) (`docs/specs/S7-api-contract.md`); S2 AU-03 AC1; the "Backlog seeds for S12" section of `docs/planning-roadmap.md` (the `e2e` required check, T6). Nothing else is needed.

## Goal

One command, `scripts/e2e.sh`, brings up the whole local stack (fake-google, unsub-testbed, Firestore emulator, `api` and `unsub` in test configuration, and the Flutter web build served with the `firebase.json` headers) and runs browser journeys against it. This task builds the harness and the first journey, adds the `e2e` CI job, and makes it a required pull request check (decision T6).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/e2e.sh` | Build, start, run, stop; `--journey <name>` filter |
| Create | `scripts/e2e_host.py` | Stdlib static server plus `/api/**` proxy, adding the headers from `firebase.json` |
| Create | `scripts/e2e/api.env`, `scripts/e2e/unsub.env` | Test configuration values (fake hosts, emulator, test keys; no real secrets) |
| Create | `backend/crates/e2e/Cargo.toml`, `src/lib.rs` | Harness library: `Stack`, `Ui`, `FakeGoogle`, `TestControl` |
| Create | `backend/crates/e2e/tests/sign_in.rs` | First journey |
| Change | `backend/Cargo.toml` | Add `crates/e2e` to the workspace; add `fantoccini` (WebDriver client, rustls feature) as its dependency |
| Change | `backend/crates/api/src/...` (test routes module, `testkit` feature only) | `POST /internal/test/invites` returning a raw invite token, beside T-604/T-605's `advance-clock` route |
| Create | `backend/crates/api/tests/release_routes.rs` | Proves test routes are absent without the feature |
| Change | `app/lib/main.dart` | When `kE2eBuild`, call `SemanticsBinding.instance.ensureSemantics()` |
| Change | `.github/workflows/ci.yml` | New `e2e` job |
| Change | `tools/apply_branch_protection.sh` | Add `"e2e"` to `contexts` |
| Change | `CLAUDE.md` | One line: journeys run with `scripts/e2e.sh` (CI job `e2e`, about 10 minutes) |

## Types and signatures

```rust
// backend/crates/e2e/src/lib.rs (dev-only crate, never a dependency of a service)
pub struct Stack { pub app_url: Url, pub fake_google: Url, pub testbed: Url, pub api_internal: Url }
impl Stack { pub fn from_env() -> Result<Self, E2eError>; } // MT_E2E_APP_URL, MT_E2E_FAKE_GOOGLE_URL, MT_E2E_TESTBED_URL, MT_E2E_API_URL

pub struct Ui { /* fantoccini::Client */ }
impl Ui {
    pub async fn open(stack: &Stack, hash_path: &str) -> Result<Self, E2eError>; // new Chrome session via ChromeDriver
    pub async fn tap(&self, label: &str) -> Result<(), E2eError>;                // flt-semantics[aria-label="..."]
    pub async fn type_into(&self, label: &str, text: &str) -> Result<(), E2eError>;
    pub async fn wait_for_text(&self, text: &str, timeout: Duration) -> Result<(), E2eError>;
    pub async fn switch_to_popup(&self) -> Result<(), E2eError>;                 // for step-up (T-1001b)
    pub async fn storage_dump(&self) -> Result<String, E2eError>;                // localStorage, sessionStorage, IndexedDB names, Cache Storage keys
    pub async fn console_errors(&self) -> Result<Vec<String>, E2eError>;         // Chrome browser log (CSP violations)
    pub async fn close(self) -> Result<(), E2eError>;
}

pub struct FakeGoogle { /* reqwest client to /__fake/... */ }
impl FakeGoogle {
    pub async fn seed_account(&self, sub: &str, email: &str, verified: bool) -> Result<(), E2eError>;
    pub async fn select_account_for_next_authorize(&self, sub: &str) -> Result<(), E2eError>;
    pub async fn seed_messages(&self, sub: &str, fixture_ids: &[&str]) -> Result<(), E2eError>;
}

pub struct TestControl { /* reqwest client to the api's test-only routes */ }
impl TestControl {
    pub async fn create_invite(&self, email: &str) -> Result<String, E2eError>; // raw token
    pub async fn advance_clock(&self, by: Duration) -> Result<(), E2eError>;
}
```

## Algorithm

1. **`scripts/e2e.sh`** (`set -euo pipefail`, `trap cleanup EXIT` that kills every child):
   1. `cargo build --locked -p api -p unsub --features api/testkit,unsub/testkit` and `cargo build --locked -p fake-google -p unsub-testbed`.
   2. `flutter build web --release --dart-define=MT_E2E=true` in `app/`.
   3. Start the Firestore emulator on `127.0.0.1:0`-style free ports chosen by the script (free port picked by a small stdlib helper `scripts/free_port.py`, then `gcloud emulators firestore start --host-port=127.0.0.1:$PORT`); reuse the install step T-301 added to the `rust` job.
   4. Start fake-google and unsub-testbed with `--port-file <dir>/<name>.port`; wait until each file exists (max 30 s). If T-205 or T-708 lack `--port-file`, add it there (a few lines in their `main.rs`).
   5. Start `unsub`, then `api`, with `scripts/e2e/*.env` plus the discovered ports; stdout of each goes to `target/e2e-logs/<service>.jsonl` (T-1101c scans these).
   6. Start `scripts/e2e_host.py --root app/build/web --firebase-json firebase.json --api http://127.0.0.1:$API_PORT --port-file ...`. Use host `localhost` in URLs so Chrome accepts `Secure` cookies over http.
   7. Start `chromedriver --port=$CD_PORT` (`$CHROMEWEBDRIVER/chromedriver` on GitHub runners).
   8. Export the `MT_E2E_*` variables and run `cargo test --locked -p e2e -- --ignored --test-threads=1` (or one test file with `--journey`).
2. **`e2e_host.py`** (stdlib only): serve files from the build folder; for every response add the headers from `firebase.json` `hosting.headers` whose `source` glob matches; proxy `/api/**` to the api preserving method, body, `Cookie`, `Origin`, `X-CSRF-Token`, `Idempotency-Key` and all response headers including `Set-Cookie` and `Clear-Site-Data`; set `X-Forwarded-For: 127.0.0.1`. Unknown paths serve `index.html` (single-page app).
3. **Test-only invite route**: `POST /internal/test/invites {email}` creates an invite through the normal invite service (hash stored, raw token returned); compiled only with `#[cfg(feature = "testkit")]`, exactly like `advance-clock`.
4. **Release check**: `release_routes.rs` builds the router with no features and asserts `404` for `POST /internal/test/advance-clock` and `POST /internal/test/invites`. The workspace `rust` job uses `--all-features`, so add a step to the `e2e` job (or the `rust` job) running `cargo test --locked -p api --test release_routes` without features.
5. **Journey 1** (`sign_in.rs`, `#[ignore = "run by scripts/e2e.sh"]`): seed a fake Google account `invitee@example.com` (verified) with three corpus messages; `create_invite`; open `/#/invite?t=<token>`; wait for the S9 invited copy; tap "Continue with Google"; fake-google auto-approves the selected account and redirects back; wait for the Feed (sender name of the newest seeded message visible).
6. **CI job `e2e`** in `ci.yml`: `runs-on: ubuntu-latest`, `timeout-minutes: 20`, steps: checkout, Rust toolchain and cache (as `rust`), Flutter (as `dart`), Java and the Firestore emulator (as T-301's step), `scripts/e2e.sh`, then on failure upload `target/e2e-logs/` as an artifact (synthetic data only). Pin every action by full commit SHA like the existing jobs.
7. Add `"e2e"` to `contexts` in `tools/apply_branch_protection.sh` and tell James in the PR to re-run it (the required check takes effect only then).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-03 AC1 | In a real browser, an invited user opens the invite link, signs in with Google and lands on the Feed |
| ASVS V13.4.2 | Release builds have no test routes (`advance-clock`, `invites`) |

## Tests that must pass

- `au_03_ac1_e2e_invited_user_lands_on_feed` (end to end, `e2e` crate)
- `asvs_v13_4_2_test_routes_absent_without_testkit_feature` (service integration, `api`, run without features)
- `scripts/e2e.sh` exits 0 locally and in the `e2e` job, in under 10 minutes on a GitHub-hosted runner (S10 3.3 target)

## Edge cases and traps

- Full-page OAuth redirects end any Dart test running inside the page, so journeys are driven from outside by WebDriver, not `integration_test` (deviation from S10 3.2, reported).
- Flutter web draws to a canvas: find controls by `aria-label` on `flt-semantics` nodes; that only works because every control has a Semantics label (XC-03) and the e2e build forces semantics on.
- Never point anything at a real Google host; fake-google URLs only, and the app accepts loopback http only in the `MT_E2E` build.
- The test routes must not compile into a release binary; the feature flag is the control, and the release test proves it.
- Kill every child process on exit, even on failure, or the CI runner hangs.
- Bind every service to `127.0.0.1` and a free port; never fixed ports that clash with parallel runs.
- No real secrets in `scripts/e2e/*.env`; generated test keys only (Gitleaks runs on them).
- Do not add container or infrastructure scanning (CLAUDE.md).

## Out of scope

- Journeys 2 to 6 (T-1101b); journeys 7 to 9, leak scans, storage and CSP checks (T-1101c).
- Bake-off e2e (JEV-1, EXP-1 against fake-vertex and fake-jev): follow-up after T-906.

## Done when

- The tests above pass and every required check is green (S10 10.1), including the new `e2e` job.
- Definition of done in S10 10.4.
- `tools/apply_branch_protection.sh` lists `e2e`.
