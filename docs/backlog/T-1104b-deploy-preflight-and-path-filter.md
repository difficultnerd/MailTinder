# T-1104b: Deploy workflow: skip cleanly when unconfigured, and only deploy when something deployable changed

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 40 lines of workflow plus one README sentence | T-1104 |

**Read only these spec sections:** `docs/backlog/T-1104-build-and-deploy-pipeline.md` (Goal, Algorithm); `.github/workflows/deploy.yml` as merged. Nothing else is needed.

**Why this task exists (verified 11 Oct 2026 from the merged workflow and its first run).** PR #81 merged at 05:53 UTC. The `deploy` run it triggered (run 38116267024) failed at once, as predicted: `build (api|unsub|worker)` stop at the auth step with "must specify exactly one of workload_identity_provider or credentials_json" because the repository variables `STAGING_WIF_PROVIDER` and `STAGING_DEPLOYER` do not exist yet (they are set after the first staging apply). That is the safe, fail-closed outcome, but it has three costs:
- Every push to `main` will show a RED `deploy` run until the owner configures staging, which drowns out real failures (24 of 500 runs in the last two days were genuine failures).
- The message is unhelpful ("must specify ... credentials_json") and sends the reader to the wrong fix.
- Once configured, the workflow triggers on EVERY push to `main` (`deploy.yml:19-22`), including docs-only merges: each would build three images, deploy staging, and ask the owner to approve a production deployment (`deploy.yml:179-187`, environment `production`), even though nothing deployable changed.

## Goal

A push to `main` runs the deploy pipeline only when deployable files changed, and when the pipeline has not been configured it ends GREY (skipped) with a one-line explanation instead of RED.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `.github/workflows/deploy.yml` | `on.push.paths` filter; a first job `configured` and a guard on `build`; plain-language notice |
| Change | `infra/terraform/README.md` | one sentence in "Deploying" about the skip and the filter; correct the line that claims smoke failures stop the deploy (line 237) until T-1105 exists |
| Change | `backend/crates/api/tests/dockerfile_policy.rs` (or the existing workflow policy test) | assert the new guard and the paths filter exist (text policy, like the existing checks) |

(Workflow files are owner-merged; the builder prepares the PR.)

## Algorithm

1. **Path filter.** Under `on.push` add `paths:` covering what can change a deployed artifact: `backend/**`, `app/**`, `firebase.json`, `.firebaserc`, `.github/workflows/deploy.yml` (the only things that can change a built image or the hosted web app: `deploy.yml` deploys with `--image` only and never applies Terraform, so nothing under `infra/` triggers it), and leave `workflow_dispatch` unfiltered so the owner can always run it. Docs, backlog, specs, tools and Terraform for other modules do not trigger a deploy.
2. **Preflight (fails open ONLY before the first setup; loud after it).** Add a job `configured` (no environment, no credentials, `permissions: contents: read`) with two outputs. It reads REPOSITORY variables (the README says they are repository variables; a job without an environment cannot see environment-scoped variables, so the README must say so in bold). `staging_ready` is true only if `vars.STAGING_PROJECT`, `vars.STAGING_WIF_PROVIDER` and `vars.STAGING_DEPLOYER` are all non-empty; `prod_ready` is true only if `vars.PROD_PROJECT`, `vars.PROD_WIF_PROVIDER` and `vars.PROD_DEPLOYER` are all non-empty. A third repository variable `DEPLOY_ENABLED`, set to `true` by the owner after the first successful staging deploy, turns silence into noise: when `DEPLOY_ENABLED == 'true'` and `staging_ready` is false the job FAILS (red) with a message naming which variables are empty (names only, never values); when `DEPLOY_ENABLED` is not set and `staging_ready` is false it ends GREEN-SKIPPED with a `::warning::` annotation (visible on the run page) and one summary line: "deploy skipped: staging variables are not set yet (infra/terraform/README.md, Deploying)". A manual `workflow_dispatch` with `staging_ready` false ALWAYS fails (the owner asked for a deploy). Never echo variable VALUES. Make `build` `needs: configured` and run only `if: needs.configured.outputs.staging_ready == 'true'` (keep the repository guard). `deploy-staging` depends on `build`; `deploy-prod` additionally requires `needs.configured.outputs.prod_ready == 'true'`, so no production approval is requested while production is not configured.
3. **No new permissions, no secrets, no new triggers.** Actions stay pinned by 40-character SHA (repository rule). Do not add `pull_request` triggers.
4. Fix the README sentence at line 237 so it describes only what exists (the production Environment reviewer is the only gate until T-1105).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| DEP-1 | With the staging variables unset, a push that matches the path filter ends with the deploy run SKIPPED (not failed) and one notice line |
| DEP-2 | A docs-only push does not start the deploy workflow at all |
| DEP-3 | `workflow_dispatch` is never filtered; with all staging variables set the pipeline behaves as in T-1104; with `DEPLOY_ENABLED=true` and a staging variable missing the run FAILS loudly naming the variable |
| DEP-4 | `deploy-prod` is skipped (no approval request) while any `PROD_*` variable is empty |

## Tests that must pass

- **New checker `tools/check_deploy_workflow.py` with unit tests (this does not exist today: `backend/crates/api/tests/dockerfile_policy.rs` reads only `backend/Dockerfile`, and no test or CI step enforces 40-hex action pinning, so this task creates both).** It parses `deploy.yml` (PyYAML or a strict line parser) and the unit tests run it over good and BAD fixture workflows, each bad fixture removing one behaviour so the test fails when the behaviour is removed: (DEP-1) `configured` job missing; `build` without `needs: configured` or without the `staging_ready` condition; `deploy-prod` without the `prod_ready` condition; the `DEPLOY_ENABLED` failure branch missing; (DEP-2) `on.push.paths` missing or containing `docs/**` or `infra/**`; (DEP-3) `workflow_dispatch` missing or carrying a `paths` filter, and the conditions evaluating true when all variables are set (evaluate the `if:` expressions over a table of variable sets); also: any `pull_request`/`pull_request_target` trigger, any `uses:` not pinned to 40 hex characters, any `environment:` on the `configured` job. The checker is run by an existing CI step (Python test discovery under `tools/`, as the other `tools/check_*.py` checkers are); if that needs a `ci.yml` change, list it for the owner instead of making it.
- `actionlint` clean (if available in CI); all required checks green (S10 10.1).
- DEP-1's runtime behaviour (grey skipped run, annotation) is additionally observed on the first push after merge (recorded in the PR description); it is not the only proof.

## Edge cases and traps

- A skipped required status can read as a pass or a fail depending on branch protection: this workflow is not a required check; keep it that way.
- Do not weaken the WIF conditions or the production reviewer gate. This task only decides WHETHER the pipeline starts.
- Accepted, unchanged by this task (review F7): the build step's image path contains the staging project id and repository variables are not masked in public Actions logs. The id is already public in the repository's docs; nothing secret is involved.
- Do not touch `ci.yml` or other workflows.

## Out of scope

- A smoke job (T-1105), a CI docker build step (needs `ci.yml`, owner), pinning base images by digest, building the web app once and promoting it.

## Done when

- Tests above pass and every required check is green; Definition of done in S10 10.4. The owner merges the workflow change.
