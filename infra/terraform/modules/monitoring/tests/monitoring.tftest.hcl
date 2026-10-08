mock_provider "google" {
  mock_resource "google_monitoring_notification_channel" {
    override_during = plan
    defaults        = { name = "projects/synthetic-monitoring/notificationChannels/email" }
  }
}

variables {
  project_id      = "synthetic-monitoring"
  billing_account = "000000-000000-000000"
  alert_email     = "operator@example.invalid"
  app_url         = "https://app.example.invalid"
}

run "budget_thresholds_50_90_100_and_forecast" {
  command = plan
  assert {
    condition = alltrue([for budget in google_billing_budget.budget :
      toset([for rule in budget.threshold_rules : "${rule.spend_basis}:${rule.threshold_percent}"]) == toset(["CURRENT_SPEND:0.5", "CURRENT_SPEND:0.9", "CURRENT_SPEND:1", "FORECASTED_SPEND:1"]) &&
      budget.amount[0].specified_amount[0].currency_code == "USD" &&
      toset(budget.budget_filter[0].projects) == toset(["projects/synthetic-monitoring"]) &&
      toset(budget.all_updates_rule[0].monitoring_notification_channels) == toset([google_monitoring_notification_channel.email.name])
    ]) && google_billing_budget.budget["project"].amount[0].specified_amount[0].units == "10" && google_billing_budget.budget["vertex"].amount[0].specified_amount[0].units == "5"
    error_message = "Both project-scoped USD budgets need actual 50/90/100%, forecast 100%, and the email channel."
  }
}

run "vertex_budget_filtered_to_vertex_ai" {
  command = plan
  assert {
    condition     = toset(google_billing_budget.budget["vertex"].budget_filter[0].services) == toset(["services/24E6-581D-38E5"]) && length(google_billing_budget.budget["project"].budget_filter[0].services) == 0
    error_message = "Only the Vertex budget must filter to the Vertex AI billing service."
  }
}

run "unrecoverable_alert_fires_on_any_event" {
  command = plan
  assert {
    condition     = google_monitoring_alert_policy.threshold["unrecoverable_actions"].conditions[0].condition_threshold[0].threshold_value == 0 && google_monitoring_alert_policy.threshold["unrecoverable_actions"].conditions[0].condition_threshold[0].duration == "0s" && google_monitoring_alert_policy.threshold["unrecoverable_actions"].conditions[0].condition_threshold[0].aggregations[0].alignment_period == "300s" && google_monitoring_alert_policy.threshold["unrecoverable_actions"].conditions[0].condition_threshold[0].comparison == "COMPARISON_GT"
    error_message = "Any unrecoverable action in a five-minute summed window must alert immediately."
  }
}

run "every_policy_uses_the_email_channel" {
  command = plan
  assert {
    condition     = length(local.policy_names) == 9 && alltrue([for policy in concat(values(google_monitoring_alert_policy.threshold), [google_monitoring_alert_policy.sweep[0]], values(google_monitoring_alert_policy.uptime)) : toset(policy.notification_channels) == toset([google_monitoring_notification_channel.email.name]) && policy.documentation[0].content == "Runbook: S11, to be written"])
    error_message = "Every operational policy must email the operator and name the S11 runbook."
  }
}

run "metrics_extract_no_labels_but_outcome" {
  command = plan
  assert {
    condition     = length(google_logging_metric.counter) == 5 && alltrue([for metric in google_logging_metric.counter : toset(keys(metric.label_extractors)) == toset(["outcome"]) && metric.label_extractors.outcome == "EXTRACT(jsonPayload.outcome)" && length(metric.metric_descriptor[0].labels) == 1 && one(metric.metric_descriptor[0].labels).key == "outcome" && metric.metric_descriptor[0].metric_kind == "DELTA" && metric.metric_descriptor[0].value_type == "INT64"])
    error_message = "Log counters may extract only the content-free outcome code."
  }
}

run "staging_has_budget_but_no_alert_policies" {
  command = plan
  variables { enable_alerts = false }
  assert {
    condition     = length(google_billing_budget.budget) == 2 && length(google_monitoring_alert_policy.threshold) == 0 && length(google_monitoring_alert_policy.sweep) == 0 && length(google_monitoring_alert_policy.uptime) == 0 && length(google_monitoring_uptime_check_config.http) == 0 && length(google_logging_metric.counter) == 0
    error_message = "Staging has both emailed budgets but no operational monitoring resources."
  }
}

run "t307_log_fields_and_security_actions" {
  command = plan
  assert {
    # T-307 proves event_type is emitted as JSON action, not event_type.
    condition     = var.log_fields.event == "action" && alltrue([for metric in google_logging_metric.counter : startswith(metric.filter, "resource.type=\"cloud_run_revision\"") && strcontains(metric.filter, "jsonPayload.action")]) && alltrue([for event in ["undo_failed", "unsub_after_undo", "history_missing", "permanent_delete_attempted"] : strcontains(google_logging_metric.counter["unrecoverable_actions"].filter, "\"${event}\"")]) && strcontains(google_logging_metric.counter["jobs_expired"].filter, "jsonPayload.outcome=\"expired\"") && alltrue([for action in ["sign_in", "step_up", "csrf_failure", "authz_failure"] : strcontains(google_logging_metric.counter["auth_failures"].filter, "\"${action}\"")]) && strcontains(google_logging_metric.counter["rate_limit_hits"].filter, "\"rate_limit_hit\"")
    error_message = "Filters must use actual T-307 JSON action and outcome fields."
  }
}

run "operational_thresholds_and_aggregation" {
  command = plan
  assert {
    condition = alltrue([for key, expected in {
      jobs_expired    = { threshold = 0, window = "3600s", duration = "0s" }
      api_errors      = { threshold = 5, window = "300s", duration = "0s" }
      auth_failures   = { threshold = 20, window = "600s", duration = "0s" }
      rate_limit_hits = { threshold = 50, window = "600s", duration = "0s" }
      queue_backlog   = { threshold = 100, window = "60s", duration = "900s" }
    } : google_monitoring_alert_policy.threshold[key].conditions[0].condition_threshold[0].threshold_value == expected.threshold && google_monitoring_alert_policy.threshold[key].conditions[0].condition_threshold[0].aggregations[0].alignment_period == expected.window && google_monitoring_alert_policy.threshold[key].conditions[0].condition_threshold[0].duration == expected.duration]) && google_monitoring_alert_policy.sweep[0].conditions[0].condition_absent[0].duration == "3600s" && alltrue([for key in ["unrecoverable_actions", "jobs_expired", "api_errors", "auth_failures", "rate_limit_hits"] : google_monitoring_alert_policy.threshold[key].conditions[0].condition_threshold[0].aggregations[0].per_series_aligner == "ALIGN_SUM" && google_monitoring_alert_policy.threshold[key].conditions[0].condition_threshold[0].aggregations[0].cross_series_reducer == "REDUCE_SUM"])
    error_message = "Count alerts must sum across revisions/outcomes and use the specified windows."
  }
}

run "uptime_checks_two_failed_regions" {
  command = plan
  assert {
    condition     = length(google_monitoring_uptime_check_config.http) == 2 && google_monitoring_uptime_check_config.http["app"].http_check[0].path == "/" && google_monitoring_uptime_check_config.http["session"].http_check[0].path == "/api/v1/session" && alltrue([for check in google_monitoring_uptime_check_config.http : check.period == "300s" && check.http_check[0].use_ssl && check.http_check[0].validate_ssl && check.monitored_resource[0].labels.host == "app.example.invalid" && length(check.selected_regions) >= 3]) && alltrue([for policy in google_monitoring_alert_policy.uptime : policy.conditions[0].condition_threshold[0].comparison == "COMPARISON_LT" && policy.conditions[0].condition_threshold[0].threshold_value == 1 && policy.conditions[0].condition_threshold[0].trigger[0].count == 2])
    error_message = "Both HTTPS endpoints must be checked every five minutes and page on two failed regions."
  }
}

run "custom_log_fields" {
  command = plan
  variables { log_fields = { event = "event_type", outcome = "result" } }
  assert {
    condition     = alltrue([for metric in google_logging_metric.counter : strcontains(metric.filter, "jsonPayload.event_type") && metric.label_extractors.outcome == "EXTRACT(jsonPayload.result)"])
    error_message = "Custom log field mappings must be honored in filters and extractors."
  }
}
