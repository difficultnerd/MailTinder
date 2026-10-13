# Terraform: MailTinder infrastructure

Two things live here:

- `modules/foundation` - the reusable module: project APIs, one service account
  per service, the KMS keys, Firestore with its TTL policies, Secret Manager
  containers with their accessors, the locked 90-day log bucket, data access
  audit logs and the Artifact Registry repository (T-1102a).
- `envs/prod` - the production root that calls the module.
- `envs/staging` - the staging root (T-1103): **the same two modules and the
  same module sources**, in its own project with its own state, so deploys, the
  smoke tests (T-1105) and the ZAP baseline run somewhere that is not
  production (decision T5). The roots may differ only in the values they pass
  (`env`, `project_id`, `lock_log_bucket`, `max_instances`,
  `deploy_environment`); `scripts/tf_env_parity.sh` fails the `terraform` CI
  job if they ever call a different module.

Nothing here holds a secret value. Agents write the Terraform; **James reviews
the plan and runs `apply`**, from his own machine, one project at a time
(S13 1, S4 1 Environments).

## Bootstrap (one time, per environment, order matters)

1. Create the Google Cloud project and link billing to it, and create one
   **identity project** for that environment too, with its billing link. Neither
   project is scripted: linking billing needs a billing admin, so the bootstrap
   script stops and prints the console link when a project has no billing
   account. The identity project holds the Workload Identity pools, their
   providers and the two service accounts; the applier has no role in it (see
   step 2), which is what makes the bootstrap identities unreachable from an
   apply job.

2. Run the one-command bootstrap (T-1116), as **your own user account** - it
   refuses to run as a service account, refuses a project without billing,
   refuses one project for both roles, and refuses production without
   `--i-know-the-log-bucket-lock-is-permanent`:

   ```
   scripts/infra/bootstrap-project.sh --project <project> \
     --identity-project <identity-project> --env staging \
     --repo <owner/name>
   ```

   It prints the plan, asks for `y` (`--yes` skips the prompt on a re-run;
   `--dry-run` prints the plan and stops, changing nothing), and then creates
   what Terraform cannot create for itself:

   - in the workload project, the six APIs Terraform needs to start
     (`serviceusage`, `cloudresourcemanager`, `iam`, `iamcredentials`, `sts`,
     `storage`; `modules/foundation/apis.tf` enables the rest) and the state
     bucket `gs://<project>-tfstate`: `us-central1`, uniform bucket-level
     access, public access prevention, versioning on.
   - in the identity project, the five APIs the identities need, two Workload
     Identity pools, `github-plan` and `github-apply`, one provider each (a
     principalSet is scoped to a pool, so one pool with two providers could not
     keep the planner away from the applier), and the `mt-planner` and
     `mt-applier` service accounts with their impersonation bindings.
     `mt-planner` is bound to the plan pool's `attribute.repository` and gets
     narrow per-service viewers plus `objectViewer` on the state bucket (it
     plans with `-lock=false`); `mt-applier` is bound to the apply pool's
     `attribute.environment` only and gets the roles the two modules need,
     `objectAdmin` on the state bucket, and **no role of any kind in the
     identity project** - so it cannot rewrite the impersonation policy that
     admits it, cannot disable or delete either identity, and cannot edit an
     admission rule. Its `roles/resourcemanager.projectIamAdmin` grant carries
     an IAM Condition that limits the roles it may grant **at project scope** to
     exactly the three roles the modules bind with a `google_project_iam_member`
     (`roles/datastore.user`, `roles/aiplatform.user`,
     `roles/firebasehosting.admin`). Every other role the modules grant is bound
     on one resource and is deliberately **not** in that list - the API
     attribute restricts role *names*, not the resource a role is granted on, so
     listing one would widen the grant from a single crypto key, secret, service,
     repository or queue to the whole project. The applier binds those
     per-resource roles with the resource-level roles it holds
     (`roles/run.admin` for `roles/run.invoker`, `roles/cloudkms.admin` for the
     KMS encrypt/decrypt role, and so on, mapped in the script header), and
     `roles/iam.workloadIdentityUser` and `roles/iam.serviceAccountUser` are
     excluded too (they are bound only on the service accounts that need them).
     Both conditions are written with `--condition-from-file`. Its other grants
     are custom roles with an explicit permission list, in the script header,
     mapped to the resource that needs each one.
   - nothing before the grants are verified: each pool must hold exactly its one
     provider with the intended issuer, mapping and `attribute_condition`, each
     bootstrap identity must carry nothing but its own impersonation binding,
     each custom role must hold exactly the permissions the script intends, and
     the state bucket must have uniform access, public access prevention and
     versioning on. A mismatch stops the run and grants nothing.
   - no service account key, no secret value, no Terraform state: the script
     never touches any of the three. The applier's secret role has no
     `secretmanager.versions.*` permission at all, so it cannot read a value
     directly; because it does hold the accessor-binding permission on the
     containers the module manages, it can reach a value transitively - the
     control there is the environment approval, and the script says so rather
     than claiming otherwise.
   - a second run creates nothing (every creating call sits behind a `describe`
     probe and says so), and a run after a partial failure resumes where it
     stopped.

   It prints, last, the exact values to store as GitHub variables: repository
   variables with an `<ENV>` suffix for the post-merge plan job (a job that names
   no environment cannot read an environment-scoped variable) and Environment
   variables in `staging`/`production` for the apply job, plus the
   `TF_APPLY_ENABLED=false` reminder (ADR 0003 item 7). The project id and the
   bucket name are arguments, never committed - this repository is public.

3. From `envs/prod`, point the backend at that bucket and initialise:

   ```
   terraform init -backend-config="bucket=<project>-tfstate"
   ```

   (or edit the placeholder in `backend.tf`; the bucket name is never committed)

4. Write `terraform.tfvars` (never committed) with `google_oauth_client_id`,
   `billing_account` and `alert_email`. The monitoring module (T-1107) needs the
   billing account for its two budgets and James's address for the one email
   notification channel that every budget and alert policy notifies; the email
   is personal data, so it lives only in `terraform.tfvars`. Then `terraform
   plan`. Read it. Then `terraform apply`, and confirm the test notification
   arrives from the channel.

   That first apply also creates the three runtime identities `mt-api`,
   `mt-unsub` and `mt-worker`, and the three Cloud Run services run as them.
   Creating or updating a service that runs as another identity needs
   `roles/iam.serviceAccountUser` (`actAs`) **on that account** - never on the
   project - and the accounts do not exist until this apply has run, so it is a
   post-first-apply step: run

   ```
   scripts/infra/bootstrap-project.sh --project <project> \
     --identity-project <identity-project> --env <env> \
     --repo <owner/name> --yes
   ```

   again (it is idempotent and changes nothing else; production additionally
   needs `--i-know-the-log-bucket-lock-is-permanent`). It now finds the three
   accounts, binds `roles/iam.serviceAccountUser` for the applier on each of
   them, and reads each binding back; the first run printed the same three
   `gcloud iam service-accounts add-iam-policy-binding` lines if you prefer to
   run them by hand. Until that step is done the pipeline can plan a change to a
   Cloud Run service but not apply it. Nothing about this step is project-wide:
   the applier holds no `actAs` at project scope, in either project.

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

### Staging (T-1103)

Staging is a **second project** (`<name>-staging`, chosen by James) with its own
billing link and **its own state bucket**. It runs the same three modules from
this repository as production; only the variables differ. Staging passes
`enable_alerts = false` to the monitoring module, so it gets the budgets and the
email channel but no alert policy, no log-based metric and no uptime check
(T-1107). Bootstrap it by repeating steps 1 and 2 above with the staging names -
the same one command, with staging's own project id, `--env staging` and
staging's own `billing_account` and `alert_email` in `terraform.tfvars`:

1. Create the staging project and its staging identity project, and link billing
   to both.
2. `scripts/infra/bootstrap-project.sh --project <staging-project>
   --identity-project <staging-identity-project> --env staging
   --repo <owner/name>`. It creates `gs://<staging-project>-tfstate`, staging's
   own identity pools in the staging identity project, and its own
   `mt-planner`/`mt-applier`. Never reuse the production bucket or the
   production identity project (S4 1: separate state, separate identities), so a
   staging destroy can never reach production state or production credentials.
3. From `envs/staging`:

   ```
   terraform init -backend-config="bucket=<staging-project>-tfstate"
   terraform plan
   terraform apply
   ```

   Then repeat the post-first-apply `actAs` step from Bootstrap 4: run the
   bootstrap command from step 2 again, now that staging's own `mt-api`,
   `mt-unsub` and `mt-worker` exist, so the applier can manage staging's Cloud
   Run services.

4. Add the staging secret values (step 5 above, with staging's own values:
   fresh `openssl rand -base64 32` HMAC keys). Never copy a production secret
   value into staging - that would put a production credential in the project
   with the weaker controls.

Staging's OAuth client is its own, created **in the staging project** in Testing
mode, and its secret goes into the staging `google-oauth-client-secret`. The
staging OAuth consent screen stays in Testing mode with James's test users only,
and staging holds synthetic data only: no real person's mailbox is linked there.

The 90-day log bucket is deliberately unlocked in staging
(`lock_log_bucket = false`) so the project can be torn down; production keeps the
locked default. Once staging is applied, record its Hosting URL
(`https://<hosting_site_id>.web.app`, the root's `hosting_site_id` output) as the
`STAGING_URL` repository variable for T-1104 and T-1105.

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
- The three Cloud Run services run as `mt-api`, `mt-unsub` and `mt-worker`, so
  the applier needs `roles/iam.serviceAccountUser` on each of them. That role is
  bound **on the service account**, never on the project, and the accounts only
  exist after the first apply - see the post-first-apply step in Bootstrap 4.
  A pipeline `apply` that changes a Cloud Run service before that step fails on
  `actAs`, and the script prints the exact commands when it is run before the
  accounts exist.
- Container scanning is not enabled here (CLAUDE.md keeps it out of the core;
  see S10 12).

## Monitoring: the Vertex AI budget filter, and follow-ups (T-1107)

- The Vertex AI budget filters on the **Cloud Billing catalogue service id, not
  the API hostname**: `var.vertex_billing_service` defaults to
  `services/C7E2-9256-1C43` (Vertex AI, now billed as Gemini Enterprise Agent
  Platform), taken from Google's "Services that are eligible for spend cap
  budgets" table
  (<https://cloud.google.com/billing/docs/how-to/budgets-spend-caps>). A filter
  of `services/aiplatform.googleapis.com` is not a catalogue entry and would
  match no cost. **Verify on first apply** (review F4): this is the one value
  here that cannot be checked offline - the budget API rejects an unknown
  service id and the Cloud Billing Catalog API
  (`GET https://cloudbilling.googleapis.com/v1/services`) lists the current ids
  with their display names. Override `vertex_billing_service` in
  `terraform.tfvars` if the catalogue has moved; the module rejects an API
  hostname in that variable.
- `firebase.json` names the Hosting `/api/**` rewrite target
  `serviceId: "mt-api"`, which is the Cloud Run service the runtime module
  creates. T-1107 left it as `"api"` (review F5); T-1104b aligned the rewrite
  with the Terraform name and with the `gcloud run deploy "mt-${svc}"` target in
  `deploy.yml`.
- The `terraform` CI job runs `init`/`validate`/`test` for `modules/foundation`
  and `modules/runtime`, and `init`/`validate` for both roots, but **not**
  `modules/monitoring`. Its `tests/monitoring.tftest.hcl` suite therefore does
  not run in CI yet (review F2 - `.github/workflows/ci.yml` is owner-only).
  Run it locally as in Checks above:
  `(cd modules/monitoring && terraform init -backend=false && terraform test)`.
- Deferred, not fixed by this task (Low/Info): review findings F6-F10.

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

## Deploying and rolling back (T-1104)

`.github/workflows/deploy.yml` builds the three service images once for every
commit on `main`, deploys them and the web app to staging, and then promotes the
same image digests to production after the `production` environment's reviewer
approves. Nothing is rebuilt for production: the promote job copies the manifest
digests staging ran into the production repository (`crane`, pinned to one
release and checksum checked), so the bytes staging ran are the bytes that ship.

T-1105 owns the staging smoke tests and adds its reusable workflow to the job
slot in `deploy.yml` marked `# T-1105 adds the smoke job here`; it depends on
this task, so it is not wired up yet. **Until T-1105 lands that smoke job,
`deploy-prod` depends on `deploy-staging` alone and the `production`
environment's required reviewer (James) is the only gate between a staging
deploy and production** - nothing automatically checks the staging deployment
before the promotion is approved.

The workflow starts only for a push to `main` that can change a deployed
artifact (`on.push.paths`: `backend/**`, `app/**`, `firebase.json`,
`.firebaserc` and the workflow itself - it deploys with `--image` only and never
applies Terraform, so nothing under `infra/` is deployable here), and its first
job `configured` ends the run GREEN-SKIPPED with one warning until the staging
variables below are set instead of failing on every push; a manual
`workflow_dispatch` is never filtered and always fails while they are unset
(T-1104b).

Images are tagged with the commit sha and pushed to
`us-central1-docker.pkg.dev/<project>/mailtinder/<service>` (T-1102a). Keeping
every image is the default while S11 is open: nothing prunes Artifact Registry.
Cloud Run is deployed with `--image` only, so Terraform keeps owning the
environment variables, the service accounts and the scaling.

GitHub repository variables the workflow reads, all set by James:

**The `configured` job names no environment on purpose (it holds no WIF token
there), and a job without an environment cannot see an environment-scoped
variable - so all of these have to be *repository* variables, not variables
scoped to the `staging` or `production` environment.**

| Variable | What it is |
| --- | --- |
| `STAGING_PROJECT` | the staging project id (T-1103) |
| `STAGING_WIF_PROVIDER` | the staging Workload Identity provider resource name |
| `STAGING_DEPLOYER` | the staging `mt-deployer` service account email |
| `PROD_PROJECT` | the production project id (T-1102b) |
| `PROD_WIF_PROVIDER` | the production Workload Identity provider resource name |
| `PROD_DEPLOYER` | the production `mt-deployer` service account email |
| `DEPLOY_ENABLED` | set to `true` after the first successful staging deploy (T-1104b): a missing staging variable then fails the run loudly, naming it, instead of skipping the deploy quietly |
| `STAGING_URL` | the staging Hosting URL (T-1105 reads it) |

GitHub environments, and why each deploy job must name one:

- `staging` - no required reviewer. Its WIF provider pins
  `assertion.environment == 'staging'`.
- `production` - James as required reviewer. Its WIF provider pins
  `assertion.environment == 'production'`, so GitHub withholds that claim, and
  therefore the token, until he approves.

One binding the promote job needs is not in Terraform yet: the production
`mt-deployer` has to read the staging `mailtinder` repository, because the
digest copy crosses projects. Add it to the staging root (T-1103):

```
gcloud artifacts repositories add-iam-policy-binding mailtinder \
  --location=us-central1 --project=<staging-project> \
  --member=serviceAccount:mt-deployer@<prod-project>.iam.gserviceaccount.com \
  --role=roles/artifactregistry.reader
```

**needs owner (ci.yml).** The grant belongs in the staging root as Terraform
(`google_artifact_registry_repository_iam_member`, `role =
"roles/artifactregistry.reader"`, member the production deployer passed in as a
variable - no project id committed). It is not there yet because the role
allowlist in `.github/workflows/ci.yml` does not list
`roles/artifactregistry.reader`, and that workflow is owner-only: the new role
has to be allowlisted there first, or the `ci-integrity` job fails and the pull
request cannot merge. The manual command above stays the interim step until the
owner makes the Terraform and allowlist change together.

Container scanning stays off in CI (CLAUDE.md); if Artifact Registry scanning
is ever enabled it runs in Google Cloud, and nothing in this pipeline reads its
findings. There is no smoke test yet (T-1105), so nothing automatically stops a
bad staging deployment before the promotion: the `production` environment's
required reviewer is the only gate between staging and production.

### Rolling back

There is no rollback automation, by design: the previous revision is already
there, so a rollback is a traffic move rather than a redeploy.

```
gcloud run services update-traffic <svc> --to-revisions=<previous>=100 --region=us-central1
firebase hosting:rollback --project <prod-project>
```

Cloud Run keeps the earlier revisions, and Hosting keeps the earlier release,
so both are minutes. The image for a known-good revision stays in Artifact
Registry under its commit sha, so `gcloud run deploy --image ...@<digest>` can
also put a specific digest back.

## Checks

CI runs the `terraform` job: `terraform fmt -check -recursive`, then inside
`modules/foundation` and `modules/runtime` `init -backend=false`, `validate` and
`test`, and the same `init`/`validate` for `envs/prod` and `envs/staging`. It
then runs `scripts/tf_env_parity.sh`, which fails unless both roots have the
same sorted `source = "..."` lines - so a staging-only fork of a module, or a
root that stopped calling one, fails the job. Both modules'
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
for d in modules/foundation modules/runtime envs/prod envs/staging; do
  (cd "$d" && terraform init -backend=false -input=false && terraform validate)
done
(cd modules/foundation && terraform test)
(cd modules/runtime && terraform test)
cd .. && ./scripts/tf_env_parity.sh
```

The job's guard step (the role allowlist, the tuple pins and the fixtures
self-test) is plain shell and awk; run the `Absence checks` step's `run:` body
from `.github/workflows/ci.yml` with `infra/terraform` as the working directory
to exercise it, including the `tests/guards/negative/` self-tests.