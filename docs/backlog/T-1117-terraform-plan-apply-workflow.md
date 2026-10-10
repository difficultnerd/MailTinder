# T-1117: Terraform plan on pull requests, apply on approval (keyless)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 200 lines of workflow plus tests | T-1116, T-1103 |

**Read only these spec sections:** `docs/decisions/0003-infrastructure-changes-owner-decides-pipeline-applies.md`; `.github/workflows/ci.yml` (the `terraform` job and `ci-integrity`); `infra/terraform/README.md`; `docs/specs/S13-agent-working-rules.md` on workflows (owner-only). Nothing else is needed.

## Goal

Infrastructure changes reach Google Cloud only as a plan the owner has seen, applied after his approval, with no stored credentials. The owner's work is reading a plan summary and tapping Approve.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `.github/workflows/terraform.yml` | `plan` on pull requests touching `infra/terraform/**`; `plan-<env>` then `apply-<env>` on push to `main` |
| Create | `tools/check_terraform_workflow.py` | static checks below, run in CI |
| Change | `.github/workflows/ci.yml` | run the static checks; `ci-integrity` waits for them |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1117` |

(Workflow files are owner-merged; the builder prepares the PR, the owner merges it.)

## Algorithm

1. **Pull request:** job `plan` (permissions `contents: read`, `id-token: write`, `pull-requests: write`; same-repository PRs only; never `pull_request_target`). Authenticate with `google-github-actions/auth` using the planner provider (Environment variables `GCP_WIF_PROVIDER`, `GCP_PLANNER_SA`), `terraform init -backend-config="bucket=$TF_STATE_BUCKET"`, `terraform plan -lock-timeout=5m -out=tfplan`, then post a short summary comment: counts of add/change/destroy and a highlighted list of every destroy or replace.
2. **Push to main:** `plan-staging` then `apply-staging` (Environment `staging`), then `plan-production` then `apply-production` (Environment `production`, which already requires the owner as reviewer). The apply job downloads the artifact created by the plan job of the SAME run, prints its sha256 and the summary to the job summary, and runs `terraform apply tfplan`. No second plan, no `-auto-approve` on a fresh plan.
3. **Safety:** one concurrent apply per environment (`concurrency`); production runs only if the staging job of the same run succeeded or `infra/terraform/envs/staging/**` did not change; any plan containing a destroy of a resource with `prevent_destroy` fails the job before the approval prompt.
4. **No keys, no secrets:** no `credentials_json`, no long-lived secrets in the workflow; every third-party action pinned by commit SHA (repository rule); minimal `permissions` per job; `id-token: write` only on jobs that authenticate.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| TFW AC1 | A pull request that changes `infra/terraform/**` gets a plan summary comment and applies nothing |
| TFW AC2 | Apply runs only from `main`, only in an Environment, and applies the plan file produced in the same run |
| TFW AC3 | Production apply waits for the Environment's required reviewer |
| TFW AC4 | The workflow contains no credential files, no `pull_request_target`, no unpinned action |

## Tests that must pass

- `tools/check_terraform_workflow.py` (unit tests with good and bad fixture workflows) covering AC2 to AC4 statically: apply jobs have `environment:`, trigger is `push` on `main`, they use `actions/download-artifact` and `terraform apply tfplan`, no `-auto-approve`, no `credentials_json`, no `pull_request_target`, all `uses:` pinned to a 40-character SHA.
- `actionlint` clean.
- AC1 and AC3 are verified by a documented dry run in a throwaway branch in the staging environment (the PR description records the run link).

## Edge cases and traps

- The planner must not be able to read secret values (secrets are not in state by design; confirm in the plan summary job that no `sensitive` value is printed).
- A plan from a pull request is advisory only; the plan that is applied is always the one created on `main`.
- Terraform refuses a plan file if state changed since it was made; treat that as a normal failure that re-runs the plan job.

## Out of scope

- Bootstrap (T-1116); application deploys (T-1104); multi-tenant fan-out.

## Done when

- Tests above pass, every required check is green (S10 10.1), cross-vendor security review of the identity conditions and of the apply gating, Definition of done in S10 10.4.
