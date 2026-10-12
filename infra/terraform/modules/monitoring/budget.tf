# The budgets (S4 1 "set a billing budget alert", S4 4 Cost optimisation, S4
# 5.7). Two budgets on the one billing account: the whole project at
# var.monthly_budget_usd, and a second one filtered to the Vertex AI service at
# var.vertex_budget_usd, so model spend is watched on its own line even though
# it is also inside the project total. Both notify the one email channel.
#
# Thresholds: 0.5, 0.9 and 1.0 of *actual* spend and 1.0 of the *forecast*. The
# forecast rule is the early warning - it fires before the month's spend has
# actually passed the figure - and it is a separate rule because
# `spend_basis = "FORECASTED_SPEND"` is what makes it forecast-based; the three
# others keep the default `CURRENT_SPEND`.

resource "google_billing_budget" "project" {
  billing_account = var.billing_account
  display_name    = "MailTinder project monthly budget"

  budget_filter {
    projects = ["projects/${data.google_project.watched.number}"]
  }

  amount {
    specified_amount {
      currency_code = "USD"
      units         = tostring(var.monthly_budget_usd)
    }
  }

  threshold_rules {
    threshold_percent = 0.5
  }

  threshold_rules {
    threshold_percent = 0.9
  }

  threshold_rules {
    threshold_percent = 1.0
  }

  # 1.0 of the *forecast*: warns before actual spend reaches the figure.
  threshold_rules {
    threshold_percent = 1.0
    spend_basis       = "FORECASTED_SPEND"
  }

  all_updates_rule {
    monitoring_notification_channels = [google_monitoring_notification_channel.email.id]
  }
}

# The Vertex AI budget (S4 5.7). `services` filters the budget to one service,
# and a budget service filter takes the Cloud Billing catalogue service id
# (`services/{service_id}`), not the API hostname the `api` service calls - see
# var.vertex_billing_service for the id and how to verify it (T-1107 review F4).
# Everything else about it matches the project budget.
resource "google_billing_budget" "vertex" {
  billing_account = var.billing_account
  display_name    = "MailTinder Vertex AI monthly budget"

  budget_filter {
    projects = ["projects/${data.google_project.watched.number}"]
    services = [var.vertex_billing_service]
  }

  amount {
    specified_amount {
      currency_code = "USD"
      units         = tostring(var.vertex_budget_usd)
    }
  }

  threshold_rules {
    threshold_percent = 0.5
  }

  threshold_rules {
    threshold_percent = 0.9
  }

  threshold_rules {
    threshold_percent = 1.0
  }

  threshold_rules {
    threshold_percent = 1.0
    spend_basis       = "FORECASTED_SPEND"
  }

  all_updates_rule {
    monitoring_notification_channels = [google_monitoring_notification_channel.email.id]
  }
}

# The Budgets API takes a project NUMBER in budget_filter.projects
# ("projects/{project_number}"), not the project id; the id gives
# "Error 400: Request contains an invalid argument" (found 12 Oct, AAR 3.134).
data "google_project" "watched" {
  project_id = var.project_id
}
