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
   for v in oauth-client-secret jev-api-key email-lookup-hmac-key log-pseudonym-hmac-key; do
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

## Checks

CI runs the `terraform` job: `terraform fmt -check -recursive`, then inside
`modules/foundation` `init -backend=false`, `validate` and `test`, and the same
`init`/`validate` for `envs/prod`. The module's `tests/foundation.tftest.hcl`
uses `mock_provider`, so it needs no credentials and no network.

To run the same checks locally:

```
cd infra/terraform
terraform fmt -check -recursive
for d in modules/foundation envs/prod; do
  (cd "$d" && terraform init -backend=false -input=false && terraform validate)
done
(cd modules/foundation && terraform test)
```