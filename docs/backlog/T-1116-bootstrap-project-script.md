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

`scripts/infra/bootstrap-project.sh --project <id> --env staging|production --repo <owner/name> [--dry-run] [--yes]` (production additionally needs `--i-know-the-log-bucket-lock-is-permanent`). Exit 0 on success or when everything already exists; prints, last, the exact values to store as GitHub variables (repository variables with an environment suffix for the plan job, Environment variables for the apply job; see step 9).

## Algorithm

1. **Refuse to run unsafely:** the active gcloud account must be a human user (not `*.gserviceaccount.com`), the project must exist, billing must be linked (print the console link and stop if not; linking billing needs a billing admin and is not scripted), and `--env production` additionally needs the explicit flag.
2. **Print the plan** (resources and roles below) and wait for `y` unless `--yes`; `--dry-run` prints and stops.
3. **Enable only what is needed to bootstrap:** `serviceusage`, `cloudresourcemanager`, `iam`, `iamcredentials`, `sts`, `storage`. Terraform enables the rest (`modules/foundation/apis.tf`).
4. **State bucket** `gs://<project>-tfstate`: `us-central1`, uniform access, public access prevention, versioning on.
5. **Two separate Workload Identity pools** (a principalSet is scoped to a pool, not a provider, so one pool with two providers is not safe):
   - `github-plan` with one provider whose `attribute_condition` admits `assertion.repository == '<repo>'` AND (`assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.job_workflow_ref == '<repo>/.github/workflows/terraform.yml@refs/heads/main'`); pull requests are not admitted.
   - `github-apply` with one provider whose condition is `assertion.repository == '<repo>' && assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.environment == '<env>' && assertion.job_workflow_ref == '<repo>/.github/workflows/terraform.yml@refs/heads/main'`. Only this provider maps `attribute.environment`, `attribute.ref` and `attribute.job_workflow_ref`.
6. **Service accounts and bindings.**
   - `mt-planner`: bound to `principalSet://…/github-plan/attribute.repository/<repo>`. Roles: narrow per-service viewers instead of `roles/viewer` (for example `cloudkms.viewer`, `datastore.viewer`, `run.viewer`, `secretmanager.viewer` (metadata only), `iam.securityReviewer`, `serviceusage.serviceUsageViewer`, `artifactregistry.reader`, `logging.viewer`, `cloudtasks.viewer`, `cloudscheduler.viewer`); confirm each with `gcloud iam roles describe` and record anything that exposes data-plane contents; state bucket: `objectViewer` only (the planner uses `-lock=false`).
   - `mt-applier`: bound ONLY to `principalSet://…/github-apply/attribute.environment/<env>` (a principalSet member matches one attribute, so the ref and workflow constraints are NOT in the member: they are enforced by the `github-apply` provider's `attribute_condition` in step 5, which admits nothing but `refs/heads/main` and the pinned workflow; never bind to `attribute.repository`). Roles: derive the list from every resource type in `modules/foundation` and `modules/runtime` (resources the bootstrap script itself creates are NOT covered by that derivation; the applier gets `roles/storage.objectAdmin` on the state bucket only and **no rights at all on the identity pools, providers or the planner and applier service accounts**, so it cannot widen its own access), each mapped in a table in the script header to the resource that needs it, resource-level where possible. The project-level grant of `roles/resourcemanager.projectIamAdmin` MUST carry an IAM Condition (`api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([...the roles the modules bind...])`; confirm in current Google documentation that this condition type is supported for that role); never `roles/owner`, `roles/editor` or `roles/iam.securityAdmin`.
7. **Never** create a service account key, never read a secret value, never touch Terraform state.
8. **Idempotent:** a second run changes nothing and says so; partial failures resume.
9. **Print, last, the exact values** to store as GitHub variables: **repository variables** with an environment suffix for what the post-merge plan jobs need (`GCP_WIF_PLAN_PROVIDER_<ENV>`, `GCP_PLANNER_SA_<ENV>`, `TF_STATE_BUCKET_<ENV>`, `GCP_PROJECT_ID_<ENV>`) and **Environment variables** for the apply job (`GCP_WIF_APPLY_PROVIDER`, `GCP_APPLIER_SA`) and the repository variable `TF_APPLY_ENABLED=false` reminder.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| BOOT AC1 | `--dry-run` creates nothing and lists every resource and role |
| BOOT AC2 | A second run is a no-op |
| BOOT AC3 | It refuses to run as a service account, without billing, or for production without the explicit flag |
| BOOT AC4 | No service account key is ever created |
| BOOT AC5 | A principal from the planner pool cannot impersonate `mt-applier`; the applier binding names the environment attribute, never the repository alone, and the apply provider's condition pins `refs/heads/main` and the workflow |
| BOOT AC6 | The project-IAM-admin grant to `mt-applier` carries a roles-limiting condition and no owner/editor/securityAdmin role is grantable |

## Tests that must pass

- `boot_ac1_dry_run_creates_nothing`, `boot_ac2_second_run_is_noop`, `boot_ac3_refuses_unsafe_contexts`, `boot_ac4_no_key_creation_call` (bash tests with a `gcloud` stub that records every call; the last one fails if the stub ever sees `keys create`), `boot_ac5_planner_cannot_reach_applier` (asserts from the recorded calls that the applier's `workloadIdentityUser` member strings reference only the apply pool and its environment attribute, and that the apply provider's `attribute_condition` contains the `refs/heads/main`, environment and `job_workflow_ref` terms), `boot_ac6_iam_admin_grant_is_conditioned`.
- `shellcheck` clean.

## Edge cases and traps

- Production's log bucket lock is irreversible; the extra flag exists to make the owner say so.
- The applier's role list is the main security decision: a reviewer from another vendor must confirm it grants no more than the modules need, and that the attribute condition cannot be satisfied from a branch, fork or pull request.
- Do not print project numbers or emails beyond what the owner needs to set the GitHub variables, and do not write any project id into the repository (the repo is public).
- The role table is a deliverable: a reviewer from another vendor checks each role against the resource that needs it.

## Out of scope

- The workflow itself (T-1117); secret values and console-only steps (T-1118); creating projects or linking billing.

## Done when

- Tests above pass and every required check is green (S10 10.1); Definition of done in S10 10.4; cross-vendor security review of the role table and the attribute conditions.
