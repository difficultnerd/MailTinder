# T-1119: Service config boot check: every variable a service's config loader requires must be set by Terraform

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: one Rust test, one small refactor in the worker crate | T-1102b |

**Read only these spec sections:** `backend/crates/api/src/config.rs` (lines 86 to 140 and `required`), `backend/crates/unsub/src/config.rs` (lines 20 to 60), `backend/crates/worker/src/main.rs` (lines 60 to 100), `infra/terraform/modules/runtime/cloud_run.tf` (the `locals` block, lines 38 to 80). Nothing else is needed.

**Do not start until PR #93 (T-1104c) is merged.** The new test would fail on `main` today by design; #93 is the fix. The owner adds this task to the builder allow-list after #93 merges.

## Why this task exists (verified 11 Oct 2026)

The first automatic staging deploy (run 38174582196, 21:03 UTC) built and pushed all three images, then `mt-api` failed with "container failed to start and listen on PORT=8080". Cause, read from the code: `ApiConfig::from_lookup` calls `required()` for `APP_ORIGIN` (`config.rs:102`) and `GOOGLE_OAUTH_CLIENT_ID` (`config.rs:105`) before the server binds, and `cloud_run.tf` set the client id only in `unsub_env` and never set the origin. Unit tests of each loader passed, Terraform tests passed, review passed: nothing connected the two sides. The same class had already hit unsub once (M1). unsub and worker were checked on 11 Oct and are consistent today (`UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`, `WORKER_AUDIENCE`, `WORKER_SCHEDULER_CALLER` are all set in `cloud_run.tf`); only api was broken.

## Goal

A CI test that fails whenever a service's real config loader would refuse to start with the environment Terraform gives that service.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/tests/service_boot_env.rs` | the test (see Algorithm) |
| Change | `backend/crates/worker/src/main.rs` | split `Config::from_env` into `Config::from_lookup(impl Fn(&str) -> Option<String>)` plus a thin `from_env`, and make `Config` reachable from the test (a `pub mod config` in a new `backend/crates/worker/src/lib.rs` if needed); no behaviour change |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1119` |

## Algorithm

1. In the test, read `infra/terraform/modules/runtime/cloud_run.tf` as text (path relative to `CARGO_MANIFEST_DIR`, as `deploy_workflow_policy.rs` reads the workflow).
2. Parse the `locals` block into three key sets, one per service, with a small line parser: `base_env = {` map keys; `api_env = merge(local.base_env, {` extra keys; the same for `unsub_env` and `worker_env`. A key is an identifier at the start of a line before `=` inside the map. The parser must fail the test (not skip) if it finds none of the three maps, so a rename cannot silently disable it.
3. For each service build a lookup closure: every key in that service's set maps to a placeholder value taken from a small table (`APP_ORIGIN` -> `https://mailtinder-prod.web.app`, `GOOGLE_OAUTH_CLIENT_ID` -> `x.apps.googleusercontent.com`, `*_URL` / `*_AUDIENCE` -> an https URL, `*_CALLER` / `*_SA` -> an email-shaped string, anything else -> `placeholder`), `PORT` is NOT provided (Cloud Run injects it), everything else is `None`.
4. Call the real loader: `ApiConfig::from_lookup(lookup, Mode::Production)`, `UnsubConfig::from_lookup(lookup)`, `worker::Config::from_lookup(lookup)`. Assert `Ok`. On `Err(Missing(name))` the failure message must name the variable and the service ("api requires APP_ORIGIN but cloud_run.tf api_env does not set it").
5. Also assert the reverse for secrets: no key in any set whose name contains `SECRET`, `TOKEN`, `PASSWORD` or `KEY` has a literal value (values must be `var.*` or `local.*` references): the existing secrets rule (ASVS V13.3.1) restated as a test.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | With `cloud_run.tf` as merged after T-1104c, the test passes for all three services |
| B2 | Deleting `APP_ORIGIN` from `api_env` makes the test fail naming `APP_ORIGIN` and `api` |
| B3 | Deleting `UNSUB_AUDIENCE` from `unsub_env` or `WORKER_AUDIENCE` from `worker_env` fails likewise |
| B4 | A literal secret-looking value in any env map fails the test |
| B5 | If the parser finds no maps, the test fails (it never passes vacuously) |

## Tests that must pass

The new test file (B1 to B5; show B2, B3 and B5 as mutation transcripts in the PR description), plus all existing tests and required checks. No Terraform command is run.

## Edge cases and traps

- Do NOT hard-code the list of required variables in the test: the real loaders are the source of truth, so a new `required()` call is caught automatically.
- The placeholder values only need to satisfy each loader's validation (`validate_origin` rejects a trailing slash, a path, credentials; `Mode::Production` may require https). Do not weaken any validator.
- Keep the parser dumb and readable; if the Terraform layout needs HCL features it cannot parse, fail loudly and say what to change.
- Do not touch `.github/` or any `.tf` file (a Terraform change is owner-only).

## Out of scope

Booting the real container, checking secret versions or KMS permissions (those need the cloud; the first deploy shows them), the monitoring module.

## Done when

The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
