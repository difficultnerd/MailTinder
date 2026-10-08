locals {
  budgets = {
    project = { amount = var.monthly_budget_usd, services = [] }
    # Cloud Billing catalog service ID for Vertex AI, not its API hostname.
    vertex = { amount = var.vertex_budget_usd, services = ["services/24E6-581D-38E5"] }
  }
}
resource "google_billing_budget" "budget" {
  for_each        = local.budgets
  billing_account = var.billing_account
  display_name    = "${var.project_id}-${each.key}-monthly"
  budget_filter {
    projects               = ["projects/${var.project_id}"]
    services               = each.value.services
    credit_types_treatment = "INCLUDE_ALL_CREDITS"
    calendar_period        = "MONTH"
  }
  amount {
    specified_amount {
      currency_code = "USD"
      units         = tostring(floor(each.value.amount))
      nanos         = floor((each.value.amount - floor(each.value.amount)) * 1000000000 + 0.5)
    }
  }
  dynamic "threshold_rules" {
    for_each = [0.5, 0.9, 1.0]
    content {
      threshold_percent = threshold_rules.value
      spend_basis       = "CURRENT_SPEND"
    }
  }
  threshold_rules {
    threshold_percent = 1.0
    spend_basis       = "FORECASTED_SPEND"
  }
  all_updates_rule {
    monitoring_notification_channels = [google_monitoring_notification_channel.email.name]
    disable_default_iam_recipients   = true
  }
}
