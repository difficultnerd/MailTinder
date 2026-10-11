# Outputs of the monitoring module (T-1107): the handles the environment roots
# and later tasks (the S10 8 report, an S11 runbook) can reference.

output "notification_channel_id" {
  description = "The single email notification channel every budget and alert policy uses (T-1107)."
  value       = google_monitoring_notification_channel.email.id
}

output "budget_ids" {
  description = "Billing budget resource ids by budget: project and vertex (S4 5.7)."
  value = {
    project = google_billing_budget.project.id
    vertex  = google_billing_budget.vertex.id
  }
}

output "metric_names" {
  description = "Log-based metric names by metric (empty when enable_alerts is false)."
  value       = { for name, metric in google_logging_metric.metric : name => metric.name }
}

output "alert_policy_ids" {
  description = "Every alert policy id (empty when enable_alerts is false)."
  value = [
    for policy in concat(
      google_monitoring_alert_policy.unrecoverable_actions,
      google_monitoring_alert_policy.jobs_expired,
      google_monitoring_alert_policy.api_errors,
      google_monitoring_alert_policy.sweep_stopped,
      google_monitoring_alert_policy.security_spike,
      google_monitoring_alert_policy.queue_backlog,
      google_monitoring_alert_policy.uptime,
    ) : policy.id
  ]
}

output "uptime_check_ids" {
  description = "Uptime check ids by check name (empty when enable_alerts is false)."
  value       = { for name, check in google_monitoring_uptime_check_config.check : name => check.uptime_check_id }
}
