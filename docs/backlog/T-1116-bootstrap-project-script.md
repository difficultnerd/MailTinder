# T-1116: One-command project bootstrap (state bucket, first APIs, planner and applier identities)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 250 lines of bash plus a stub-based test | T-1102b |

**Read only these spec sections:** `docs/decisions/0003-infrastructure-changes-owner-decides-pipeline-applies.md`; `infra/terraform/README.md` "Bootstrap" and "Cautions"; `infra/terraform/modules/runtime/deploy_identity.tf` (the pattern to copy); `docs/security/asvs-l2-register.md` rows V13.2.1 and V13.2.2. Nothing else is needed.

## Goal

The owner pastes one command per project (staging and production). It shows what it will do, asks for confirmation, then creates exactly what Terraform cannot create for itself, so every later change flows through the plan/apply pipeline (T-1117) and the owner only approves.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/infra/bootstrap-project.sh` | the script |
| Create | `scripts/infra/tests/bootstrap_test.sh` | tests against a `gcloud` stub on PATH |
| Change | `infra/terraform/README.md` | replace Bootstrap steps 1 to 4 with the script, keep the cautions |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1116` |

## Types and signatures

`scripts/infra/bootstrap-project.sh --project <id> --env staging|production --repo <owner/name> [--dry-run] [--yes]` (production additionally needs `--i-know-the-log-bucket-lock-is-permanent`). Exit 0 on success or when everything already exists; prints, last, the exact values to store as GitHub Environment variables (`GCP_PROJECT_ID`, `GCP_WIF_PROVIDER`, `GCP_PLANNER_SA`, `GCP_APPLIER_SA`, `TF_STATE_BUCKET`).

## Algorithm

1. **Refuse to run unsafely:** the active gcloud account must be a human user (not `*.gserviceaccount.com`), the project must exist, billing must be linked (print the console link and stop if not; linking billing needs a billing admin and is not scripted), and the project id must match `--env` rules the owner supplied (never guess production).
2. **Print the plan** (resources and roles below) and wait for `y` unless `--yes`; `--dry-run` prints and stops.
3. **Enable only what is needed to bootstrap:** `serviceusage`, `cloudresourcemanager`, `iam`, `iamcredentials`, `sts`, `storage`. Terraform enables the rest (`modules/foundation/apis.tf`).
4. **State bucket** `gs://<project>-tfstate`: `us-central1`, uniform access, public access prevention, versioning on (the README commands).
5. **Workload Identity:** pool `github-apply` and provider with `attribute_condition = "assertion.repository == '<repo>' && assertion.ref == 'refs/heads/main' && assertion.environment == '<env>'"` (copy the structure of `deploy_identity.tf`). A second provider for the planner whose condition pins the repository and pull-request runs but never `ref == main` apply rights.
6. **Service accounts** `mt-planner` (project `roles/viewer`, `roles/iam.securityReviewer`, object viewer on the state bucket) and `mt-applier` (roles needed by `modules/foundation` and `modules/runtime`; derive the minimal predefined-role list by reading every resource type in those modules and record the table in the script header; no `roles/owner`; project IAM changes limited to what the modules bind). Each is impersonable only through its pool (`roles/iam.workloadIdentityUser` on `principalSet://…/attribute.repository/<repo>`).
7. **Never** create a service account key, never read a secret value, never touch Terraform state.
8. **Idempotent:** a second run changes nothing and says so; partial failures resume.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| BOOT AC1 | `--dry-run` creates nothing and lists every resource and role |
| BOOT AC2 | A second run is a no-op |
| BOOT AC3 | It refuses to run as a service account, without billing, or for production without the explicit flag |
| BOOT AC4 | No service account key is ever created |

## Tests that must pass

- `boot_ac1_dry_run_creates_nothing`, `boot_ac2_second_run_is_noop`, `boot_ac3_refuses_unsafe_contexts`, `boot_ac4_no_key_creation_call` (bash tests with a `gcloud` stub that records every call; the last one fails if the stub ever sees `keys create`).
- `shellcheck` clean.

## Edge cases and traps

- Production's log bucket lock is irreversible; the extra flag exists to make the owner say so.
- The applier's role list is the main security decision: a reviewer from another vendor must confirm it grants no more than the modules need, and that the attribute condition cannot be satisfied from a branch, fork or pull request.
- Do not print project numbers or emails beyond what the owner needs to set the GitHub variables.

## Out of scope

- The workflow itself (T-1117); secret values and console-only steps (T-1118); creating projects or linking billing.

## Done when

- Tests above pass and every required check is green (S10 10.1); Definition of done in S10 10.4; cross-vendor security review of the role table and the attribute conditions.
