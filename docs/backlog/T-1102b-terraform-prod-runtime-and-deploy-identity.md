# T-1102b: Terraform, production runtime: Cloud Run, Cloud Tasks, Scheduler, Hosting and the deploy identity

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | strong | about 400 lines of HCL plus tests | T-1102a, needs S11 |

**Read only these spec sections:** S4 sections 1 (rows API, Delayed jobs, Job execution, Scheduled sweeps, Static app), 2 (role table) and 3.3 steps 1 to 4 (`docs/specs/S4-architecture.md`); S7 sections 5.12 (API-INT-1 queue settings, API-INT-2 schedule) and 6 (the `max-instances` sentence) (`docs/specs/S7-api-contract.md`); register rows V12.3.3, V13.2.1, V13.2.2 in `docs/security/asvs-l2-register.md`. Nothing else is needed.

## Goal

A `runtime` module that creates the three Cloud Run services with their service accounts from T-1102a, the unsubscribe Cloud Tasks queue, the sweep schedule, the Firebase Hosting site, and a GitHub Actions deploy identity through Workload Identity Federation with only the roles a deploy needs. Images are deployed later by the pipeline (T-1104); Terraform owns every other setting.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `infra/terraform/modules/runtime/variables.tf`, `outputs.tf`, `versions.tf` | Interface below |
| Create | `infra/terraform/modules/runtime/cloud_run.tf` | `api`, `unsub`, `worker` services and invoker bindings |
| Create | `infra/terraform/modules/runtime/tasks.tf` | Queue and queue IAM |
| Create | `infra/terraform/modules/runtime/scheduler.tf` | Sweep job |
| Create | `infra/terraform/modules/runtime/hosting.tf` | Firebase project and Hosting site (`google-beta`) |
| Create | `infra/terraform/modules/runtime/deploy_identity.tf` | Workload identity pool, provider, deployer account and its roles |
| Create | `infra/terraform/modules/runtime/tests/runtime.tftest.hcl` | Plan-time tests |
| Change | `infra/terraform/envs/prod/main.tf` | `module "runtime"` wired to foundation outputs |
| Change | `.github/workflows/ci.yml` | `terraform` job also tests this module |

## Types and signatures

```hcl
variable "project_id"         { type = string }
variable "region"             { type = string default = "us-central1" }
variable "env"                { type = string }
variable "service_accounts"   { type = map(string) }   # from foundation
variable "kms_key_id"         { type = string }
variable "secret_ids"         { type = map(string) }
variable "github_repository"  { type = string default = "difficultnerd/MailTinder" }
variable "deploy_ref"         { type = string default = "refs/heads/main" }
variable "max_instances"      { type = number default = 3 }   # S7 6 [ASSUMES] 3
variable "placeholder_image"  { type = string default = "us-docker.pkg.dev/cloudrun/container/hello" }

output "api_url"              { value = google_cloud_run_v2_service.api.uri }
output "unsub_url"            { value = google_cloud_run_v2_service.unsub.uri }
output "worker_url"           { value = google_cloud_run_v2_service.worker.uri }
output "queue_id"             { value = google_cloud_tasks_queue.unsubscribe.id }
output "hosting_site_id"      { value = google_firebase_hosting_site.app.site_id }
output "wif_provider"         { value = google_iam_workload_identity_pool_provider.github.name }
output "deployer_email"       { value = google_service_account.deployer.email }
```

## Algorithm

1. **Cloud Run** (`google_cloud_run_v2_service`, `location = var.region`), each with its own runtime service account, `max_instance_count = var.max_instances`, min 0, 1 vCPU, 512 MiB `[DEFAULT]`, request timeout `api` 30 s, `unsub` 60 s, `worker` 300 s `[DEFAULT]`, image `var.placeholder_image` with `lifecycle { ignore_changes = [template[0].containers[0].image, client, client_version] }` so the pipeline owns the image only. `deletion_protection = true`.
   - `api`: `ingress = "INGRESS_TRAFFIC_ALL"` (Firebase Hosting rewrites need it, S4 1); `roles/run.invoker` to `allUsers` on `api` only.
   - `unsub`: `ingress = "INGRESS_TRAFFIC_INTERNAL_ONLY"`; invoker `tasks-invoker` only.
   - `worker`: `ingress = "INGRESS_TRAFFIC_INTERNAL_ONLY"`; invoker `scheduler-invoker` only.
   - Non-secret environment: `MT_ENV`, `MT_REGION`, `MT_PROJECT_ID`, `MT_KMS_KEY`, the secret resource names (services read values through Secret Manager at start, T-305), and for `api` also `MT_TASKS_QUEUE`, `MT_UNSUB_URL`, `MT_TASKS_INVOKER_SA`. Use T-305's and T-304's variable names if they differ, and say so in the PR. `CLASSIFIER_GEMINI_ENABLED` and `CLASSIFIER_JEV_ENABLED` start `"false"` `[DEFAULT]`.
   - The OIDC audience for `unsub` and `worker` is each service's URL (S7 5.12).
2. **Queue** `unsubscribe`: `retry_config { max_attempts = 4, min_backoff = "30s", max_backoff = "300s" }` (S7 5.12), `rate_limits { max_dispatches_per_second = 5, max_concurrent_dispatches = 10 }` `[DEFAULT]`. Queue-level IAM: `api` gets `roles/cloudtasks.enqueuer` and `roles/cloudtasks.taskDeleter` on this queue only. `api` gets `roles/iam.serviceAccountUser` on the `tasks-invoker` account only (needed to create tasks with that OIDC identity).
3. **Scheduler** `sweep`: every 15 minutes (`*/15 * * * *`, S7 5.12), `POST ${worker_url}/internal/v1/sweep`, body `{}`, `oidc_token { service_account_email = scheduler-invoker, audience = worker_url }`, `time_zone = "Etc/UTC"`, retry 1.
4. **Hosting:** `google_firebase_project` and `google_firebase_hosting_site` `mailtinder-${var.env}` (provider `google-beta`). Headers and rewrites live in `firebase.json` (T-006) and are deployed by T-1104.
5. **Deploy identity:** pool `github`, OIDC provider with issuer `https://token.actions.githubusercontent.com`, attribute mapping `google.subject = assertion.sub`, `attribute.repository = assertion.repository`, `attribute.ref = assertion.ref`, and `attribute_condition = "assertion.repository == '${var.github_repository}' && assertion.ref == '${var.deploy_ref}'"`. Service account `mt-deployer`; `roles/iam.workloadIdentityUser` on it for `principalSet://.../attribute.repository/${var.github_repository}`. Deployer roles:
   - `roles/run.developer` on each of the three services (resource level).
   - `roles/iam.serviceAccountUser` on the three runtime accounts only.
   - `roles/artifactregistry.writer` on the `mailtinder` repository only.
   - `roles/firebasehosting.admin` on the project (no narrower role exists).
   Nothing else: no KMS, Secret Manager, Firestore, IAM admin or primitive role.
6. **Prod root:** `module "runtime"` with `env = "prod"`; production deploys need a GitHub environment approval (T-1104), so set `deploy_ref = "refs/heads/main"` and leave approval to GitHub.
7. **Tests** (`mock_provider`, `command = plan`): checks below.

**Waits on S11** (defaults used): instance sizes and timeouts; queue dispatch rates; whether `api` also gets a custom domain; the production approval rule (`[DEFAULT]` GitHub environment `production` with James as required reviewer, T-1104); Cloud Armor and a load balancer (S4 open decision 2, not built).

## Acceptance criteria

None enforced by `ac-coverage` (Terraform). This task provides `review` evidence for V12.3.3, V13.2.1 and V13.2.2 through the checklist and tests below.

## Tests that must pass

- `run "only_api_is_public"` (`allUsers` invoker on `api` only; `unsub` and `worker` internal ingress)
- `run "unsub_invoker_is_tasks_invoker_only"`
- `run "worker_invoker_is_scheduler_invoker_only"`
- `run "queue_retry_matches_s7"` (4 attempts, 30 s, 300 s)
- `run "api_can_enqueue_and_delete_on_one_queue_only"`
- `run "sweep_every_15_minutes_with_oidc"`
- `run "deployer_has_no_kms_secret_or_firestore_role"`
- `run "wif_condition_pins_repository_and_ref"`
- `run "max_instances_three"`
- `run "image_ignored_by_terraform"`

## Edge cases and traps

- `actAs` (`roles/iam.serviceAccountUser`) is granted on single service accounts, never on the project.
- Do not mount secrets as environment values; pass names only, the services read them at start.
- The `api` service is public on purpose; `unsub` and `worker` must stay internal, or Cloud Tasks and Scheduler auth alone protects them.
- Without `ignore_changes` on the image, every `apply` would roll back the last deploy.
- Never add a service account key for GitHub; Workload Identity Federation only.
- Do not add an infrastructure scanner (CLAUDE.md).

## Out of scope

- The deploy workflow itself (T-1104); staging (T-1103); alerts (T-1107).

## Security review checklist

- Only `api` has `allUsers` invoker; `unsub` and `worker` have internal ingress and one invoker each (V12.3.3, V13.2.1).
- `api` has `actAs` on `tasks-invoker` only; Cloud Tasks and Scheduler calls carry OIDC tokens with the service URL as audience.
- The deployer can deploy images and Hosting only: no KMS, Secret Manager, Firestore, IAM admin or primitive role (V13.2.2).
- The workload identity condition pins the repository and branch; no service account keys exist.
- Every resource is in `us-central1`.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has reviewed the production `terraform plan`; the reviewer has signed the checklist.
