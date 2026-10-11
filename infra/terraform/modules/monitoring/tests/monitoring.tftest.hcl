# Plan-time tests for the monitoring module (T-1107 step 5).
#
# Every run uses mock_provider, so the suite needs no credentials and no
# network. The runs inspect the resources the module declares; the absence
# properties CI checks (no IAM role, no infrastructure scanner) are enforced by
# the role allowlist and resource guards in the `terraform` job, which scan
# every `*.tf` file under modules/ and envs/.
mock_provider "google" {}

# The notification channel's id is computed by the API, so a `command = plan`
# suite cannot read it. Give it a fixed value during plan, so the budget and
# policy assertions can prove they name *this* channel (the same technique the
# foundation suite uses for computed attributes).
override_resource {
  target          = google_monitoring_notification_channel.email
  override_during = plan
  values = {
    id = "projects/mailtinder-test/notificationChannels/1"
  }
}

variables {
  project_id      = "mailtinder-test"
  billing_account = "000000-000000-000000"
  alert_email     = "james@example.com"
  app_url         = "https://mailtinder-test.web.app"
}

run "budget_thresholds_50_90_100_and_forecast" {
  command = plan

  assert {
    condition = (
      length(google_billing_budget.project.threshold_rules) == 4 &&
      toset([for r in google_billing_budget.project.threshold_rules : r.threshold_percent]) == toset([0.5, 0.9, 1.0])
    )
    error_message = "the project budget needs exactly four rules: 0.5, 0.9, 1.0 of actual spend and a second 1.0 of forecast (S4 4)"
  }

  assert {
    condition = length([
      for r in google_billing_budget.project.threshold_rules :
      r if r.spend_basis == "FORECASTED_SPEND" && r.threshold_percent == 1.0
    ]) == 1
    error_message = "one rule must be the 1.0 forecast warning (spend_basis FORECASTED_SPEND), so spend is flagged before the month closes (S4 4)"
  }

  assert {
    condition     = google_billing_budget.project.amount[0].specified_amount[0].currency_code == "USD"
    error_message = "the budget amount must carry a currency"
  }

  assert {
    condition     = toset(google_billing_budget.project.all_updates_rule[0].monitoring_notification_channels) == toset([google_monitoring_notification_channel.email.id])
    error_message = "the budget must notify the one email channel (S4 5.7)"
  }
}

run "vertex_budget_filtered_to_vertex_ai" {
  command = plan

  assert {
    condition = (
      length(google_billing_budget.vertex.budget_filter[0].services) == 1 &&
      contains(tolist(google_billing_budget.vertex.budget_filter[0].services), "services/aiplatform.googleapis.com")
    )
    error_message = "the second budget must be filtered to the Vertex AI service alone, so model spend is watched on its own line (S4 5.7)"
  }

  assert {
    condition     = google_billing_budget.vertex.amount[0].specified_amount[0].units == tostring(var.vertex_budget_usd)
    error_message = "the Vertex AI budget must use var.vertex_budget_usd (default 5, S4 5.7)"
  }

  assert {
    condition = (
      toset(google_billing_budget.vertex.budget_filter[0].projects) == toset(["projects/${var.project_id}"]) &&
      toset(google_billing_budget.vertex.all_updates_rule[0].monitoring_notification_channels) == toset([google_monitoring_notification_channel.email.id])
    )
    error_message = "the Vertex AI budget belongs to var.project_id and notifies the one email channel (S4 5.7)"
  }
}

run "unrecoverable_alert_fires_on_any_event" {
  command = plan

  assert {
    condition = (
      google_monitoring_alert_policy.unrecoverable_actions[0].conditions[0].condition_threshold[0].threshold_value == 0 &&
      google_monitoring_alert_policy.unrecoverable_actions[0].conditions[0].condition_threshold[0].comparison == "COMPARISON_GT"
    )
    error_message = "the unrecoverable-action alert must fire on any non-zero count (threshold 0, strictly greater), so one event pages James (S10 8)"
  }

  assert {
    condition = (
      google_monitoring_alert_policy.unrecoverable_actions[0].conditions[0].condition_threshold[0].aggregations[0].alignment_period == "300s" &&
      strcontains(google_monitoring_alert_policy.unrecoverable_actions[0].conditions[0].condition_threshold[0].filter, "logging.googleapis.com/user/${google_logging_metric.metric["unrecoverable_actions"].name}")
    )
    error_message = "the alert must count the unrecoverable_actions metric over a five-minute window (S10 8)"
  }
}

run "every_policy_uses_the_email_channel" {
  command = plan

  assert {
    condition = length(concat(
      google_monitoring_alert_policy.unrecoverable_actions,
      google_monitoring_alert_policy.jobs_expired,
      google_monitoring_alert_policy.api_errors,
      google_monitoring_alert_policy.sweep_stopped,
      google_monitoring_alert_policy.security_spike,
      google_monitoring_alert_policy.queue_backlog,
      google_monitoring_alert_policy.uptime,
    )) > 0
    error_message = "the module must declare alert policies when enable_alerts is true"
  }

  assert {
    condition = alltrue([
      for policy in concat(
        google_monitoring_alert_policy.unrecoverable_actions,
        google_monitoring_alert_policy.jobs_expired,
        google_monitoring_alert_policy.api_errors,
        google_monitoring_alert_policy.sweep_stopped,
        google_monitoring_alert_policy.security_spike,
        google_monitoring_alert_policy.queue_backlog,
        google_monitoring_alert_policy.uptime,
      ) :
      toset(policy.notification_channels) == toset([google_monitoring_notification_channel.email.id])
    ])
    error_message = "every alert policy must notify exactly the one email channel (S4 1, S11)"
  }

  assert {
    condition = alltrue([
      for policy in concat(
        google_monitoring_alert_policy.unrecoverable_actions,
        google_monitoring_alert_policy.jobs_expired,
        google_monitoring_alert_policy.api_errors,
        google_monitoring_alert_policy.sweep_stopped,
        google_monitoring_alert_policy.security_spike,
        google_monitoring_alert_policy.queue_backlog,
        google_monitoring_alert_policy.uptime,
      ) :
      length(policy.documentation) == 1 && length(trimspace(policy.documentation[0].content)) > 0
    ])
    error_message = "every alert policy must carry runbook documentation so the email links to a next step (S4 1)"
  }
}

run "metrics_extract_no_labels_but_outcome" {
  command = plan

  assert {
    condition = (
      length(google_logging_metric.metric) == 5 &&
      toset(keys(google_logging_metric.metric)) == toset(["unrecoverable_actions", "jobs_expired", "sweep_runs", "auth_failures", "rate_limit_hits"])
    )
    error_message = "the module must declare exactly the five S10 8 / S6 7 counters, so the alert policies have a metric each (S10 8)"
  }

  assert {
    condition = alltrue([
      for metric in values(google_logging_metric.metric) :
      length(metric.label_extractors) == 1 &&
      contains(keys(metric.label_extractors), var.log_fields.outcome) &&
      strcontains(metric.label_extractors[var.log_fields.outcome], "jsonPayload.${var.log_fields.outcome}") &&
      !strcontains(metric.label_extractors[var.log_fields.outcome], "user_pseudo") &&
      !strcontains(metric.label_extractors[var.log_fields.outcome], "request_id")
    ])
    error_message = "the only label a counter may extract is the outcome code, from the outcome field; never a user ID, request ID or route parameter (T-1107 edge case)"
  }

  assert {
    condition = alltrue([
      for metric in values(google_logging_metric.metric) :
      strcontains(metric.filter, "resource.type=\"cloud_run_revision\"") &&
      strcontains(metric.filter, "jsonPayload.${var.log_fields.event}")
    ])
    error_message = "every counter must be scoped to the Cloud Run revision entries and key off T-307's real event field, or its alert can never fire (T-1107 edge case)"
  }

  assert {
    condition = alltrue([
      for metric in values(google_logging_metric.metric) :
      !strcontains(metric.filter, "jsonPayload.user_pseudo") &&
      !strcontains(metric.filter, "jsonPayload.request_id") &&
      !strcontains(metric.filter, "jsonPayload.route")
    ])
    error_message = "no counter may filter on a field that is not a C1 event or outcome code (S5 logs, S10 8)"
  }
}

run "staging_has_budget_but_no_alert_policies" {
  command = plan

  variables {
    enable_alerts = false
  }

  assert {
    condition = (
      length(google_monitoring_alert_policy.unrecoverable_actions) == 0 &&
      length(google_monitoring_alert_policy.jobs_expired) == 0 &&
      length(google_monitoring_alert_policy.api_errors) == 0 &&
      length(google_monitoring_alert_policy.sweep_stopped) == 0 &&
      length(google_monitoring_alert_policy.security_spike) == 0 &&
      length(google_monitoring_alert_policy.queue_backlog) == 0 &&
      length(google_monitoring_alert_policy.uptime) == 0
    )
    error_message = "staging passes enable_alerts = false: it gets no alert policy, so a staging deploy cannot page James (T-1107 Files)"
  }

  assert {
    condition = (
      length(google_logging_metric.metric) == 0 &&
      length(google_monitoring_uptime_check_config.check) == 0
    )
    error_message = "staging is budget only: no log-based metric and no uptime check either (T-1107 Files)"
  }

  assert {
    condition = (
      length(google_billing_budget.project.threshold_rules) == 4 &&
      length(google_billing_budget.vertex.threshold_rules) == 4
    )
    error_message = "staging still gets both budgets with their thresholds (T-1107 Files: staging keeps the budget)"
  }
}

run "uptime_checks_probe_root_and_session" {
  command = plan

  assert {
    condition = (
      length(google_monitoring_uptime_check_config.check) == 2 &&
      contains([for c in values(google_monitoring_uptime_check_config.check) : c.http_check[0].path], "/") &&
      contains([for c in values(google_monitoring_uptime_check_config.check) : c.http_check[0].path], "/api/v1/session")
    )
    error_message = "there must be two five-minute HTTPS checks: the app root and /api/v1/session (S4 1 observability)"
  }

  assert {
    condition = alltrue([
      for c in values(google_monitoring_uptime_check_config.check) :
      c.period == "300s" &&
      c.http_check[0].use_ssl &&
      c.monitored_resource[0].labels["host"] == "mailtinder-test.web.app"
    ])
    error_message = "each check must run every five minutes over HTTPS against var.app_url's host (S4 1 observability)"
  }

  assert {
    condition = (
      google_monitoring_alert_policy.uptime[0].conditions[0].condition_threshold[0].aggregations[0].cross_series_reducer == "REDUCE_COUNT_FALSE" &&
      google_monitoring_alert_policy.uptime[0].conditions[0].condition_threshold[0].threshold_value == 1
    )
    error_message = "the uptime alert must fire only once two regions fail (count of failing regions strictly greater than one)"
  }
}
