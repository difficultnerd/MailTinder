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