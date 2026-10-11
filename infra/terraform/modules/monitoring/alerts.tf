# The email notification channel and the alert policies (S4 1 observability,
# S4 4 Reliability, S6 7, S10 8, S11, ASVS V16.4.3). Every budget in budget.tf
# and every policy below notifies the one channel, so a single address receives
# all of it and a policy that forgot the channel fails the plan-time suite.

resource "google_monitoring_notification_channel" "email" {
  project      = var.project_id
  display_name = "MailTinder alerts (email)"
  type         = "email"

  labels = {
    email_address = var.alert_email
  }
}

# `mt-api` is the Cloud Run service name the runtime module creates (T-1102b,
# cloud_run.tf). It is named here rather than passed in: the monitoring module's
# interface is fixed by T-1107 and the API-error policy is the only place the
# name is needed.
locals {
  api_service_name = "mt-api"

  # Shown on every policy that has no S11 runbook yet. The S11 runbooks are out
  # of scope for this task (T-1107 "Out of scope"), so this is the placeholder
  # the task specifies.
  runbook = "Runbook: S11, to be written"
}

# S10 8: any non-zero value pages James. The unrecoverable-action counters mean
# the app did something it must never do, so the threshold is 0 over a
# five-minute window and a single event breaches it.
resource "google_monitoring_alert_policy" "unrecoverable_actions" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Unrecoverable action (S10 8)"
  combiner     = "OR"

  conditions {
    display_name = "unrecoverable_actions > 0 in 5 minutes"

    condition_threshold {
      filter          = "${local.app_log_resource} AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.metric["unrecoverable_actions"].name}\""
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "0s"

      aggregations {
        alignment_period     = "300s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
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

# S4 4 Reliability: "alert on jobs reaching Needs Attention by expiry". A job
# that expires is a user whose unsubscribe never went, so it is worth an email
# on the first one an hour.
resource "google_monitoring_alert_policy" "jobs_expired" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Unsubscribe job expired (S4 4)"
  combiner     = "OR"

  conditions {
    display_name = "jobs_expired > 0 in 1 hour"

    condition_threshold {
      filter          = "${local.app_log_resource} AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.metric["jobs_expired"].name}\""
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "0s"

      aggregations {
        alignment_period     = "3600s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
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

# The public API erroring. Cloud Run's own request_count for the `api` service,
# the 5xx class only, more than five in five minutes. This is a platform metric,
# not one of the log-based counters: a request the router never reached still
# counts.
resource "google_monitoring_alert_policy" "api_errors" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "API 5xx responses (S4 1)"
  combiner     = "OR"

  conditions {
    display_name = "api 5xx > 5 in 5 minutes"

    condition_threshold {
      filter          = "resource.type=\"cloud_run_revision\" AND resource.label.service_name=\"${local.api_service_name}\" AND metric.type=\"run.googleapis.com/request_count\" AND metric.label.response_code_class=\"5xx\""
      comparison      = "COMPARISON_GT"
      threshold_value = 5
      duration        = "0s"

      aggregations {
        alignment_period     = "300s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
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

# The sweep stopped. It is scheduled every 15 minutes (S7 5.12), so an hour
# without a run means the schedule or the worker is dead and jobs are piling up.
# This is an absence alert: it fires when the `sweep_runs` metric, created in
# metrics.tf in the same apply, reports nothing for an hour.
resource "google_monitoring_alert_policy" "sweep_stopped" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Sweep stopped (S4 4)"
  combiner     = "OR"

  conditions {
    display_name = "no sweep run for 1 hour"

    condition_absent {
      filter   = "${local.app_log_resource} AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.metric["sweep_runs"].name}\""
      duration = "3600s"

      aggregations {
        alignment_period     = "3600s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
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

# S6 7 security spike. Two independent breaches of the same idea, either of
# which is a spike: authentication and authorisation failures, or rate-limit
# hits. They share one policy so one email names "security", and the combiner is
# OR so either condition alone fires it.
resource "google_monitoring_alert_policy" "security_spike" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Security event spike (S6 7)"
  combiner     = "OR"

  conditions {
    display_name = "auth_failures > 20 in 10 minutes"

    condition_threshold {
      filter          = "${local.app_log_resource} AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.metric["auth_failures"].name}\""
      comparison      = "COMPARISON_GT"
      threshold_value = 20
      duration        = "0s"

      aggregations {
        alignment_period     = "600s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
      }

      trigger {
        count = 1
      }
    }
  }

  conditions {
    display_name = "rate_limit_hits > 50 in 10 minutes"

    condition_threshold {
      filter          = "${local.app_log_resource} AND metric.type=\"logging.googleapis.com/user/${google_logging_metric.metric["rate_limit_hits"].name}\""
      comparison      = "COMPARISON_GT"
      threshold_value = 50
      duration        = "0s"

      aggregations {
        alignment_period     = "600s"
        per_series_aligner   = "ALIGN_DELTA"
        cross_series_reducer = "REDUCE_SUM"
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

# The unsubscribe queue backing up. Cloud Tasks' own queue depth for the
# `unsubscribe` queue (T-1102b tasks.tf), above 100 held for 15 minutes: a
# backlog that deep means the internal unsub service is not keeping up or is
# failing.
resource "google_monitoring_alert_policy" "queue_backlog" {
  count = var.enable_alerts ? 1 : 0

  project      = var.project_id
  display_name = "Unsubscribe queue backlog (S4 4)"
  combiner     = "OR"

  conditions {
    display_name = "unsubscribe queue depth > 100 for 15 minutes"

    condition_threshold {
      filter          = "resource.type=\"cloud_tasks_queue\" AND resource.label.queue_id=\"unsubscribe\" AND metric.type=\"cloudtasks.googleapis.com/queue/depth\""
      comparison      = "COMPARISON_GT"
      threshold_value = 100
      duration        = "900s"

      aggregations {
        alignment_period     = "900s"
        per_series_aligner   = "ALIGN_MAX"
        cross_series_reducer = "REDUCE_MAX"
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
