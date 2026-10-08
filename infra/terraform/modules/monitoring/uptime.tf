locals {
  uptime_paths = { app = "/", session = "/api/v1/session" }
}
resource "google_monitoring_uptime_check_config" "http" {
  for_each         = var.enable_alerts ? local.uptime_paths : {}
  project          = var.project_id
  display_name     = "MailTinder ${each.key} HTTPS"
  timeout          = "10s"
  period           = "300s"
  selected_regions = ["USA", "EUROPE", "ASIA_PACIFIC"]
  monitored_resource {
    type = "uptime_url"
    labels = {
      project_id = var.project_id
      host       = trimsuffix(trimprefix(var.app_url, "https://"), "/")
    }
  }
  http_check {
    path           = each.value
    port           = 443
    request_method = "GET"
    use_ssl        = true
    validate_ssl   = true
    accepted_response_status_codes {
      status_class = "STATUS_CLASS_2XX"
    }
  }
}
resource "google_monitoring_alert_policy" "uptime" {
  for_each              = var.enable_alerts ? local.uptime_paths : {}
  project               = var.project_id
  display_name          = "MailTinder ${each.key} unavailable in two regions"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.email.name]
  documentation {
    content   = "Runbook: S11, to be written"
    mime_type = "text/markdown"
  }
  conditions {
    display_name = "Two failed regions"
    condition_threshold {
      filter          = "resource.type=\"uptime_url\" AND metric.type=\"monitoring.googleapis.com/uptime_check/check_passed\" AND metric.labels.check_id=\"${google_monitoring_uptime_check_config.http[each.key].uptime_check_id}\""
      comparison      = "COMPARISON_LT"
      threshold_value = 1
      duration        = "0s"
      aggregations {
        alignment_period     = "300s"
        per_series_aligner   = "ALIGN_NEXT_OLDER"
        cross_series_reducer = "REDUCE_FRACTION_TRUE"
        group_by_fields      = ["metric.label.checker_location"]
      }
      trigger {
        count = 2
      }
    }
  }
}
