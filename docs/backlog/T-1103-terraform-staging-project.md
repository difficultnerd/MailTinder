# T-1103: Terraform, staging project

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 150 lines of HCL plus a parity script | T-1102b |

**Read only these spec sections:** S4 section 1 "Environments" (`docs/specs/S4-architecture.md`); S10 decision T5 in section 11 (`docs/specs/S10-test-strategy.md`); the "Backlog seeds for S12" line on staging Terraform in `docs/planning-roadmap.md`; the two module interfaces in T-1102a and T-1102b. Nothing else is needed.

## Goal

A staging Google Cloud project built from the same `foundation` and `runtime` modules as production, with its own state, so deploys, smoke tests (T-1105) and the ZAP baseline run somewhere that is not production (decision T5). A small script proves staging and production use the same modules.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `infra/terraform/envs/staging/main.tf`, `backend.tf`, `variables.tf`, `versions.tf`, `terraform.tfvars.example` | Staging root |
| Create | `scripts/tf_env_parity.sh` | Fails if the two roots call different modules |
| Change | `infra/terraform/README.md` | Staging bootstrap steps |
| Change | `.github/workflows/ci.yml` | `terraform` job: `validate` both roots and run the parity script |

## Types and signatures

```hcl
# envs/staging/main.tf
module "foundation" {
  source          = "../../modules/foundation"
  project_id      = var.project_id
  env             = "staging"
  lock_log_bucket = false        # [DEFAULT] staging can be torn down; production stays locked
}
module "runtime" {
  source           = "../../modules/runtime"
  project_id       = var.project_id
  env              = "staging"
  service_accounts = module.foundation.service_accounts
  kms_key_id       = module.foundation.kms_key_id
  secret_ids       = module.foundation.secret_ids
  max_instances    = 1           # [DEFAULT] staging traffic is tiny
}
# backend.tf
terraform { backend "gcs" { prefix = "staging" } }   # bucket passed with -backend-config, never committed
```

```bash
# scripts/tf_env_parity.sh: exits 1 when the sorted `source = "..."` lines of envs/prod/*.tf and envs/staging/*.tf differ
```

## Algorithm

1. Staging is a separate project (`<name>-staging`, chosen by James) with its own billing link and its own state bucket `gs://<staging-project>-tfstate` (S4 Environments: separate state).
2. Use exactly the same modules; staging may differ only in variables: `env`, `project_id`, `lock_log_bucket = false`, `max_instances = 1`. Any other difference needs a comment with its reason.
3. `tf_env_parity.sh`: `grep -h 'source *=' envs/prod/*.tf | sort` versus the same for staging; `diff` them; exit non-zero on difference.
4. README steps for James: create the staging project, link billing, create the state bucket (same command as production with the staging name), `terraform init -backend-config="bucket=<staging-project>-tfstate"`, plan, apply, then add the staging secret values (separate values from production; never copy production secrets). Create a staging OAuth client (Testing mode) in this project; its secret goes into the staging `oauth-client-secret`.
5. Staging OAuth consent screen stays in Testing mode with James's test users only.

**Waits on S11** (defaults used): staging teardown and cost policy; whether staging data is ever reset (`[DEFAULT]` no scheduled reset; synthetic data only); naming of the two projects.

## Acceptance criteria

None enforced by `ac-coverage` (Terraform). Decision T5 (staging project) is met by this task.

## Tests that must pass

- `scripts/tf_env_parity.sh` exits 0 in the `terraform` CI job.
- `terraform validate` passes for `envs/staging` and `envs/prod`.
- The module tests from T-1102a and T-1102b still pass with `env = "staging"` (add one `run` block in each module's test file with staging variables).

## Edge cases and traps

- Never reuse the production state bucket, OAuth client or secret values in staging.
- Do not fork the modules for staging; pass variables instead (parity script fails otherwise).
- Staging holds synthetic data only; no real person's mailbox is linked there.
- Unlocked log bucket is a staging-only setting; the production root must keep the module default (`true`).

## Out of scope

- The smoke-test identity and its staging-only roles (T-1105); deploys (T-1104); the Gmail sandbox account (T-1106).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has applied staging and recorded the staging Hosting URL in the repository variables (`STAGING_URL`) for T-1104 and T-1105.
