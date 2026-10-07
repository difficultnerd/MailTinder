# Logs (S5 "Logs and telemetry", S6 7, S4 1 observability).
#
# Application logs go to one locked 90-day bucket in var.region. No application
# service account is granted any roles/logging.* role: Cloud Run and Cloud Tasks
# write their own logs, so no app identity can read, edit or delete them
# (ASVS V16.2.3, V16.4.2, V16.4.3).
resource "google_logging_project_bucket_config" "app" {
  project        = var.project_id
  location       = var.region
  bucket_id      = "mailtinder-logs"
  retention_days = 90
  locked         = var.lock_log_bucket

  lifecycle {
    prevent_destroy = true
  }
}

locals {
  # Every entry the application and its runtime produce.
  app_log_resource_filter = join(" OR ", [
    "resource.type=\"cloud_run_revision\"",
    "resource.type=\"cloud_tasks_queue\"",
    "resource.type=\"cloud_scheduler_job\"",
  ])

  # Cloud Run additionally writes a platform "request log" per inbound request
  # (`logName` .../logs/run.googleapis.com%2Frequests). Those entries record the
  # request URL with its query string and the client IP, which S5 bans from logs
  # (S5-data-inventory "Logs and telemetry"; S7-api-contract: the OAuth callback
  # is a GET carrying `state` and an authorisation `code`). The application
  # cannot redact a platform log, so the sink routes the application's own
  # stdout/stderr only and leaves the request log out of the locked, 90-day,
  # unremovable bucket (review F3).
  app_log_filter = "${local.app_log_resource_filter} AND NOT logName:\"run.googleapis.com%2Frequests\""
}

resource "google_logging_project_sink" "app" {
  project = var.project_id
  name    = "mailtinder-app"

  destination = "logging.googleapis.com/projects/${var.project_id}/locations/${var.region}/buckets/${google_logging_project_bucket_config.app.bucket_id}"
  filter      = local.app_log_filter

  unique_writer_identity = true
}

# The sink's own writer identity needs bucketWriter on the destination bucket.
# This is Google's logging writer, not an application identity, so it does not
# give any MailTinder service account access to logs.
resource "google_project_iam_member" "log_sink_writer" {
  project = var.project_id
  role    = "roles/logging.bucketWriter"
  member  = google_logging_project_sink.app.writer_identity
}

# Keep the app logs out of _Default as well, so they are not retained for 30
# days in a second, less-protected place. This uses the full resource filter
# (including the Cloud Run request log): those entries are dropped entirely
# rather than retained anywhere, so the banned request URL and client IP are
# never captured (review F3).
resource "google_logging_project_exclusion" "app_default" {
  project     = var.project_id
  name        = "mailtinder-app-default"
  description = "App logs are routed only to the locked mailtinder-logs bucket"
  filter      = local.app_log_resource_filter
}

# Audit logs (S6 5, T-1102a step 9): data access on KMS and Secret Manager, and
# admin activity everywhere. allServices accepts ADMIN_READ only.
resource "google_project_iam_audit_config" "all_services" {
  project = var.project_id
  service = "allServices"

  audit_log_config {
    log_type = "ADMIN_READ"
  }
}

resource "google_project_iam_audit_config" "cloudkms" {
  project = var.project_id
  service = "cloudkms.googleapis.com"

  audit_log_config {
    log_type = "DATA_READ"
  }

  audit_log_config {
    log_type = "DATA_WRITE"
  }
}

resource "google_project_iam_audit_config" "secretmanager" {
  project = var.project_id
  service = "secretmanager.googleapis.com"

  audit_log_config {
    log_type = "DATA_READ"
  }

  audit_log_config {
    log_type = "DATA_WRITE"
  }
}