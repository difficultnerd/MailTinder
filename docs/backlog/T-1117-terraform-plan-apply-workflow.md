# T-1117: Terraform plan after merge, apply on approval (keyless)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 200 lines of workflow plus tests | T-1116, T-1103 |

**Read only these spec sections:** `docs/decisions/0003-infrastructure-changes-owner-decides-pipeline-applies.md`; `.github/workflows/ci.yml` (the `terraform` job and `ci-integrity`); `infra/terraform/README.md`; the rule that workflow files are merged by the owner is stated here and enforced by `CODEOWNERS` (this task adds it). Nothing else is needed.

## Goal

Infrastructure changes reach Google Cloud only as a plan the owner has seen, applied after his approval, with no stored credentials. The owner's work is reading a plan summary and tapping Approve.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `.github/workflows/terraform.yml` | `plan-<env>` then `apply-<env>` on push to `main` touching `infra/terraform/**`; no job runs on pull requests |
| Create | `.github/CODEOWNERS` | owner as code owner for `.github/workflows/**`, `infra/**`, `scripts/infra/**` |
| Create | `tools/check_terraform_workflow.py` | static checks below, run in CI |
| Create | `tools/terraform_plan_summary.py` | redacted summary + digest from `terraform show -json`, and the protected-address deny-list check |
| Change | `.github/workflows/ci.yml` | run the static checks; `ci-integrity` waits for them |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1117` |

(Workflow files are owner-merged; the builder prepares the PR, the owner merges it.)

## Algorithm

1. **Pull requests get no cloud identity.** `terraform.yml` has no `pull_request` trigger and never `pull_request_target`; PR checks stay in `ci.yml` (`fmt`, `validate`, `test`, guards, no credentials). Reason: on a PR the workflow file and `tools/terraform_plan_summary.py` are the author's code, and a step inside the PR's own workflow cannot be trusted to skip itself, so there is no safe way to give a PR a read identity (state holds full attribute values; the planner reads state). The owner sees the plan only after merge, in step 2, before anything applies. IDs for the plan jobs come from **repository variables** with an environment suffix (`GCP_WIF_PLAN_PROVIDER_STAGING`, `GCP_PLANNER_SA_STAGING`, `TF_STATE_BUCKET_STAGING`, and `_PRODUCTION`).
2. **Push to main** (jobs run only if repository variable `TF_APPLY_ENABLED == 'true'`):
   - `plan-<env>`: planner identity (main-run condition), same read-only commands, writes the redacted summary and its SHA-256 **digest** to the job summary and to a small text artifact with `retention-days: 1`. The summary is the thing the owner approves.
   - `apply-<env>` (`environment: staging` or `production`; production has the owner as required reviewer): authenticates as the applier, **re-plans**, recomputes the summary digest with the same tool, aborts if it differs from the approved digest, then applies that plan in the same job. No `-auto-approve` on a plan that was not just produced in this job and digest-checked.
3. **Safety:** one concurrent apply per environment (`concurrency`); production runs only if the staging job of the same run succeeded or `infra/terraform/envs/staging/**` did not change; **`tools/terraform_plan_summary.py --check-protected` fails the plan job when any delete or replace touches a protected address** (log bucket, KMS key rings and keys, Firestore database, Secret Manager secrets, the Terraform state bucket, the Artifact Registry repository, and the Workload Identity pools, providers and the planner and applier service accounts), by address, independent of `prevent_destroy` (which vanishes if the block is deleted). Plan jobs also fail if a configuration uses `external` data sources, `local-exec` or `remote-exec` provisioners.
4. **No keys, no secrets:** no `credentials_json`, no long-lived secrets in the workflow; every third-party action pinned by 40-character commit SHA (repository rule); minimal `permissions` per job; `id-token: write` only on jobs that authenticate; no project ids, numbers or service-account emails written to logs or comments (the repository is public): GitHub Actions masks values stored as variables only if they are secrets, so the summary tool must itself redact member emails and project ids.
5. **CODEOWNERS and the switch:** add `.github/CODEOWNERS` as above. The apply jobs stay disabled until the owner sets `TF_APPLY_ENABLED=true`, which he does after turning on "Require review from Code Owners" on `main` (the T-1117 PR description says so).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| TFW AC1 | `terraform.yml` has no pull-request trigger; a pull request obtains no cloud identity and applies nothing |
| TFW AC2 | Apply runs only from `main`, only in an Environment, re-plans and aborts if the summary digest differs from the approved one |
| TFW AC3 | Production apply waits for the Environment's required reviewer |
| TFW AC4 | The workflow contains no credential files, no `pull_request_target`, no unpinned action, no plan-file upload |
| TFW AC5 | A plan that deletes or replaces a protected address (Terraform-managed: secrets, Artifact Registry and the like; the state bucket, identity pools and the two service accounts are created by the bootstrap script, not by Terraform, so they are protected by the applier having no rights on them (T-1116 step 6), not by this deny-list) fails before the approval prompt |
| TFW AC8 | The summary names changed attributes only (no values) and flags `SENSITIVE` change categories (unit-tested with fixture plans: IAM member widened, sink filter changed, retention lowered, KMS and WIF changes, secret changes) |
| TFW AC6 | Plan jobs use `-lock=false` and a read-only state role; nothing a plan job runs can write state |
| TFW AC7 | Comments, summaries and artifacts contain no values, project ids or member emails; artifact retention is 1 day |

## Tests that must pass

- `tools/check_terraform_workflow.py` (unit tests with good and bad fixture workflows) covering AC1, AC2 and AC4 to AC7 statically (including: no `pull_request` trigger in `terraform.yml`, and plan jobs authenticate only under `push` on `main`): apply jobs have `environment:`, trigger is `push` on `main`, the digest comparison step exists, no `upload-artifact` of a `*.tfplan` or `terraform show` output, no `-auto-approve` outside the apply job, `-lock=false` on PR plans, `retention-days: 1`, no `credentials_json`, no `pull_request_target`, all `uses:` pinned to a 40-character SHA.
- `tools/terraform_plan_summary.py` unit tests with fixture plan JSON: redaction (no attribute values, project ids or emails survive), stable digest, and the protected-address deny-list (delete and replace of each protected type fail; a plain update passes).
- `actionlint` clean.
- AC1 and AC3 are verified by a documented dry run in a throwaway branch in the staging environment (the PR description records the run link).

## Edge cases and traps

- The planner must not be able to read secret values (secrets are not in state by design; confirm in the plan summary job that no `sensitive` value is printed).
- There is no pull-request plan; what is applied is always produced on `main`, inside the apply job, and digest-checked against what the owner approved.
- Say plainly in the approval prompt text that the owner approves the redacted summary and digest, not a signed plan file.
- Terraform refuses a plan file if state changed since it was made; treat that as a normal failure that re-runs the plan job.

## Out of scope

- Bootstrap (T-1116); application deploys (T-1104); multi-tenant fan-out.

## Done when

- Tests above pass, every required check is green (S10 10.1), cross-vendor security review of the identity conditions and of the apply gating, Definition of done in S10 10.4. The owner merges the workflow and `CODEOWNERS` files and sets the repository variable.
