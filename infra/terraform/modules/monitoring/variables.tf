variable "project_id" {
  type = string
}
variable "billing_account" {
  type = string
}
variable "alert_email" {
  type      = string
  sensitive = true
}
variable "monthly_budget_usd" {
  type    = number
  default = 10
  validation {
    condition     = var.monthly_budget_usd > 0 && floor(var.monthly_budget_usd * 100) == var.monthly_budget_usd * 100
    error_message = "The monthly budget must be positive with at most two decimal places."
  }
}
variable "vertex_budget_usd" {
  type    = number
  default = 5
  validation {
    condition     = var.vertex_budget_usd > 0 && floor(var.vertex_budget_usd * 100) == var.vertex_budget_usd * 100
    error_message = "The Vertex budget must be positive with at most two decimal places."
  }
}
variable "enable_alerts" {
  type    = bool
  default = true
}
variable "app_url" {
  type = string
  validation {
    condition     = can(regex("^https://[a-zA-Z0-9.-]+/?$", var.app_url))
    error_message = "app_url must be an HTTPS origin without a port, credentials, query or path."
  }
}
variable "log_fields" {
  type = object({ event = string, outcome = string })
  # T-307 emits MetricEvent.event_type as the JSON `action` field.
  default = { event = "action", outcome = "outcome" }
  validation {
    condition     = alltrue([for field in [var.log_fields.event, var.log_fields.outcome] : can(regex("^[a-zA-Z_][a-zA-Z0-9_]*$", field))])
    error_message = "Log field names must be simple JSON identifiers."
  }
}
