# T-1105: Staging smoke tests and the ZAP baseline

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 350 lines of code plus tests | T-301, T-302, T-304, T-1104 |

**Read only these spec sections:** S10 sections 3.1 (row "Staging smoke"), 6.2 (last paragraph), 7.4 and 10.3 (`docs/specs/S10-test-strategy.md`); S2 UN-02 AC4; the "Backlog seeds for S12" line on `optional/zap` in `docs/planning-roadmap.md`; `optional/zap/README.md`. Nothing else is needed.

## Goal

After each staging deploy, a small Rust smoke binary checks the real platform: KMS wraps and unwraps, Firestore reads and writes, Cloud Tasks schedules and cancels and delivers to `unsub`, `unsub` refuses the metadata server as a one-click target, the internal services are not reachable from the internet, and the hosted app and API carry the right headers. The template's ZAP layer is installed and runs a nightly passive baseline against staging (decisions T4, T5).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/smoke/Cargo.toml`, `src/main.rs`, `src/checks.rs` | Smoke binary (not deployed as a service) |
| Change | `backend/Cargo.toml` | Add `crates/smoke` to the workspace |
| Create | `infra/terraform/envs/staging/smoke.tf` | Staging-only `mt-smoke` account and its roles |
| Create | `.github/workflows/smoke.yml` | Reusable (`workflow_call`) and manual; runs the binary with WIF as `mt-smoke` |
| Install | `optional/install.sh zap` | Copies `.github/workflows/zap.yml` and `.zap/rules.tsv` |
| Change | `.github/workflows/zap.yml` | Nightly cron; baseline only |

## Types and signatures

```rust
// backend/crates/smoke/src/checks.rs
pub struct SmokeConfig { pub project: String, pub region: String, pub kms_key: String, pub queue: String,
                         pub unsub_url: Url, pub worker_url: Url, pub app_url: Url, pub tasks_invoker: String }
impl SmokeConfig { pub fn from_env() -> Result<Self, SmokeError>; } // MT_SMOKE_* variables

pub async fn kms_round_trip(p: &Ports) -> Result<(), SmokeError>;
pub async fn firestore_round_trip(p: &Ports) -> Result<(), SmokeError>;
pub async fn tasks_schedule_and_cancel(p: &Ports, cfg: &SmokeConfig) -> Result<(), SmokeError>;
pub async fn un_02_ac4_staging_unsub_refuses_metadata_target(p: &Ports, cfg: &SmokeConfig) -> Result<(), SmokeError>;
pub async fn internal_services_not_public(cfg: &SmokeConfig) -> Result<(), SmokeError>;
pub async fn hosting_and_api_headers(cfg: &SmokeConfig) -> Result<(), SmokeError>;
// main.rs runs every check, prints one line per check (name and pass or fail, never data), exits 1 on any failure.
```

## Algorithm

1. **Identity:** `smoke.tf` (staging root only) creates `mt-smoke` with: `roles/datastore.user` (project), `roles/cloudkms.cryptoKeyEncrypterDecrypter` on the staging key, `roles/cloudtasks.enqueuer` and `roles/cloudtasks.taskDeleter` on the queue, `roles/iam.serviceAccountUser` on `mt-tasks-invoker`, and `workloadIdentityUser` for the repository. `[DEFAULT]` This makes four KMS holders in staging only, which holds synthetic data; production keeps exactly three, and `scripts/tf_env_parity.sh` ignores `smoke.tf` by name. The PR asks for a strong-model review of `smoke.tf`.
2. Build `Ports` with the real `adapters-gcp` adapters (T-301, T-302, T-304) and the system clock and RNG.
3. `kms_round_trip`: `new_user_key` for a random smoke user, `seal` and `open` a 32-byte random value with `Aad { scope: "smoke", field: "smoke" }`; compare.
4. `firestore_round_trip`: write a `users/{smoke uuid}` document with the wrapped key through `ServerStore`, read it back, delete it at the end of the run.
5. `tasks_schedule_and_cancel`: schedule a task 10 minutes ahead for a random `JobId`, cancel it, expect `Cancelled`.
6. `un_02_ac4_...`: create a smoke user and two `one_click` jobs in `queued` state whose encrypted targets are `https://metadata.google.internal/computeMetadata/v1/` and `https://169.254.169.254/`, schedule both due now; poll the job documents every 5 s for up to 120 s; each must end `needs_attention` and a `needs_attention` item with reason `one_click_address_refused` must exist. Delete the user, jobs and items afterwards (in a `finally`-style cleanup that runs on failure too).
7. `internal_services_not_public`: unauthenticated `GET` on the `unsub` and `worker` URLs from the runner must return 403 or 404, never 200.
8. `hosting_and_api_headers`: `GET app_url/` has `Content-Security-Policy` (with `require-trusted-types-for`), `Strict-Transport-Security`, `X-Content-Type-Options: nosniff`, `Referrer-Policy`; `GET app_url/api/v1/session` returns 200 with `Cache-Control: no-store` and a `Set-Cookie` for `__session` with `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/` and no `Domain`.
9. **`smoke.yml`:** `workflow_call` plus `workflow_dispatch`; auth with WIF as `mt-smoke`; `cargo run --release --locked -p smoke`. T-1104's deploy calls it before production.
10. **ZAP** (T4, T5): run `optional/install.sh zap`; change the cron in `.github/workflows/zap.yml` to nightly (`"41 16 * * *"`, early morning in eastern Australia `[DEFAULT]`); leave `ZAP_OPENAPI_URL` unset (active API scan needs an authenticated context, S10 7.4). Step for James: set repository variable `ZAP_TARGET_URL` to the staging Hosting URL. The workflow is not a required check. Triage the first report into `.zap/rules.tsv` with a comment per ignored rule.

**Waits on S11** (defaults used): who reads the nightly ZAP report and the remediation window for findings (V15.1.1); whether smoke failures page anyone (`[DEFAULT]` the deploy stops and GitHub notifies the committer).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-02 AC4 | The deployed `unsub` refuses metadata targets and raises Needs Attention, in staging |

The smoke binary's functions are not `#[test]`s; `tools/check_ac_coverage.py` finds the ID in the function name under `backend/`. The ZAP baseline is the `dast` verification for V4.1.2, V12.1.1 and V13.4.3 (register), recorded in the PR.

## Tests that must pass

- `un_02_ac4_staging_unsub_refuses_metadata_target` (staging smoke)
- `kms_round_trip`, `firestore_round_trip`, `tasks_schedule_and_cancel`, `internal_services_not_public`, `hosting_and_api_headers` (staging smoke)
- Unit tests in `crates/smoke` for `SmokeConfig::from_env` (missing variable is an error) and for the header checks against canned responses (these run in normal CI)
- One manual `zap.yml` run against staging with its report attached to the PR

## Edge cases and traps

- Smoke never touches production: its identity exists only in the staging root.
- Clean up every document the smoke run creates, even when a check fails.
- Print only check names and pass or fail; never document contents, tokens or URLs with secrets.
- Do not follow redirects in the header checks.
- The ZAP layer is DAST, allowed in the core (T4); still do not add container or infrastructure scanning.
- Smoke runs only after deploy; it is not part of pull request CI (no cloud credentials there).

## Out of scope

- An authenticated ZAP context and API scan (later, when S10 7.4's auth context exists).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has set `ZAP_TARGET_URL`; the first nightly ZAP baseline has run and its findings are triaged.
