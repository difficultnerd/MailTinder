variable "project_id" {
  type        = string
  description = "Production project id (created once by James, see ../../README.md)."
}

variable "region" {
  type        = string
  default     = "us-central1"
  description = "Single region for every regional service (S4 1). Do not change for production."
}

variable "lock_log_bucket" {
  type        = bool
  default     = true
  description = "Lock the 90-day log bucket. Locking cannot be undone; production keeps it true."
}

variable "google_oauth_client_id" {
  type        = string
  description = <<-EOT
    Production Google OAuth client id, passed to the unsub service as
    GOOGLE_OAUTH_CLIENT_ID. No default: the unsub service refuses to start
    without it, so a missing or blank value must fail the plan rather than
    deploy a broken service. Supply it in terraform.tfvars (never committed).
  EOT

  validation {
    condition     = length(trimspace(var.google_oauth_client_id)) > 0
    error_message = "google_oauth_client_id must be a non-empty Google OAuth client id (unsub refuses to start without it)."
  }
}

variable "billing_account" {
  type        = string
  description = <<-EOT
    Billing account the monitoring module's two budgets are attached to
    (T-1107, S4 5.7). It is account-level configuration, so it cannot be
    derived from project_id; supply it in terraform.tfvars (never committed).
  EOT
}

variable "alert_email" {
  type        = string
  sensitive   = true
  description = <<-EOT
    James's address: the destination of the monitoring module's email
    notification channel, which every budget and alert policy notifies
    (T-1107, S4 1). Personal data, so it is sensitive and supplied at apply
    time in terraform.tfvars (never committed).
  EOT

  validation {
    condition     = length(trimspace(var.alert_email)) > 0
    error_message = "alert_email must be a non-empty address; an empty channel destination would silence every alert (S4 1, S11)."
  }
}