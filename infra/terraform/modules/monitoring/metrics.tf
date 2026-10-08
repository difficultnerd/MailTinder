locals {
  event_field   = "jsonPayload.${var.log_fields.event}"
  outcome_field = "jsonPayload.${var.log_fields.outcome}"
  metric_filters = {
    unrecoverable_actions = "${local.event_field}=(\"undo_failed\" OR \"unsub_after_undo\" OR \"history_missing\" OR \"permanent_delete_attempted\")"
    jobs_expired          = "${local.event_field}=\"unsub_outcome\" AND ${local.outcome_field}=\"expired\""
    sweep_runs            = "${local.event_field}=\"sweep\""
    # Include the specific refusal codes emitted by the security helpers, not
    # only the generic failure outcome. S6 7 includes CSRF/authorisation refusals.
    auth_failures   = "${local.event_field}=(\"sign_in\" OR \"step_up\" OR \"csrf_failure\" OR \"authz_failure\") AND ${local.outcome_field}=(\"failure\" OR \"refused\" OR \"csrf_failed\" OR \"forbidden\" OR \"unauthenticated\" OR \"step_up_required\" OR \"wrong_account\" OR \"stale_auth_time\" OR \"state_invalid\" OR \"id_token_invalid\" OR \"not_invited\" OR \"email_mismatch\" OR \"email_unverified\")"
    rate_limit_hits = "${local.event_field}=(\"rate_limited\" OR \"rate_limit_hit\")"
  }
}
resource "google_logging_metric" "counter" {
  for_each = var.enable_alerts ? local.metric_filters : {}
  project  = var.project_id
  name     = each.key
  filter   = "resource.type=\"cloud_run_revision\" AND ${each.value}"
  label_extractors = {
    outcome = "EXTRACT(${local.outcome_field})"
  }
  metric_descriptor {
    metric_kind = "DELTA"
    value_type  = "INT64"
    unit        = "1"
    labels {
      key         = "outcome"
      value_type  = "STRING"
      description = "Content-free registered outcome code."
    }
  }
}
