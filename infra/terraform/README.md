# Terraform: MailTinder infrastructure

Two things live here:

- `modules/foundation` - the reusable module: project APIs, one service account
  per service, the KMS keys, Firestore with its TTL policies, Secret Manager
  containers with their accessors, the locked 90-day log bucket, data access
  audit logs and the Artifact Registry repository (T-1102a).
- `envs/prod` - the production root that calls the module. `envs/staging` is
  T-1103.

Nothing here holds a secret value. Agents write the Terraform; **James reviews
the plan and runs `apply`**, from his own machine, one project at a time
(S13 1, S4 1 Environments).

## Bootstrap (one time, per environment, order matters)

1. Create the Google Cloud project and link billing to it.
2. Create the remote state bucket. `us-central1` keeps the state in the same
   region as everything else:

   ```
   gcloud storage buckets create gs://<project>-tfstate \
     --location=us-central1 --uniform-bucket-level-access \
     --public-access-prevention
   gcloud storage buckets update gs://<project>-tfstate --versioning
   ```

3. From `envs/prod`, point the backend at that bucket and initialise:

   ```
   terraform init -backend-config="bucket=<project>-tfstate"
   ```

   (or edit the placeholder in `backend.tf`; the bucket name is never committed)

4. `terraform plan`. Read it. Then `terraform apply`.
5. Add the secret values. Terraform creates the four containers and stops
   there, so no value ever enters state:

   ```
   for v in google-oauth-client-secret jev-api-key email-lookup-hmac-key log-pseudonym-hmac-key; do
     printf '%s' "$VALUE" | gcloud secrets versions add "$v" --data-file=-
   done
   ```

   The two HMAC keys are 32 random bytes, base64 encoded:

   ```
   openssl rand -base64 32
   ```

   The OAuth client secret comes from the Google Cloud console, the Jev API
   key from TypeSafe.

## Cautions

- `locked = true` on the log bucket **cannot be undone**. Production always
  passes it; staging passes `lock_log_bucket = false` (T-1103).
- `prevent_destroy` is set on the log bucket and both KMS keys. Removing either
  from the configuration needs a deliberate state operation, which is the point.
- The Firestore location is permanent once the database is created. If `apply`
  rejects `us-central1`, stop and ask - do not fall back to another multi-region
  silently (S4 1).
- KMS and secret accessor bindings are authoritative for their role on their
  resource. Adding a member by hand outside Terraform will be reverted by the
  next `apply` - that is deliberate (S6 5, S13 `Edge cases and traps`).
- No service account keys, ever. Every service authenticates with its own
  Google identity and short-lived OIDC tokens.
- Container scanning is not enabled here (CLAUDE.md keeps it out of the core;
  see S10 12).

## Decision (owner, 10 Oct): deletion semantics

The owner's decision is **immediate crypto-shredding: no point-in-time recovery
and no backup schedule for the trial** (`firestore.tf`, S6 5, DEL-2). Destroying
a user's `data_key` is what makes their Firestore ciphertext unreadable, so no
recoverable copy should outlive the deletion by design.

What crypto-shredding does and does not promise, stated plainly:

- It destroys the per-user `data_key`. It is not instantly irrecoverable
  against a historical read taken just before the deletion: Firestore still
  serves reads of a document's *historical* versions for about an hour even with
  PITR disabled, and Cloud KMS keeps earlier versions of the shared KEK usable,
  so a wrapped-key copy read within that window can still be unwrapped.
- So the honest boundary is the KMS key version plus Firestore's
  historical-read window, not the deletion of a single document: the per-user
  key is destroyed and the old readable copies age out within about an hour.

The residual ~1 hour historical-read window is accepted as this trial's cost of
an immediate, PITR-free erasure; Terraform behaviour (recovery disabled, no
backup schedule) is unchanged.

## Deploy identity and its real blast radius (T-1102b)

`mt-deployer` is the only credential GitHub Actions uses; there is no service
account key. The Workload Identity provider admits a token only from
`difficultnerd/MailTinder` on `refs/heads/main` **and** from the GitHub
environment `production`, so a workflow outside that environment cannot
impersonate the deployer at all.

What it holds, directly and transitively:

- **Directly**, the deployer holds no KMS, Secret Manager, Firestore, IAM-admin
  or primitive role (V13.2.2). Every role it is granted is resource-scoped:
  `roles/run.developer` on the three Cloud Run services,
  `roles/iam.serviceAccountUser` (`actAs`) on the three runtime accounts,
  `roles/artifactregistry.writer` on the one repository, and
  `roles/firebasehosting.admin` on the project.
- **Transitively**, that is not the whole story and the checklist must not claim
  it is. `roles/run.developer` plus `actAs` on `mt-api`/`mt-unsub`/`mt-worker`
  lets the deployer deploy an *arbitrary* image that then runs as one of those
  runtime identities, and those identities **do** hold KMS and Secret Manager
  access (S4 2). The deployer reaches KMS and secrets through the image it
  deploys, even though it holds no KMS or secret role itself. The same applies
  to Firestore and to anything else a runtime account can reach (V13.2.2 is
  satisfied for direct grants only - say so).
- The **compensating control** is the required reviewer configured on the
  GitHub `production` environment (James). This is a GitHub-side control: the
  WIF condition requires the token to carry `assertion.environment ==
  'production'`, which GitHub only issues to a job that names `environment:
  production` - and it withholds that claim until the environment's reviewer
  approves. So GCP enforces the claim, and the reviewer is what makes the claim
  meaningful; the two together mean a deploy cannot impersonate the deployer
  without the approval gate. Branch protection on `main` is the second layer
  (S11 3, T-1104). Approving a production deploy is a decision to run a
  specific image as those identities - treat it as one.

## Checks

CI runs the `terraform` job: `terraform fmt -check -recursive`, then inside
`modules/foundation` and `modules/runtime` `init -backend=false`, `validate` and
`test`, and the same `init`/`validate` for `envs/prod`. Both modules'
`tests/*.tftest.hcl` use `mock_provider`, so they need no credentials and no
network.

The module tests inspect the resources the module declares; they cannot prove
the absence of a resource type, or see a grant a later change adds elsewhere.
That property is enforced by the job's final step, which scans every `.tf` file
under `modules/` and `envs/`, and then runs each guard again against a negative
fixture under `tests/guards/negative/` to prove it still fires (a self-test): a
guard edited so it no longer rejects its fixture fails the job.

- a resource-type guard rejects `google_service_account_key`,
  `google_secret_manager_secret_version`, `google_firestore_backup_schedule` and
  `google_project_iam_policy`;
- a role allowlist rejects any granted role that is not one this work
  authorises: `roles/cloudkms.cryptoKeyEncrypterDecrypter`,
  `roles/aiplatform.user`, `roles/datastore.user`,
  `roles/secretmanager.secretAccessor`, `roles/run.invoker`,
  `roles/run.developer`, `roles/cloudtasks.enqueuer`,
  `roles/cloudtasks.taskDeleter`, `roles/artifactregistry.writer`,
  `roles/firebasehosting.admin`, `roles/iam.workloadIdentityUser` and
  `roles/iam.serviceAccountUser`. It reads both the scalar
  `role = "roles/..."` form (every `google_*_iam_member`, `_binding` and
  `_policy` resource) and the `roles = ["roles/..."]` list form, and **fails
  closed on any role value that is not a plain quoted literal** (`role = var.x`,
  `role = local.x`) **or that hides an interpolation or a conditional
  expression inside a quoted value** (`role = "roles/${...}"`,
  `role = "roles/datastore.user" == "" ? var.x : var.x`), so a role cannot be
  granted through a variable, a local, an interpolation or a quoted prefix and
  stay unseen. This is what covers "no primitive role" and "no logging role for
  an app identity". (The same-project log-bucket sinks need no writer identity,
  so `roles/logging.bucketWriter` is no longer granted or allowed - review D1.)
- a role-name allowlist is not a *scope* allowlist, so two further checks pin
  the (role, principal, resource) tuple: every role granted on a
  `google_project_iam_*` resource must be one of the three project-scope roles
  this work uses (`roles/datastore.user`, `roles/aiplatform.user`,
  `roles/firebasehosting.admin`), so an otherwise-allowed role cannot be widened
  from a resource to the whole project; and every binding that grants
  `roles/run.invoker` (or the deployer's `roles/run.developer`) is pinned by its
  **type, module file, target service, role and principal** - not just its
  label - in *any* Cloud Run IAM resource family, v1
  (`google_cloud_run_service_iam_*`) as well as v2
  (`google_cloud_run_v2_service_iam_*`), and `_binding`/`_policy` as well as
  `_member`. So a second `api_public` elsewhere, a v1 binding on a v2-created
  service, a binding re-pointed at an internal service, a widened role or a
  swapped principal all fail, and no extra invoker can reach an internal service
  (V12.3.3, V13.2.2).
- two narrower checks back the allowlist up: `roles/iam.serviceAccountTokenCreator`
  is never allowed, and `roles/iam.serviceAccountUser` (`actAs`) is allowed only
  on an individual service account, never on the project. Only `api_public` may
  name `allUsers`/`allAuthenticatedUsers` (V13.2.1).

Note what the log bucket holds. The `mailtinder-app` sink selects Cloud Run
revision entries: the application's own stdout/stderr and the Cloud Run system
entries that share that resource type, not stdout/stderr alone (review F7). The
Cloud Run platform request log (`run.googleapis.com%2Frequests`) records the
request URL, its query string and the client IP, so the sink excludes it and
`_Default` drops it - those entries are never captured in the locked 90-day
store (S5 bans URLs and addresses in logs). Neither sink carries Cloud Tasks or
Cloud Scheduler platform logs, which stay outside these filters. Both sinks
target a log bucket in the same project, so neither has a writer identity and
neither needs a `roles/logging.bucketWriter` grant (review D1).

KMS and Secret Manager Data Access audit logs go to the same locked bucket
through a second sink (`mailtinder-audit`), so the evidence the monthly
elevation review and the leak investigation read is not confined to `_Default`'s
30-day, unlocked window. That sink is additive: the `_Default` copy and the
immutable `_Required` copies (Admin Activity and System Event records, retained
for 400 days) remain; the sink only adds the locked, unremovable copy
(review F8).

To run the same checks locally:

```
cd infra/terraform
terraform fmt -check -recursive
for d in modules/foundation modules/runtime envs/prod; do
  (cd "$d" && terraform init -backend=false -input=false && terraform validate)
done
(cd modules/foundation && terraform test)
(cd modules/runtime && terraform test)
```

The job's guard step (the role allowlist, the tuple pins and the fixtures
self-test) is plain shell and awk; run the `Absence checks` step's `run:` body
from `.github/workflows/ci.yml` with `infra/terraform` as the working directory
to exercise it, including the `tests/guards/negative/` self-tests.