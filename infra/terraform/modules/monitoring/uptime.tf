# Uptime checks and their alert (S4 1 observability, S4 4 Reliability). Two
# probes every five minutes against the public origin: the app root (Firebase
# Hosting serves the web app) and `/api/v1/session`, which Hosting rewrites to
# the public API service (S7 API-AUTH-3), so one check covers the static app and
# one covers the API without reaching an internal-only service.

locals {
  # The bare hostname for the `uptime_url` monitored resource: var.app_url with
  # any scheme and a trailing slash removed.
  app_host = trimsuffix(trimprefix(trimprefix(var.app_url, "https://"), "http://"), "/")

  # Probe path -> a stable key, so the check resource can be a map.
  uptime_paths = {
    app_root    = "/"
    api_session = "/api/v1/session"
  }
}

resource "google_monitoring_uptime_check_config" "check" {
  for_each = var.enable_alerts ? local.uptime_paths : {}

  project      = var.project_id
  display_name = "MailTinder ${each.key}"
  timeout      = "10s"
  period       = "300s"

  http_check {
    path         = each.value
    port         = 443
    use_ssl      = true
    validate_ssl = true
  }

  monitored_resource {
    type = "uptime_url"
    labels = {
      project_id = var.project_id
      host       = local.app_host
    }
  }

  selected_regions = ["USA", "EUROPE", "ASIA_PACIFIC"]
}

# Alert once two regions report the check failing, not one: a single region can
# fail for a network reason that is nobody's problem, two failing together is
# the service. `REDUCE_COUNT_FALSE` counts the failing series (check_passed is 1
# for a pass and 0 for a fail), so `> 1` is "two regions".
resource "google_monitoring_alert_policy" "uptime" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Uptime check failing (S4 4)"
  combiner     = "OR"

  conditions {
    display_name = "uptime check failed in 2 regions"

    condition_threshold {
      filter          = "resource.type=\"uptime_url\" AND metric.type=\"monitoring.googleapis.com/uptime_check/check_passed\""
      comparison      = "COMPARISON_GT"
      threshold_value = 1
      duration        = "0s"

      aggregations {
        alignment_period     = "600s"
        per_series_aligner   = "ALIGN_NEXT_OLDER"
        cross_series_reducer = "REDUCE_COUNT_FALSE"
        group_by_fields      = ["resource.label.check_id"]
      }

      trigger {
        count = 1
      }
    }
  }

  notification_channels = [google_monitoring_notification_channel.email.id]

  documentation {
    content   = local.runbook
    mime_type = "text/markdown"
  }
}
