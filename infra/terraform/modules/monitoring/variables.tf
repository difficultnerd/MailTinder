# Interface of the monitoring module (T-1107): the monthly budget with alerts,
# the log-based metrics built from the content-free structured logs, and the
# alert policies that email James (S4 1 "Observability" and 4, S4 5.7, S6 7,
# S10 8, ASVS V16.4.3).

variable "project_id" {
  type        = string
  description = "Google Cloud project whose spend, logs and uptime are watched (S4 1: one project per environment)."
}

variable "billing_account" {
  type        = string
  description = <<-EOT
    The billing account both budgets are attached to. Supplied at apply time
    (terraform.tfvars, never committed): a budget belongs to an account, not a
    project, so it cannot be derived from project_id (S4 5.7).
  EOT
}

variable "alert_email" {
  type        = string
  sensitive   = true
  description = <<-EOT
    James's address: the sole destination of the email notification channel
    every budget and alert policy uses. Marked sensitive and supplied at apply
    time (never committed) - it is personal data (S4 1, S11).
  EOT

  validation {
    condition     = length(trimspace(var.alert_email)) > 0
    error_message = "alert_email must be a non-empty address; an empty channel destination would silence every alert (S4 1, S11)."
  }
}

variable "monthly_budget_usd" {
  type        = number
  default     = 10
  description = "Project budget in US dollars per month. [DEFAULT] 10: S4 1 puts the expected running cost at trial scale at \"roughly zero to a few dollars a month\", so this is a little headroom above it."
}

variable "vertex_budget_usd" {
  type        = number
  default     = 5
  description = "Vertex AI budget in US dollars per month, a second budget filtered to the Vertex AI service. [DEFAULT] 5: S4 5.7 wants a separate billing alert on Vertex AI usage."
}

variable "enable_alerts" {
  type        = bool
  default     = true
  description = <<-EOT
    Create the alert policies, the log-based metrics they reference and the
    uptime checks. [DEFAULT] true. Staging sets it false: staging gets the
    notification channel and the budgets only, so a deploy there cannot page
    James (T-1107 Files: "staging: budget only").
  EOT
}

variable "app_url" {
  type        = string
  description = <<-EOT
    The public base URL of the app the uptime checks probe, with no trailing
    slash: the Firebase Hosting origin (S4 1), which rewrites `/api/**` to the
    public API service, so `app_url/api/v1/session` reaches the API and
    `app_url/` serves the web app.
  EOT

  validation {
    condition     = length(trimspace(var.app_url)) > 0
    error_message = "app_url must be a non-empty base URL; an uptime check cannot be built from an empty host (S4 1 observability)."
  }
}

# The two JSON keys a T-307 log line carries the event type and outcome under.
# `event` is NOT the field's name: T-307's AllowlistJsonLayer writes `event` for
# the line *category* ("metric", "security", "request", "op") and stores the
# event type ("unsub_outcome", "undo_failed", ...) under `action`
# (backend/crates/obs/src/layer.rs, checks()). A filter built on an
# `event_type` key would match nothing and its alert would never fire - the trap
# T-1107 warns about - so the default is the field T-307 actually emits. The
# outcome key is `outcome`. Both are inputs so a rename in T-307 is one change
# here, not five.
variable "log_fields" {
  type = object({
    event   = string
    outcome = string
  })
  default = {
    event   = "action"
    outcome = "outcome"
  }
  description = "The event-type and outcome JSON keys of a content-free T-307 log line (event = \"action\", outcome = \"outcome\")."
}
