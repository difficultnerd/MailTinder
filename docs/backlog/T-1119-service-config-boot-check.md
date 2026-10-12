# T-1119: Service config boot check: every variable a service's config loader requires must be set by Terraform

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: one production-env loader per service plus one test | T-1102b |

**Read only these spec sections:** `backend/crates/api/src/config.rs` (lines 86 to 140 and `required`), `backend/crates/unsub/src/config.rs` (lines 20 to 60), `backend/crates/worker/src/main.rs` (lines 60 to 100), `infra/terraform/modules/runtime/cloud_run.tf` (the `locals` block, lines 38 to 80). `backend/crates/api/src/startup.rs` (`build_production_ports`, lines 186 to 230), `backend/crates/unsub/src/main.rs` (`production_run`, lines 135 to 145), `backend/crates/worker/src/main.rs` (`run`, lines 70 to 110). Nothing else is needed.

**Do not start until PR #93 (T-1104c) is merged.** The new test would fail on `main` today by design; #93 is the fix. The owner adds this task to the builder allow-list after #93 merges.

## Why this task exists (verified 11 Oct 2026)

The first automatic staging deploy (run 38174582196, 21:03 UTC) built and pushed all three images, then `mt-api` failed with "container failed to start and listen on PORT=8080". Cause, read from the code: `ApiConfig::from_lookup` calls `required()` for `APP_ORIGIN` (`config.rs:102`) and `GOOGLE_OAUTH_CLIENT_ID` (`config.rs:105`) before the server binds, and `cloud_run.tf` set the client id only in `unsub_env` and never set the origin. The first diagnosis (11 Oct) was INCOMPLETE: the API also needs `UNSUB_BASE_URL`, `UNSUB_AUDIENCE` and `UNSUB_TASKS_CALLER` (`startup.rs:189-193`), which `ApiConfig::from_lookup` does not read; PR #93 sets all five. The lesson is that the whole startup path must be checked, not only the config struct. Unit tests of each loader passed, Terraform tests passed, review passed: nothing connected the two sides. The same class had already hit unsub once (M1). unsub and worker were checked on 11 Oct and are consistent today (`UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`, `WORKER_AUDIENCE`, `WORKER_SCHEDULER_CALLER` are all set in `cloud_run.tf`); only api was broken.

## Goal

A CI test that fails whenever a service's COMPLETE production environment (every variable its startup path reads and requires) is not provided, correctly bound and non-blank, by the environment Terraform gives that service.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/tests/service_boot_env.rs` | the test (see Algorithm) |
| Change | `backend/crates/api/src/startup.rs` | extract `ProductionEnv::from_lookup(lookup)` that reads and validates EVERY required variable of `build_production_ports` (config struct, `GOOGLE_CLOUD_PROJECT` with `validate_project_id`, `UNSUB_BASE_URL` parsed, `UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`); `build_production_ports` calls it, so the test and startup cannot diverge |
| Change | `backend/crates/unsub/src/main.rs` | the same for `production_run` (config struct, project, base URL, client id) |
| Change | `backend/crates/worker/src/main.rs` | the same for `run` (audience, scheduler caller, project, port); make the types reachable from tests (a small `lib.rs` if needed); no behaviour change |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1119` |

## Algorithm

1. In the test, read `infra/terraform/modules/runtime/cloud_run.tf` as text (path relative to `CARGO_MANIFEST_DIR`, as `deploy_workflow_policy.rs` reads the workflow).
2. Parse the `locals` block into three maps, one per service, with a small line parser: `base_env = {` entries; `api_env = merge(local.base_env, {` extra entries; the same for `unsub_env` and `worker_env`. An entry is `KEY = <expression>`. Also parse each service RESOURCE (`resource "google_cloud_run_v2_service" "api"` and the others) and assert that its `dynamic "env"` `for_each` is the matching map (`local.api_env`, `local.unsub_env`, `local.worker_env`) and that each service map inherits `local.base_env` via `merge`, and that the resource's `content` block emits `name = env.key` and `value = env.value` (so the map really becomes the container environment). The parser must FAIL (not skip) on any construct it does not understand, and when it finds none of the maps or bindings, so a rename or restructure cannot silently disable the test.
3. For each service build a lookup closure over that service's merged entries. A literal value is passed through AS WRITTEN (so a blank or malformed literal fails the loader). A `var.*` or `local.*` reference is replaced by a placeholder from a small table (`APP_ORIGIN` -> `https://mailtinder-prod.web.app`, `GOOGLE_OAUTH_CLIENT_ID` -> `x.apps.googleusercontent.com`, `*_URL` / `*_AUDIENCE` -> an https URL, `*_CALLER` / `*_SA` -> an email-shaped string, project -> a valid project id). `PORT` is NOT provided (Cloud Run injects it). Everything else is `None`. State plainly in the test's doc comment that for references the guarantee is name coverage, binding and literal validity ONLY. What a `var.*` / `local.*` reference resolves to is proven just where `runtime.tftest.hcl` asserts it today (`api_carries_app_origin_and_client_id` covers APP_ORIGIN, the client id, UNSUB_BASE_URL, UNSUB_AUDIENCE and a non-blank UNSUB_TASKS_CALLER); it does NOT assert the caller e-mail values of `UNSUB_TASKS_CALLER` / `WORKER_SCHEDULER_CALLER` against the one allowed invoker (S7 line 536). Do not claim more. A follow-up owner-merged Terraform test task (T-1119b) may add those assertions; this task must not edit any `.tf` or `.tftest.hcl` file.
4. Call the real production loaders: `api::startup::ProductionEnv::from_lookup(lookup, Mode::Production)`, the unsub and worker equivalents. Assert `Ok`. On `Err(Missing(name))` the failure message must name the variable and the service ("api requires APP_ORIGIN but cloud_run.tf api_env does not set it").
5. Supplemental guard (NOT a verification of ASVS V13.3.1; Secret Manager design review and Gitleaks remain the controls): no entry whose name contains `SECRET`, `TOKEN`, `PASSWORD` or `KEY` has a DIRECT literal value; values must be `var.*` or `local.*` references. Add a mutation that puts a literal in a `local.*` the entry points at only if the parser can resolve it; otherwise say in the doc comment that indirect literals are out of scope.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | With `cloud_run.tf` as merged after T-1104c, the test passes for all three services |
| B2 | Deleting `APP_ORIGIN` from `api_env` makes the test fail naming `APP_ORIGIN` and `api` |
| B3 | Deleting `UNSUB_AUDIENCE` from `unsub_env` or `WORKER_AUDIENCE` from `worker_env` fails likewise |
| B4 | A literal secret-looking value in any env map fails the test |
| B5 | If the parser finds no maps or bindings, or meets an unsupported construct, the test fails (it never passes vacuously) |
| B6 | Deleting `UNSUB_BASE_URL` (or `UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER`, `GOOGLE_CLOUD_PROJECT`) from `api_env`/`base_env` fails the test naming the variable |
| B7 | Rebinding the worker resource's `for_each` to `local.unsub_env`, or setting a required literal to blank, fails the test |
| B8 | Changing `value = env.value` to `value = env.key` (or `name = env.key` to `name = env.value`) in any service's `content` block fails the test |

## Tests that must pass

The new test file (B1 to B8; show B2, B3, B5, B6, B7 and B8 as mutation transcripts in the PR description), plus all existing tests and required checks. No Terraform command is run.

## Edge cases and traps

- Do NOT hard-code the list of required variables in the test: the production-env loaders (which startup itself calls) are the source of truth, so a new required variable is caught automatically. Any `env("...")` added to a startup path must go through them.
- The placeholder values only need to satisfy each loader's validation (`validate_origin` rejects a trailing slash, a path, credentials; `Mode::Production` may require https). Do not weaken any validator.
- Keep the parser dumb and readable; if the Terraform layout needs HCL features it cannot parse, fail loudly and say what to change.
- Do not touch `.github/` or any `.tf` file (a Terraform change is owner-only).

## Out of scope

Booting the real container, checking secret versions or KMS permissions (those need the cloud; the first deploy shows them), the monitoring module.

## Done when

The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
