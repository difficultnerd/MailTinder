# Staging root: the same two modules as production, in its own project with its
# own state, so deploys, the smoke tests (T-1105) and the ZAP baseline run
# somewhere that is not production (S4 1 Environments, decision T5, T-1103).
#
# Staging may differ from ../../prod only in the values passed to the modules:
# `env`, `project_id`, `lock_log_bucket`, `max_instances` and
# `deploy_environment`. Every module call is identical, and
# scripts/tf_env_parity.sh fails the build if the two roots ever call a
# different module.
module "foundation" {
  source = "../../modules/foundation"

  project_id = var.project_id
  region     = var.region
  env        = "staging"

  # [DEFAULT] staging can be torn down; production stays locked. Locking the
  # 90-day bucket cannot be undone, so this is the one staging-only setting here
  # (T-1103 edge case). Retention and location are unchanged.
  lock_log_bucket = false
}

# The runtime: Cloud Run, Cloud Tasks, Scheduler, Hosting and the deploy
# identity (T-1102b). Staging deploys from main as production does, so the ref
# pin is the same; only the GitHub environment differs. The WIF provider's
# condition pins the `environment` claim, so the staging deploy job must name
# `environment: staging` (T-1104): without that value here, or without the job
# naming the environment, every staging token is rejected.
module "runtime" {
  source = "../../modules/runtime"

  project_id             = var.project_id
  region                 = var.region
  env                    = "staging"
  service_accounts       = module.foundation.service_accounts
  kms_key_id             = module.foundation.kms_key_id
  secret_ids             = module.foundation.secret_ids
  google_oauth_client_id = var.google_oauth_client_id

  deploy_ref = "refs/heads/main"

  # NOT the production default: the WIF condition requires the staging claim
  # (T-1103, T-1104).
  deploy_environment = "staging"

  # [DEFAULT] staging traffic is tiny.
  max_instances = 1
}

output "service_accounts" {
  description = "Service account emails by short name (used by T-1102b)."
  value       = module.foundation.service_accounts
}

output "kms_key_id" {
  description = "Key that wraps every user's data_key."
  value       = module.foundation.kms_key_id
}

output "kms_system_key_id" {
  description = "Key for data that exists before a user."
  value       = module.foundation.kms_system_key_id
}

output "secret_ids" {
  description = "Secret Manager containers (values are added by James, see ../../README.md)."
  value       = module.foundation.secret_ids
}

output "artifact_repo" {
  description = "Docker repository address for Cloud Run deploys."
  value       = module.foundation.artifact_repo
}

output "log_bucket_id" {
  description = "90-day log bucket. Unlocked in staging (`lock_log_bucket = false`)."
  value       = module.foundation.log_bucket_id
}

output "api_url" {
  description = "Cloud Run URL of the public API service."
  value       = module.runtime.api_url
}

output "queue_id" {
  description = "Cloud Tasks queue the API enqueues unsubscribe jobs on."
  value       = module.runtime.queue_id
}

output "hosting_site_id" {
  description = "Firebase Hosting site id. The staging URL recorded as the STAGING_URL repository variable (T-1104, T-1105) is https://<site_id>.web.app."
  value       = module.runtime.hosting_site_id
}

output "wif_provider" {
  description = "Workload Identity Federation provider GitHub Actions authenticates against."
  value       = module.runtime.wif_provider
}

output "deployer_email" {
  description = "Deploy service account the pipeline impersonates."
  value       = module.runtime.deployer_email
}
