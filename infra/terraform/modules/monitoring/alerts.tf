resource "google_monitoring_notification_channel" "email" {
  project      = var.project_id
  display_name = "MailTinder budget and operational alerts"
  type         = "email"
  enabled      = true
  labels = {
    email_address = var.alert_email
  }
}
locals {
  thresholds = {
    unrecoverable_actions = { threshold = 0, window = "300s", duration = "0s", aligner = "ALIGN_SUM" }
    jobs_expired          = { threshold = 0, window = "3600s", duration = "0s", aligner = "ALIGN_SUM" }
    api_errors            = { threshold = 5, window = "300s", duration = "0s", aligner = "ALIGN_SUM" }
    auth_failures         = { threshold = 20, window = "600s", duration = "0s", aligner = "ALIGN_SUM" }
    rate_limit_hits       = { threshold = 50, window = "600s", duration = "0s", aligner = "ALIGN_SUM" }
    queue_backlog         = { threshold = 100, window = "60s", duration = "900s", aligner = "ALIGN_MAX" }
  }
  threshold_filters = merge(
    { for name, metric in google_logging_metric.counter : name => "resource.type=\"cloud_run_revision\" AND metric.type=\"logging.googleapis.com/user/${metric.name}\"" },
    {
      api_errors    = "resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"api\" AND metric.type=\"run.googleapis.com/request_count\" AND metric.labels.response_code_class=\"5xx\""
      queue_backlog = "resource.type=\"cloud_tasks_queue\" AND resource.labels.queue_id=\"unsubscribe\" AND metric.type=\"cloudtasks.googleapis.com/queue/depth\""
    }
  )
}
resource "google_monitoring_alert_policy" "threshold" {
  for_each              = var.enable_alerts ? local.thresholds : {}
  project               = var.project_id
  display_name          = "MailTinder ${each.key}"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.email.name]
  documentation {
    content   = "Runbook: S11, to be written"
    mime_type = "text/markdown"
  }
  conditions {
    display_name = each.key
    condition_threshold {
      filter          = local.threshold_filters[each.key]
      comparison      = "COMPARISON_GT"
      threshold_value = each.value.threshold
      duration        = each.value.duration
      aggregations {
        alignment_period     = each.value.window
        per_series_aligner   = each.value.aligner
        cross_series_reducer = "REDUCE_SUM"
      }
      trigger {
        count = 1
      }
    }
  }
}
resource "google_monitoring_alert_policy" "sweep" {
  count                 = var.enable_alerts ? 1 : 0
  project               = var.project_id
  display_name          = "MailTinder sweep stopped"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.email.name]
  documentation {
    content   = "Runbook: S11, to be written"
    mime_type = "text/markdown"
  }
  conditions {
    display_name = "No sweep for one hour"
    condition_absent {
      filter   = "resource.type=\"cloud_run_revision\" AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.counter["sweep_runs"].name}\""
      duration = "3600s"
      aggregations {
        alignment_period     = "300s"
        per_series_aligner   = "ALIGN_SUM"
        cross_series_reducer = "REDUCE_SUM"
      }
      trigger {
        count = 1
      }
    }
  }
}
