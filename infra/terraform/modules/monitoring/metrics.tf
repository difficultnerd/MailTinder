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

  # The outcome codes a `sign_in` or `step_up` security event carries on a
  # refused or failed attempt (T-1107b N1). The source of truth is every
  # outcome string passed to `event(.., "sign_in"|"step_up", <outcome>, ..)`
  # under backend/crates/api/src/auth/ - not only `Outcome::code()`:
  #   * the failing `Outcome::code()` values in auth/outcome.rs;
  #   * `state_invalid` (auth/callback.rs state_invalid) and `id_token_invalid`
  #     (auth/callback.rs after_state), which are logged on `sign_in` but are
  #     not `Outcome` variants (review F1 of PR #87);
  #   * `wrong_account` and `stale_auth_time`, logged on `step_up` by
  #     auth/step_up.rs step_up_event.
  # Every SUCCESS code is deliberately absent: `signed_in`, `stepped_up`,
  # `joined`, `linked`, `reconnected` and step-up `success`. The old filter
  # excluded only the two it knew (`signed_in`, `stepped_up`), so every
  # successful join, link and reconnect counted as an authentication failure.
  # The Rust test t_1107b_auth_failures_metric_matches_logged_outcomes scans
  # the auth sources and fails when an outcome is logged that is in neither
  # this list nor the success set, so a new outcome forces a decision here.
  auth_failure_outcomes = [
    # Failing `Outcome::code()` values (auth/outcome.rs).
    "not_invited",
    "not_registered",
    "invite_invalid",
    "email_mismatch",
    "email_unverified",
    "mailbox_linked_elsewhere",
    "step_up_wrong_account",
    "consent_blocked",
    "cancelled",
    "failed",
    # Sign-in outcomes that are not `Outcome` variants (auth/callback.rs).
    "state_invalid",
    "id_token_invalid",
    # Step-up outcomes logged by `step_up_event` (auth/step_up.rs).
    "wrong_account",
    "stale_auth_time",
  ]

  # The positive outcome clause for `sign_in` and `step_up`: one
  # `jsonPayload.<outcome>="<code>"` per failure code, OR-ed together.
  auth_failure_outcome_clause = join(
    " OR ",
    [for code in local.auth_failure_outcomes : "jsonPayload.${local.outcome_field}=\"${code}\""],
  )

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

    # The sweep runs every 15 minutes (S7 5.12). The worker emits exactly one
    # `sweep` metric event per run, whatever the outcome
    # (backend/crates/worker/src/sweep.rs run_sweep; the value is registered in
    # backend/crates/obs/src/registry.rs ACTIONS). So this counter has a series
    # for the "sweep stopped" absence alert to be absent from (T-1107 edge
    # case, F1).
    sweep_runs = "jsonPayload.${local.event_field}=\"sweep\""

    # S6 7: security events for the four authentication and authorisation
    # actions. `csrf_failure` and `authz_failure` are logged only when the
    # check fails, so every event of those two actions is a failure and stays
    # in unconditionally (api/src/session/csrf.rs, http/security.rs). `sign_in`
    # and `step_up` are logged with the outcome code, success and failure
    # alike, so the metric names the failure codes explicitly
    # (local.auth_failure_outcomes) rather than excluding a fixed pair of
    # success codes (T-1107b N1; review F1 of PR #87).
    auth_failures = "jsonPayload.event=\"security\" AND ((jsonPayload.${local.event_field}=\"sign_in\" OR jsonPayload.${local.event_field}=\"step_up\") AND (${local.auth_failure_outcome_clause}) OR jsonPayload.${local.event_field}=\"csrf_failure\" OR jsonPayload.${local.event_field}=\"authz_failure\")"

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

  # The metric descriptor must declare one label per `label_extractors` key or
  # the Logging API rejects the metric ("the metric descriptor has no label with
  # this key"). The label is the outcome code and nothing else (T-1107 step 3,
  # F3). A log-based counter is a DELTA INT64.
  metric_descriptor {
    metric_kind = "DELTA"
    value_type  = "INT64"

    labels {
      key         = local.outcome_field
      value_type  = "STRING"
      description = "The S10 8 outcome code of the counted entry; never a user ID, an address or a route parameter."
    }
  }

  # The only label is the outcome code: it lets one counter be split by result
  # without ever extracting a user ID, an address or a route parameter (T-1107
  # edge case; both extractions are anchored on the outcome field).
  label_extractors = {
    (local.outcome_field) = "REGEXP_EXTRACT(jsonPayload.${local.outcome_field}, \"(.*)\")"
  }
}
