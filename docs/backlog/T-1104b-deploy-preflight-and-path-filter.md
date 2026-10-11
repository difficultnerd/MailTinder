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

1. **Path filter.** Under `on.push` add `paths:` covering what can change a deployed artifact: `backend/**`, `app/**`, `firebase.json`, `.firebaserc`, `.github/workflows/deploy.yml`, `infra/terraform/modules/runtime/**` (service definitions), and leave `workflow_dispatch` unfiltered so the owner can always run it. Docs, backlog, specs, tools and Terraform for other modules do not trigger a deploy.
2. **Preflight.** Add a job `configured` (no environment, no credentials, `permissions: contents: read`) whose single step writes `ready=true|false` to its output: true only if `vars.STAGING_PROJECT`, `vars.STAGING_WIF_PROVIDER` and `vars.STAGING_DEPLOYER` are all non-empty. When false it prints one notice line: "deploy skipped: staging variables are not set yet (see infra/terraform/README.md, Deploying)". Never echo the variable VALUES. Make `build` `needs: configured` and run only `if: needs.configured.outputs.ready == 'true'` (keep the existing repository guard). `deploy-staging` and `deploy-prod` already depend on `build`, so they are skipped too.
3. **No new permissions, no secrets, no new triggers.** Actions stay pinned by 40-character SHA (repository rule). Do not add `pull_request` triggers.
4. Fix the README sentence at line 237 so it describes only what exists (the production Environment reviewer is the only gate until T-1105).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| DEP-1 | With the staging variables unset, a push that matches the path filter ends with the deploy run SKIPPED (not failed) and one notice line |
| DEP-2 | A docs-only push does not start the deploy workflow at all |
| DEP-3 | `workflow_dispatch` still runs it; with variables set, behaviour is unchanged from T-1104 |

## Tests that must pass

- The workflow policy test asserts: a `configured` job exists, `build` has `needs: configured` and the `ready` condition, `on.push.paths` is present and does not list `docs/**`, no `pull_request` trigger, every `uses:` is pinned to 40 hex characters.
- `actionlint` clean (if available in CI); all required checks green (S10 10.1).
- DEP-1 and DEP-2 are verified on the first pushes after merge (recorded in the PR description / checkpoint); they cannot be unit-tested offline.

## Edge cases and traps

- A skipped required status can read as a pass or a fail depending on branch protection: this workflow is not a required check; keep it that way.
- Do not weaken the WIF conditions or the production reviewer gate. This task only decides WHETHER the pipeline starts.
- Do not touch `ci.yml` or other workflows.

## Out of scope

- A smoke job (T-1105), a CI docker build step (needs `ci.yml`, owner), pinning base images by digest, building the web app once and promoting it.

## Done when

- Tests above pass and every required check is green; Definition of done in S10 10.4. The owner merges the workflow change.
