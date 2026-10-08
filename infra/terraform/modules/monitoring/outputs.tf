locals {
  policy_names = concat(
    [for policy in google_monitoring_alert_policy.threshold : policy.name],
    [for policy in google_monitoring_alert_policy.sweep : policy.name],
    [for policy in google_monitoring_alert_policy.uptime : policy.name]
  )
}
output "budget_names" {
  value = { for key, budget in google_billing_budget.budget : key => budget.name }
}
output "notification_channel_name" {
  value = google_monitoring_notification_channel.email.name
}
output "alert_policy_names" {
  value = local.policy_names
}
output "metric_names" {
  value = { for key, metric in google_logging_metric.counter : key => metric.name }
}
output "uptime_check_ids" {
  value = { for key, check in google_monitoring_uptime_check_config.http : key => check.uptime_check_id }
}
