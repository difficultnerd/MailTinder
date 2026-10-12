# T-1104d: Harden the deploy workflow policy test (review findings S-1 to S-7 of PR #89)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 150 lines of Rust in one test file | T-1104b |

**Read only these spec sections:** `backend/crates/api/tests/deploy_workflow_policy.rs` (whole file, 800 lines; the checkers are `check_triggers` line 307, `check_jobs` 380, `check_configured` 476, `check_pins` 534, `check_services` 558, the mutation helpers `workflow_mutation` 603 and `firebase_mutation` 618), `.github/workflows/deploy.yml` as merged (read only). Nothing else is needed.

## Why this task exists

PR #89 (T-1104b) merged on 11 Oct 2026 with an "approve with findings" review: no Critical or High, one Medium and six Low/Info test-strictness gaps that two rework rounds did not close. They are real but small, and they are in a TEST FILE, so this task needs no workflow edit and the factory can merge it (no owner-only path is touched).

## Goal

The policy test rejects each of the seven gaps below, each proven by a new mutation test in the existing style (`workflow_mutation(from, to)` expects the checker to report a violation).

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/tests/deploy_workflow_policy.rs` | the checks and mutation tests below |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1104d` |

Do NOT edit `.github/`, `infra/` or `firebase.json`. If a fix needs a workflow change, stop and report it.

## The seven gaps (from the PR #89 review; quote-checked against the merged file)

| ID | Sev | Gap | Required check |
| --- | --- | --- | --- |
| S-1 | Medium | The `configured` job's variable loop is not covered by any mutation: removing a variable from `for name in STAGING_PROJECT STAGING_WIF_PROVIDER STAGING_DEPLOYER` (or the PROD_ loop) passes | `check_configured` must require all three STAGING_ and all three PROD_ names in the loops and the env block; add one mutation test per loop (drop one name) |
| S-2 | Low | No check that the workflow keeps `permissions: contents: read` (top level and on `configured`) | assert both; mutation: change to `write` or delete |
| S-3 | Low | `check_triggers` accepts an `on.push.paths` catch-all such as `**` | reject `**`, `*` and an empty list; mutation: add `- '**'` |
| S-4 | Low | `check_services` does not constrain the Cloud Run region | assert (a) the `REGION` env equals the `region` default declared by the runtime module (`infra/terraform/modules/runtime/variables.tf`), (b) every `gcloud run deploy` command passes `--region "${REGION}"` (not a literal), (c) the Hosting rewrite region in `firebase.json` equals `REGION`; one mutation for each of the three (Sol review of #94, R-9) |
| S-5 | Low | The PR description did not list behaviours B1 to B5 or mutation transcripts | process item: this task's PR description MUST list its own mutations and their failing output |
| S-6 | Info | `DEPLOY_ENABLED` has no verification | assert the `configured` job reads `vars.DEPLOY_ENABLED` and that both the workflow comment block and the README name it (read-only check of the README text; do not edit the README) |
| S-8 | Low | (PR #91 review N3) The policy accepts deleting the `staging_ready=false` or `prod_ready=false` assignments in the `configured` job | require both assignments in the Bash block and mutate each away; also execute the extracted Bash block with the variables unset and assert `staging_ready=false`/`prod_ready=false` |
| S-7 | Info | The action-pin check (`check_pins`) is fed only `deploy.yml` | run it over every file in `.github/workflows/` (read-only) and fail on an unpinned `uses:`; if an existing workflow is not pinned, list it in the PR and exclude it from the check with a comment naming the file (the owner fixes workflows) |

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | `deploy_policy_accepts_the_real_files` still passes on the merged files |
| B2 | Each of S-1 to S-4, S-6 and S-7 has a mutation test that fails the checker, and the transcript is in the PR description |
| B3 | No workflow, Terraform or `firebase.json` file is modified |

## Tests that must pass

The existing tests plus the new mutation tests; all required checks green (S10 10.1).

## Out of scope

Changing `deploy.yml`, adding a smoke job (T-1105), the Terraform README text.

## Done when

Tests green, definition of done in S10 10.4.
