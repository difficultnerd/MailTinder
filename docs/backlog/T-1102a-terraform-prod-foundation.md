# T-1102a: Terraform, production foundation: service accounts, IAM, KMS, Firestore, secrets, logs

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 450 lines of HCL plus tests | needs S11 |

**Read only these spec sections:** S4 section 1 (Services table, Environments) and section 2 (the service account role table and the sentence after it) (`docs/specs/S4-architecture.md`); S6 sections 5 (KMS row) and 7 (last sentence) (`docs/specs/S6-security.md`); S5 "Firestore (server)" (Retention column) and "Logs and telemetry" (`docs/specs/S5-data-inventory.md`); register rows V6.3.2, V12.3.3, V13.2.1, V13.2.2, V13.2.3, V13.3.1, V13.3.2, V16.2.3, V16.4.2, V16.4.3 in `docs/security/asvs-l2-register.md`. Nothing else is needed.

## Goal

A reusable Terraform module, `foundation`, and the production root that uses it: project APIs, one service account per service with only the S4 roles, the KMS key that wraps every user's `data_key`, the Firestore database in `us-central1` with its TTL policies, Secret Manager secret containers with one accessor list each, the locked 90-day log bucket, data access audit logs and the Artifact Registry repository. James reviews the plan and applies it; nothing here holds a secret value.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `infra/terraform/modules/foundation/versions.tf` | Terraform and provider constraints |
| Create | `infra/terraform/modules/foundation/variables.tf`, `outputs.tf` | Interface below |
| Create | `infra/terraform/modules/foundation/apis.tf` | `google_project_service` for each API |
| Create | `infra/terraform/modules/foundation/service_accounts.tf` | `api`, `unsub`, `worker`, `tasks-invoker`, `scheduler-invoker` |
| Create | `infra/terraform/modules/foundation/kms.tf` | Key ring, key, authoritative IAM binding |
| Create | `infra/terraform/modules/foundation/firestore.tf` | Database, TTL fields, project IAM for Firestore |
| Create | `infra/terraform/modules/foundation/secrets.tf` | Secret containers and accessor bindings |
| Create | `infra/terraform/modules/foundation/logging.tf` | Bucket, sink, `_Default` exclusion, audit config |
| Create | `infra/terraform/modules/foundation/artifact_registry.tf` | Docker repository |
| Create | `infra/terraform/modules/foundation/tests/foundation.tftest.hcl` | Plan-time tests with `mock_provider` |
| Create | `infra/terraform/envs/prod/main.tf`, `backend.tf`, `variables.tf`, `versions.tf`, `terraform.tfvars.example` | Production root |
| Create | `infra/terraform/README.md` | Bootstrap and apply steps for James |
| Change | `.gitignore` | `.terraform/`, `*.tfstate`, `*.tfstate.*`, `*.tfvars` (keep `*.tfvars.example`), `.terraform.lock.hcl` is committed |
| Change | `.github/workflows/ci.yml` | Job `terraform`: `fmt -check`, `init -backend=false`, `validate`, `test` for the module |

## Types and signatures

```hcl
# modules/foundation/variables.tf
variable "project_id"         { type = string }
variable "region"             { type = string  default = "us-central1" }  # S4: one region variable
variable "env"                { type = string  validation { condition = contains(["prod", "staging"], var.env) error_message = "env is prod or staging" } }
variable "lock_log_bucket"    { type = bool    default = true }          # staging sets false (T-1103)
variable "kms_rotation_days"  { type = number  default = 365 }           # S6 5: yearly

# modules/foundation/outputs.tf
output "service_accounts"   { value = { api = ..., unsub = ..., worker = ..., tasks_invoker = ..., scheduler_invoker = ... } } # emails
output "kms_key_id"         { value = google_kms_crypto_key.data_key_kek.id }
output "secret_ids"         { value = { oauth_client_secret = ..., jev_api_key = ..., email_lookup_hmac = ..., log_pseudonym_hmac = ... } }
output "artifact_repo"      { value = "${var.region}-docker.pkg.dev/${var.project_id}/mailtinder" }
output "log_bucket_id"      { value = google_logging_project_bucket_config.app.id }
```

## Algorithm

1. **Versions:** `required_version = ">= 1.9"`; provider `hashicorp/google` `~> 7.0` `[DEFAULT]` (latest major at writing; if `terraform init` cannot resolve it, use the newest available major and note it in the PR). Commit `.terraform.lock.hcl`.
2. **APIs:** enable `run`, `cloudtasks`, `cloudscheduler`, `firestore`, `cloudkms`, `secretmanager`, `aiplatform`, `logging`, `monitoring`, `artifactregistry`, `iam`, `iamcredentials`, `sts`, `firebase`, `firebasehosting` (all `.googleapis.com`), each with `disable_on_destroy = false`.
3. **Service accounts:** `mt-api`, `mt-unsub`, `mt-worker` (runtime), `mt-tasks-invoker` (OIDC identity on Cloud Tasks calls to `unsub`), `mt-scheduler-invoker` (OIDC identity on Cloud Scheduler calls to `worker`). No keys are ever created (`google_service_account_key` must not appear).
4. **KMS:** key ring `mailtinder` in `var.region`; crypto key `data-key-kek`, purpose `ENCRYPT_DECRYPT`, `rotation_period = "${var.kms_rotation_days * 86400}s"`, protection `SOFTWARE`, `lifecycle { prevent_destroy = true }`. Grant `roles/cloudkms.cryptoKeyEncrypterDecrypter` with `google_kms_crypto_key_iam_binding` (authoritative for that role on that key) listing exactly the `api`, `unsub` and `worker` accounts (S4 2: exactly three holders).
5. **Firestore:** `google_firestore_database` name `(default)`, `location_id = var.region`, type `FIRESTORE_NATIVE`, `delete_protection_state = "DELETE_PROTECTION_ENABLED"`, `deletion_policy = "ABANDON"`. TTL backstops (S5 Retention; the sweeper is the control): `google_firestore_field` with `ttl_config {}` and an empty `index_config {}` on field `expires_at` of collections `sessions`, `jobs`, `needs_attention`, `rate_limits`, `classifier_eval` `[DEFAULT field name: confirm with T-301's document shapes]`. Project-level `roles/datastore.user` for `api`, `unsub`, `worker` only (Firestore has no per-collection IAM).
6. **Secrets** (containers only, `replication { user_managed { replicas { location = var.region } } }`): `oauth-client-secret`, `jev-api-key`, `email-lookup-hmac-key`, `log-pseudonym-hmac-key`. Accessors with `google_secret_manager_secret_iam_binding` (authoritative) on `roles/secretmanager.secretAccessor`:
   - `oauth-client-secret`: `api`, `unsub`, `worker`.
   - `jev-api-key`: `api` only.
   - `email-lookup-hmac-key`: `api` only.
   - `log-pseudonym-hmac-key`: `api`, `unsub`, `worker`.
   No `google_secret_manager_secret_version` resources: values never enter Terraform state. James adds them (step 10).
7. **Vertex AI:** `roles/aiplatform.user` on the project for `api` only.
8. **Logs:** `google_logging_project_bucket_config` `bucket_id = "mailtinder-logs"`, `location = var.region`, `retention_days = 90`, `locked = var.lock_log_bucket`, `lifecycle { prevent_destroy = true }`. A `google_logging_project_sink` `mailtinder-app` routing `resource.type="cloud_run_revision" OR resource.type="cloud_tasks_queue" OR resource.type="cloud_scheduler_job"` to that bucket, and a `google_logging_project_exclusion` with the same filter so `_Default` does not keep a second copy for 30 days. No application service account gets any `roles/logging.*` role (Cloud Run writes stdout logs itself), so no app identity can delete or edit logs (V16.4.2).
9. **Audit logs:** `google_project_iam_audit_config` for `cloudkms.googleapis.com` and `secretmanager.googleapis.com` with `DATA_READ` and `DATA_WRITE`; `ADMIN_READ` for `allServices`.
10. **Artifact Registry:** Docker repo `mailtinder` in `var.region`. Do not enable `containerscanning.googleapis.com` here (CLAUDE.md keeps container scanning out of the core; see Waits on S11).
11. **Prod root:** `module "foundation" { source = "../../modules/foundation" project_id = var.project_id env = "prod" }`; `backend "gcs" { bucket = "<project>-tfstate" prefix = "prod" }` with the bucket name filled from `terraform.tfvars.example` comments, never a real value committed.
12. **README for James** (bootstrap, one time, in this order): create the project and link billing; `gcloud storage buckets create gs://<project>-tfstate --location=us-central1 --uniform-bucket-level-access --public-access-prevention` and turn on versioning; `terraform init`, `plan`, review, `apply`; then add secret values with `printf '%s' "$VALUE" | gcloud secrets versions add <name> --data-file=-` (HMAC keys from `openssl rand -base64 32`). Note that `locked = true` on the log bucket cannot be undone.
13. **Tests** (`terraform test`, `mock_provider "google"`, `command = plan`): one `run` block per check below.

**Waits on S11** (open points, defaults used):
- Firestore backups and point-in-time recovery: `[DEFAULT]` off (no mail content is stored; S5 data is rebuildable or TTL-bound).
- Organisation Policy constraints and Security Command Center (S4 1): need an organisation; `[DEFAULT]` not in Terraform.
- Container scanning and who reads its findings (S10 12): `[DEFAULT]` not enabled.
- Who may run `apply`, and where state lives: `[DEFAULT]` James only, from his machine, GCS bucket per environment.
- First admin (ASVS V6.3.2, S7 3.7): `[DEFAULT]` not in Terraform yet; the user record ID is created at first sign-in, so S11 must pick the bootstrap (runbook script or an api start-up setting).
- Key rotation runbook and log alert routing: S11 (alerts are T-1107).

## Acceptance criteria

None enforced by `ac-coverage`: Terraform lives outside `backend/` and `app/`. This task provides the `review` evidence for the register rows in the checklist below, backed by the `terraform test` runs.

## Tests that must pass

- `run "kms_has_exactly_three_holders"` (members are exactly `api`, `unsub`, `worker`)
- `run "kms_key_rotates_yearly_in_region"`
- `run "firestore_in_us_central1_native_with_delete_protection"`
- `run "ttl_on_every_ttl_collection"` (five fields)
- `run "jev_and_email_lookup_secrets_have_api_only"`
- `run "no_secret_versions_in_state"` (no `google_secret_manager_secret_version` resources)
- `run "log_bucket_90_days_locked_in_region"`
- `run "no_logging_roles_for_app_accounts"`
- `run "no_service_account_keys"`
- `run "no_primitive_roles"` (no `roles/owner`, `roles/editor`, `roles/viewer` anywhere)
- `run "every_regional_resource_uses_var_region"`
- CI job `terraform` green (`fmt -check`, `validate`, `test`).

## Edge cases and traps

- Use authoritative `_iam_binding` resources for KMS and secret accessors so no extra member can sneak in; never `google_project_iam_member` with KMS roles.
- Never grant KMS, Secret Manager or Firestore roles at a wider scope than the table says; never a primitive role.
- If `apply` rejects `us-central1` for Firestore, stop and ask James; do not fall back to `nam5` silently (S4: location is permanent).
- `locked = true` and `prevent_destroy` are irreversible in effect: staging passes `lock_log_bucket = false` (T-1103).
- Do not add tfsec, checkov or any other infrastructure scanner (CLAUDE.md).
- No real project IDs, emails or secrets in committed files; `terraform.tfvars` is gitignored.
- Do not create Cloud Run services, queues or schedulers here (T-1102b).

## Out of scope

- Cloud Run, Cloud Tasks, Cloud Scheduler, Firebase Hosting and the deploy identity (T-1102b); staging (T-1103); alerts and budgets (T-1107).

## Security review checklist

- KMS key has exactly three encrypt and decrypt holders: `api`, `unsub`, `worker` (S4 2, S6 T1, V13.2.2).
- Secret accessors match step 6 exactly: Jev key and email lookup HMAC for `api` only (V13.3.2).
- No service account keys; no primitive roles; no `roles/iam.serviceAccountUser` granted in this module (V13.2.1, V13.2.3).
- No secret value anywhere in Terraform files or state (V13.3.1).
- Log bucket: 90 days, locked, region `us-central1`, app identities hold no logging role (V16.2.3, V16.4.2, V16.4.3).
- Firestore and every regional resource in `us-central1`; TTL fields match the S5 TTL collections.
- Audit logs on for KMS and Secret Manager data access.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has reviewed `terraform plan` for production and the reviewer has signed the checklist in the PR; the register rows above get a dated `review` sign-off.
