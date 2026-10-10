# T-1117: Terraform plan on pull requests, apply on approval (keyless)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 200 lines of workflow plus tests | T-1116, T-1103 |

**Read only these spec sections:** `docs/decisions/0003-infrastructure-changes-owner-decides-pipeline-applies.md`; `.github/workflows/ci.yml` (the `terraform` job and `ci-integrity`); `infra/terraform/README.md`; the rule that workflow files are merged by the owner is stated here and enforced by `CODEOWNERS` (this task adds it). Nothing else is needed.

## Goal

Infrastructure changes reach Google Cloud only as a plan the owner has seen, applied after his approval, with no stored credentials. The owner's work is reading a plan summary and tapping Approve.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `.github/workflows/terraform.yml` | `plan` on pull requests touching `infra/terraform/**`; `plan-<env>` then `apply-<env>` on push to `main` |
| Create | `.github/CODEOWNERS` | owner as code owner for `.github/workflows/**`, `infra/**`, `scripts/infra/**` |
| Create | `tools/check_terraform_workflow.py` | static checks below, run in CI |
| Create | `tools/terraform_plan_summary.py` | redacted summary + digest from `terraform show -json`, and the protected-address deny-list check |
| Change | `.github/workflows/ci.yml` | run the static checks; `ci-integrity` waits for them |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1117` |

(Workflow files are owner-merged; the builder prepares the PR, the owner merges it.)

## Algorithm

1. **Pull request:** job `plan` (permissions `contents: read`, `id-token: write`, `pull-requests: write`; same-repository PRs only; never `pull_request_target`). **The PR-controlled code problem:** on a same-repository PR the workflow file and the summary tool are the PR author's code, and they would run with the planner identity. So (a) the job checks out `main` into a second directory and runs `tools/terraform_plan_summary.py` from that copy, never the PR's; (b) a first step lists the changed paths and, if the PR changes `.github/workflows/**`, `tools/terraform_plan_summary.py` or `scripts/infra/**`, the job posts "plan skipped: workflow or tool changed, owner review required" and does **not** authenticate; (c) the planner stays read-only with narrow viewer roles, so the worst a collaborator's PR can read is metadata. Then authenticate with `google-github-actions/auth` using the planner provider. The IDs come from **repository variables** with an environment suffix (`GCP_WIF_PLAN_PROVIDER_STAGING`, `GCP_PLANNER_SA_STAGING`, `TF_STATE_BUCKET_STAGING`, and the `_PRODUCTION` equivalents), because the PR plan job names no Environment (an Environment's branch policy would reject PR branches); `terraform init -lockfile=readonly -backend-config="bucket=$TF_STATE_BUCKET"` (provider hashes must match the committed lock file), `terraform plan -lock=false -out=tfplan` (the planner has read-only state access and takes no lock), then `tools/terraform_plan_summary.py` posts a PR comment with the **redacted summary only**: counts, and per change the resource address, type and action, plus for update and replace the **names** of the changed attributes (names only, never values) and a `SENSITIVE` flag on any change to an IAM binding or member, a log sink or its filter, a retention or lock setting, a KMS key, a Workload Identity condition, or a Secret Manager secret; the flag tells the approver to read that part of the PR diff, because a name alone cannot show that an IAM member was widened. Never `terraform show` text. The plan file stays on the runner and is not uploaded.
2. **Push to main** (jobs run only if repository variable `TF_APPLY_ENABLED == 'true'`):
   - `plan-<env>`: planner identity (main-run condition), same read-only commands, writes the redacted summary and its SHA-256 **digest** to the job summary and to a small text artifact with `retention-days: 1`. The summary is the thing the owner approves.
   - `apply-<env>` (`environment: staging` or `production`; production has the owner as required reviewer): authenticates as the applier, **re-plans**, recomputes the summary digest with the same tool, aborts if it differs from the approved digest, then applies that plan in the same job. No `-auto-approve` on a plan that was not just produced in this job and digest-checked.
3. **Safety:** one concurrent apply per environment (`concurrency`); production runs only if the staging job of the same run succeeded or `infra/terraform/envs/staging/**` did not change; **`tools/terraform_plan_summary.py --check-protected` fails the plan job when any delete or replace touches a protected address** (log bucket, KMS key rings and keys, Firestore database, Secret Manager secrets, the Terraform state bucket, the Artifact Registry repository, and the Workload Identity pools, providers and the planner and applier service accounts), by address, independent of `prevent_destroy` (which vanishes if the block is deleted). Plan jobs also fail if a configuration uses `external` data sources, `local-exec` or `remote-exec` provisioners.
4. **No keys, no secrets:** no `credentials_json`, no long-lived secrets in the workflow; every third-party action pinned by 40-character commit SHA (repository rule); minimal `permissions` per job; `id-token: write` only on jobs that authenticate; no project ids, numbers or service-account emails written to logs or comments (the repository is public): GitHub Actions masks values stored as variables only if they are secrets, so the summary tool must itself redact member emails and project ids.
5. **CODEOWNERS and the switch:** add `.github/CODEOWNERS` as above. The apply jobs stay disabled until the owner sets `TF_APPLY_ENABLED=true`, which he does after turning on "Require review from Code Owners" on `main` (the T-1117 PR description says so).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| TFW AC1 | A pull request that changes `infra/terraform/**` gets a plan summary comment and applies nothing |
| TFW AC2 | Apply runs only from `main`, only in an Environment, re-plans and aborts if the summary digest differs from the approved one |
| TFW AC3 | Production apply waits for the Environment's required reviewer |
| TFW AC4 | The workflow contains no credential files, no `pull_request_target`, no unpinned action, no plan-file upload |
| TFW AC5 | A plan that deletes or replaces a protected address (including secrets, state bucket, Artifact Registry, identity pools and service accounts) fails before the approval prompt |
| TFW AC8 | A PR that changes the workflow, the summary tool or `scripts/infra/**` gets no authenticated plan; the PR summary tool always runs from `main`; a `SENSITIVE` change is flagged in the summary |
| TFW AC6 | Pull-request plans use `-lock=false` and a read-only state role; nothing a PR runs can write state |
| TFW AC7 | Comments, summaries and artifacts contain no values, project ids or member emails; artifact retention is 1 day |

## Tests that must pass

- `tools/check_terraform_workflow.py` (unit tests with good and bad fixture workflows) covering AC2 to AC7 statically: apply jobs have `environment:`, trigger is `push` on `main`, the digest comparison step exists, no `upload-artifact` of a `*.tfplan` or `terraform show` output, no `-auto-approve` outside the apply job, `-lock=false` on PR plans, `retention-days: 1`, no `credentials_json`, no `pull_request_target`, all `uses:` pinned to a 40-character SHA.
- `tools/terraform_plan_summary.py` unit tests with fixture plan JSON: redaction (no attribute values, project ids or emails survive), stable digest, and the protected-address deny-list (delete and replace of each protected type fail; a plain update passes).
- `actionlint` clean.
- AC1 and AC3 are verified by a documented dry run in a throwaway branch in the staging environment (the PR description records the run link).

## Edge cases and traps

- The planner must not be able to read secret values (secrets are not in state by design; confirm in the plan summary job that no `sensitive` value is printed).
- A plan from a pull request is advisory only; what is applied is always produced on `main`, inside the apply job, and digest-checked against what the owner approved.
- Say plainly in the approval prompt text that the owner approves the redacted summary and digest, not a signed plan file.
- Terraform refuses a plan file if state changed since it was made; treat that as a normal failure that re-runs the plan job.

## Out of scope

- Bootstrap (T-1116); application deploys (T-1104); multi-tenant fan-out.

## Done when

- Tests above pass, every required check is green (S10 10.1), cross-vendor security review of the identity conditions and of the apply gating, Definition of done in S10 10.4. The owner merges the workflow and `CODEOWNERS` files and sets the repository variable.
