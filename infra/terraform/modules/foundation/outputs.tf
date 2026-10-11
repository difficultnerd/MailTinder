output "service_accounts" {
  description = "Service account emails by short name."
  value = {
    api               = google_service_account.service["api"].email
    unsub             = google_service_account.service["unsub"].email
    worker            = google_service_account.service["worker"].email
    tasks_invoker     = google_service_account.service["tasks-invoker"].email
    scheduler_invoker = google_service_account.service["scheduler-invoker"].email
  }
}

output "kms_key_id" {
  description = "Key id of `data-key-kek`, the key that wraps every user's data_key."
  value       = google_kms_crypto_key.data_key_kek.id
}

output "kms_system_key_id" {
  description = "Key id of `system-fields`."
  value       = google_kms_crypto_key.system_fields.id
}

output "secret_ids" {
  description = "Secret Manager container ids (containers only; values are added by James)."
  value = {
    oauth_client_secret = google_secret_manager_secret.secret["google-oauth-client-secret"].secret_id
    jev_api_key         = google_secret_manager_secret.secret["jev-api-key"].secret_id
    email_lookup_hmac   = google_secret_manager_secret.secret["email-lookup-hmac-key"].secret_id
    log_pseudonym_hmac  = google_secret_manager_secret.secret["log-pseudonym-hmac-key"].secret_id
  }
}

output "artifact_repo" {
  description = "Docker repository address for Cloud Run deploys."
  value       = "${var.region}-docker.pkg.dev/${var.project_id}/mailtinder"
}

output "log_bucket_id" {
  description = "Id of the locked 90-day log bucket."
  value       = google_logging_project_bucket_config.app.id
}