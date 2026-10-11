variable "project_id" {
  type        = string
  description = "Staging project id (`<name>-staging`), created once by James (see ../../README.md). Never the production project."
}

variable "region" {
  type        = string
  default     = "us-central1"
  description = "Single region for every regional service (S4 1). The Firestore location is permanent once created, so staging matches production rather than choosing its own."
}

variable "google_oauth_client_id" {
  type        = string
  description = <<-EOT
    Staging Google OAuth client id, passed to the unsub service as
    GOOGLE_OAUTH_CLIENT_ID. It is the staging project's own OAuth client
    (created in Testing mode, see ../../README.md), never production's. No
    default: the unsub service refuses to start without it, so a missing or
    blank value must fail the plan rather than deploy a broken service. Supply
    it in terraform.tfvars (never committed).
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
    (T-1107, S4 5.7). The staging project's billing account; supply it in
    terraform.tfvars (never committed).
  EOT
}

variable "alert_email" {
  type        = string
  sensitive   = true
  description = <<-EOT
    James's address: the destination of the monitoring module's email
    notification channel (T-1107, S4 1). Staging still creates the channel for
    its budgets even though it has no alert policy; supply it in
    terraform.tfvars (never committed).
  EOT

  validation {
    condition     = length(trimspace(var.alert_email)) > 0
    error_message = "alert_email must be a non-empty address; an empty channel destination would silence every budget notification (S4 1, S11)."
  }
}
