# T-1107: Budget, monitoring and alerts

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 300 lines of HCL plus tests | T-307, T-1102b, T-1114, needs S11 |

**Read only these spec sections:** S4 sections 1 ("Observability", cost paragraph), 4 (Reliability and Cost optimisation rows) and 5.7 (billing alerts bullet) (`docs/specs/S4-architecture.md`); S10 section 8 (`docs/specs/S10-test-strategy.md`); S6 section 7 (`docs/specs/S6-security.md`); register row V16.4.3 in `docs/security/asvs-l2-register.md`. Nothing else is needed.

## Goal

A `monitoring` Terraform module that sets a monthly billing budget with alerts, log-based metrics built from the content-free structured logs, and alert policies that email James when something that must never happen happens (the "unrecoverable action" counters), when jobs expire, when the API errors, when the sweep stops, or when security events spike.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `infra/terraform/modules/monitoring/variables.tf`, `outputs.tf`, `versions.tf` | Interface below |
| Create | `infra/terraform/modules/monitoring/budget.tf` | Project budget and Vertex AI budget |
| Create | `infra/terraform/modules/monitoring/metrics.tf` | Log-based metrics |
| Create | `infra/terraform/modules/monitoring/alerts.tf` | Notification channel and alert policies |
| Create | `infra/terraform/modules/monitoring/uptime.tf` | Uptime checks |
| Create | `infra/terraform/modules/monitoring/tests/monitoring.tftest.hcl` | Plan-time tests |
| Change | `infra/terraform/envs/prod/main.tf`, `envs/staging/main.tf` | `module "monitoring"` (staging: budget only) |

## Types and signatures

```hcl
variable "project_id"          { type = string }
variable "billing_account"     { type = string }               # passed at apply time, not committed
variable "alert_email"         { type = string sensitive = true } # James's address, passed at apply time
variable "monthly_budget_usd"  { type = number default = 10 }  # [DEFAULT] S4: "roughly zero to a few dollars a month"
variable "vertex_budget_usd"   { type = number default = 5 }   # [DEFAULT] S4 5.7
variable "enable_alerts"       { type = bool   default = true } # staging false
variable "app_url"             { type = string }
variable "log_fields"          { type = object({ event = string, outcome = string }) default = { event = "event_type", outcome = "outcome" } } # match T-307
```

## Algorithm

1. **Budgets:** `google_billing_budget` for the project at `monthly_budget_usd` with threshold rules 0.5, 0.9 and 1.0 of actual spend and 1.0 of forecast; a second budget filtered to the Vertex AI service at `vertex_budget_usd`. Both notify the email channel.
2. **Notification channel:** one email channel from `var.alert_email`.
3. **Log-based metrics** (counters, filter on `resource.type="cloud_run_revision"` and `jsonPayload.<event field>`):
   - `unrecoverable_actions`: event in `undo_failed`, `unsub_after_undo`, `history_missing`, `permanent_delete_attempted` (S10 8).
   - `jobs_expired`: event `unsub_outcome` with outcome `expired`.
   - `sweep_runs`: event `sweep` (any outcome).
   - `auth_failures`: security events with outcome `failure` for sign-in, step-up, CSRF and authorisation (S6 7).
   - `rate_limit_hits`: event `rate_limited`.
   Label only by outcome code; never extract any other field (no user ID, no route parameters).
4. **Alert policies** (only when `enable_alerts`), each with `documentation.content` naming the runbook section in S11 (placeholder text "Runbook: S11, to be written"):
   - Unrecoverable action: `unrecoverable_actions` count > 0 in 5 minutes (S10 8: any non-zero value pages James).
   - Jobs expiring: `jobs_expired` > 0 in 1 hour `[DEFAULT]` (S4 4 Reliability).
   - API errors: Cloud Run `request_count` with `response_code_class = "5xx"` for `api` > 5 in 5 minutes `[DEFAULT]`.
   - Sweep stopped: `sweep_runs` absent for 1 hour `[DEFAULT]` (scheduled every 15 minutes).
   - Security spike: `auth_failures` > 20 in 10 minutes `[DEFAULT]`; `rate_limit_hits` > 50 in 10 minutes `[DEFAULT]`.
   - Queue backlog: Cloud Tasks `queue/depth` for `unsubscribe` > 100 for 15 minutes `[DEFAULT]`.
   - Uptime: checks every 5 minutes on `app_url/` and `app_url/api/v1/session`; alert after 2 failed regions `[DEFAULT]`.
5. **Tests** (`mock_provider`, `command = plan`): checks below.

**Waits on S11** (defaults used): budget amounts and currency; whether alerts page (SMS or app) or only email (`[DEFAULT]` email only); on-call and incident steps, including notifiable data breach handling (S11 scope); TypeSafe billing alert (outside Google Cloud: James sets it in the TypeSafe console, S4 5.7); weekly trial measures report for unsubscribe success and undo rate (S10 8, T2: `[DEFAULT]` not an alert, a later report); alert thresholds above.

## Acceptance criteria

None enforced by `ac-coverage` (Terraform). This task supplies the alerting half of V16.4.3 (`review`) and the alert S10 section 8 assumes for the unrecoverable-action counters.

## Tests that must pass

- `run "budget_thresholds_50_90_100_and_forecast"`
- `run "vertex_budget_filtered_to_vertex_ai"`
- `run "unrecoverable_alert_fires_on_any_event"` (threshold 0, 5-minute window)
- `run "every_policy_uses_the_email_channel"`
- `run "metrics_extract_no_labels_but_outcome"`
- `run "staging_has_budget_but_no_alert_policies"` (`enable_alerts = false`)

## Edge cases and traps

- Log metric filters read only the C1 fields (event type and outcome); never add a label extractor for user IDs, addresses or URLs.
- `alert_email` and `billing_account` are passed at apply time and never committed; mark the email `sensitive`.
- Confirm the log field names with T-307 before writing filters; a wrong field name makes an alert that never fires (test with a log line from T-307's tests in the PR).
- An absence alert needs the metric to exist first; create `sweep_runs` in the same apply.
- No infrastructure scanning tools (CLAUDE.md).

## Out of scope

- Dashboards and the trial measures report (later); the S11 runbooks.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has applied the module to production and received the test notification from the email channel.
