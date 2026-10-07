# Production root. Staging is a separate root built from the same module
# (T-1103); production and staging are separate projects, each with its own
# state (S4 1 Environments).
module "foundation" {
  source = "../../modules/foundation"

  project_id      = var.project_id
  region          = var.region
  env             = "prod"
  lock_log_bucket = var.lock_log_bucket
}

# The runtime: Cloud Run, Cloud Tasks, Scheduler, Hosting and the deploy
# identity (T-1102b). Only production deploys from main. A production deploy
# additionally needs the GitHub `production` environment's required reviewer:
# the WIF provider's condition pins repository, ref AND the `environment`
# claim, so a token whose job did not run under the `production` environment
# cannot impersonate the deployer. GitHub only puts that claim in the token
# once the environment's required reviewer approves, so the approval is the
# GitHub-side control the condition depends on - the condition alone does not
# enforce it.
module "runtime" {
  source = "../../modules/runtime"

  project_id             = var.project_id
  region                 = var.region
  env                    = "prod"
  service_accounts       = module.foundation.service_accounts
  kms_key_id             = module.foundation.kms_key_id
  secret_ids             = module.foundation.secret_ids
  google_oauth_client_id = var.google_oauth_client_id
  deploy_ref             = "refs/heads/main"
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
  description = "Locked 90-day log bucket."
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

output "wif_provider" {
  description = "Workload Identity Federation provider GitHub Actions authenticates against."
  value       = module.runtime.wif_provider
}

output "deployer_email" {
  description = "Deploy service account the pipeline impersonates."
  value       = module.runtime.deployer_email
}