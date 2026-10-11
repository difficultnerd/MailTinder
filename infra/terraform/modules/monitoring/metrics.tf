# Log-based metrics (S10 8, S6 7, S4 1). Each is a counter over the entries the
# content-free structured logger writes and the locked bucket holds
# (`resource.type="cloud_run_revision"`, the same scope foundation's sink uses,
# T-1102a logging.tf). Every filter keys off the two C1 fields alone - the event
# type and the outcome code - never a user ID, an address, a URL or a route
# parameter, so no metric reads anything the logger would not have logged.

locals {
  # The scope every counter shares: the Cloud Run revision resource, i.e. the
  # application's own stdout/stderr entries (foundation logging.tf).
  app_log_resource = "resource.type=\"cloud_run_revision\""

  # The event-type and outcome keys of a T-307 log line (var.log_fields).
  event_field   = var.log_fields.event
  outcome_field = var.log_fields.outcome

  # The filter body (everything after the shared resource clause) per counter.
  metric_filters = {
    # S10 8 "Unrecoverable actions", target zero. Any of these four means the
    # exact previous state was not restored, unsubscribe ran after a successful
    # undo, an automated action left no History entry, or a permanent delete was
    # attempted. The first three are metric event types the services emit
    # (backend/crates/api/src/services/undo.rs, worker/src/sweep.rs);
    # `permanent_delete_attempted` is the S10 8 structural-check counter, kept
    # in the filter so the day anything emits it the alert already exists.
    unrecoverable_actions = "(jsonPayload.${local.event_field}=\"undo_failed\" OR jsonPayload.${local.event_field}=\"unsub_after_undo\" OR jsonPayload.${local.event_field}=\"history_missing\" OR jsonPayload.${local.event_field}=\"permanent_delete_attempted\")"

    # S4 4 Reliability: a job reached Needs Attention by expiry. The expiry
    # writes one `unsub_outcome` metric with outcome `expired`
    # (worker/src/sweep.rs log_expired_outcome).
    jobs_expired = "jsonPayload.${local.event_field}=\"unsub_outcome\" AND jsonPayload.${local.outcome_field}=\"expired\""

    # The sweep runs every 15 minutes (S7 5.12). This counter must exist for the
    # "sweep stopped" absence alert to have a metric to be absent from, whether
    # or not the run itself has an outcome (T-1107 edge case).
    sweep_runs = "jsonPayload.${local.event_field}=\"sweep\""

    # S6 7: security events for the four authentication and authorisation
    # actions. T-307 emits a specific failure code per case (failed, refused,
    # email_mismatch, ...) rather than one `failure` value, so the metric
    # excludes the two success codes instead: everything left on these four
    # actions is a failed or refused attempt
    # (api/src/session/csrf.rs, http/security.rs, auth/callback.rs).
    auth_failures = "jsonPayload.event=\"security\" AND (jsonPayload.${local.event_field}=\"sign_in\" OR jsonPayload.${local.event_field}=\"step_up\" OR jsonPayload.${local.event_field}=\"csrf_failure\" OR jsonPayload.${local.event_field}=\"authz_failure\") AND NOT jsonPayload.${local.outcome_field}=\"signed_in\" AND NOT jsonPayload.${local.outcome_field}=\"stepped_up\""

    # S6 7: rate-limit hits. The security event's action is `rate_limit_hit`
    # with outcome `refused` (api/src/limits.rs); `rate_limited` is the S7 4
    # error code in the response body, not a logged event type, so filtering on
    # it would never fire.
    rate_limit_hits = "jsonPayload.${local.event_field}=\"rate_limit_hit\""
  }
}

resource "google_logging_metric" "metric" {
  for_each = var.enable_alerts ? local.metric_filters : {}

  project = var.project_id
  name    = each.key
  filter  = "${local.app_log_resource} AND ${each.value}"

  # The only label is the outcome code: it lets one counter be split by result
  # without ever extracting a user ID, an address or a route parameter (T-1107
  # edge case; both extractions are anchored on the outcome field).
  label_extractors = {
    (local.outcome_field) = "REGEXP_EXTRACT(jsonPayload.${local.outcome_field}, \"(.*)\")"
  }
}
