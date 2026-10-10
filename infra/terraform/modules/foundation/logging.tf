# Logs (S5 "Logs and telemetry", S6 7, S4 1 observability).
#
# Application logs are routed to one locked 90-day bucket in var.region. No
# application service account is granted any roles/logging.* role: Cloud Run and
# Cloud Tasks write their own logs, so no app identity can read, edit or delete
# them (ASVS V16.2.3, V16.4.2, V16.4.3).
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
  # Everything Cloud Run writes against the revision resource: the application's
  # own stdout/stderr AND the Cloud Run platform/system entries that share the
  # same resource type. The filter is resource-scoped, so it admits those system
  # entries too, not stdout/stderr alone (review F7). Cloud Tasks and Cloud
  # Scheduler write their own platform logs under different resource types, so
  # the locked bucket that S5/S6 present as the application log stream does not
  # capture them.
  app_log_resource_filter = "resource.type=\"cloud_run_revision\""

  # Cloud Run additionally writes a platform "request log" per inbound request
  # (`logName` .../logs/run.googleapis.com%2Frequests). Those entries record the
  # request URL with its query string and the client IP, which S5 bans from logs
  # (S5-data-inventory "Logs and telemetry"; S7-api-contract: the OAuth callback
  # is a GET carrying `state` and an authorisation `code`). The application
  # cannot redact a platform log, so the sink routes the revision entries with
  # the request log excluded and leaves it out of the locked, 90-day,
  # unremovable bucket (review F3).
  app_log_filter = "${local.app_log_resource_filter} AND NOT logName:\"run.googleapis.com%2Frequests\""
}

# The destination is a log bucket in this same project. Cloud Logging routes
# such a sink with no writer identity and grants it no additional permission, so
# this sink declares none: there is deliberately no `unique_writer_identity` and
# no `roles/logging.bucketWriter` IAM member here. (A sink whose destination is
# a log bucket in a *different* project would need a writer identity and that
# grant; neither this project's sink does. The provider rejects an IAM member
# bound to the empty writer identity such a same-project sink returns - review
# D1.)
resource "google_logging_project_sink" "app" {
  project = var.project_id
  name    = "mailtinder-app"

  destination = "logging.googleapis.com/projects/${var.project_id}/locations/${var.region}/buckets/${google_logging_project_bucket_config.app.bucket_id}"
  filter      = local.app_log_filter
}

# Keep the revision entries out of _Default as well, so they are not retained
# for 30 days in a second, less-protected place. The exclusion uses the
# resource filter without the sink's `NOT logName` clause, so it drops every
# Cloud Run revision entry including the platform request log: those entries
# are removed entirely rather than retained anywhere, so the banned request URL
# and client IP are never captured (review F3).
resource "google_logging_project_exclusion" "app_default" {
  project     = var.project_id
  name        = "mailtinder-app-default"
  description = "App logs are routed only to the locked mailtinder-logs bucket"
  filter      = local.app_log_resource_filter
}

# Audit logs (S6 5, T-1102a step 9). ADMIN_READ is an audit *Data Access* log
# type (it records administrative read operations); it is not Admin Activity.
# Admin Activity and System Event audit records are, separately, always routed
# by the immutable `_Required` sink to the `_Required` bucket; a custom sink
# neither removes nor re-routes those normal copies (review F8).
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

# Audit logs (S6 5, S11 3.4/9, V16.4.2, V16.4.3). The monthly elevation review
# (S11 144), alert A7 and leak investigation (S11 282) read who decrypted a KMS
# key or read a secret. The audit configs above only produce those entries; on
# their own they fall into _Default with the default 30-day, unlocked retention.
# This sink routes KMS and Secret Manager Data Access entries to the same locked
# 90-day bucket as the application logs, so the evidence outlives _Default and
# cannot be edited away (review F1, second round). It is additive: the `_Default`
# copy and any `_Required` copy remain (review F8), this sink only adds the
# locked, unremovable copy. As with the app sink, the destination is a
# same-project log bucket, so no writer identity or bucketWriter grant is needed
# (review D1).
resource "google_logging_project_sink" "audit" {
  project = var.project_id
  name    = "mailtinder-audit"

  destination = "logging.googleapis.com/projects/${var.project_id}/locations/${var.region}/buckets/${google_logging_project_bucket_config.app.bucket_id}"
  filter      = "logName:\"cloudaudit.googleapis.com\" AND (protoPayload.serviceName=\"cloudkms.googleapis.com\" OR protoPayload.serviceName=\"secretmanager.googleapis.com\")"
}
