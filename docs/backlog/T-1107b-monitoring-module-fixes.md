# T-1107b: Monitoring module: fix the alert definitions found by review before the first apply

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 40 lines of HCL plus test changes | T-1107 |

**Read only these spec sections:** `infra/terraform/modules/monitoring/{metrics.tf,uptime.tf,alerts.tf,tests/monitoring.tftest.hcl}`; `backend/crates/api/src/auth/outcome.rs:40-62`; `infra/terraform/envs/staging/main.tf` and `envs/prod/main.tf` (the `module "monitoring"` call). Nothing else is needed.

**Why this task exists (verified 11 Oct 2026 from `main` after PR #82 merged at 06:14 UTC).** The Sol review of docs PR #86 (head 0a6cadfa) read the merged monitoring module and raised four Medium findings. Checked against the code:
- **N1 (verified).** `metrics.tf:47` defines `auth_failures` as events on `sign_in`/`step_up`/`csrf_failure`/`authz_failure` with `NOT outcome="signed_in" AND NOT outcome="stepped_up"`. But successful outcomes also include `joined`, `linked` and `reconnected` (`outcome.rs:51-53`, kept-token outcomes `outcome.rs:70-75`) and step-up success is logged as `success` (`auth/step_up.rs:140`). Every successful invite join, mailbox link and reconnect is counted as an authentication failure, which inflates the alert.
- **N2 (to be confirmed by the first plan/apply).** `uptime.tf:59-70` aggregates `uptime_check/check_passed` with `group_by_fields = ["resource.label.check_id"]`. For the `uptime_url` resource the labels are `project_id` and `host`; `check_id` and `checker_location` are METRIC labels. The alert policy therefore most likely fails API validation at apply. The condition filter also selects every URL uptime check in the project, not the two this module creates.
- **N3.** `envs/{staging,prod}/main.tf` call `module "monitoring"` with only the project and runtime outputs; nothing orders it after the foundation module enabling the Monitoring and Logging APIs, so a first apply can fail with 'API not enabled' on the notification channel or the log-based metrics (a retry succeeds).
- **N4 (design, NOT this task).** Alert thresholds differ from S11 (absolute counts instead of a percentage with a minimum traffic; sweep absence one hour instead of 45 minutes; no internal-service error alert; no audit/IAM/break-glass/kill-switch signals; alerts point at 'S11, to be written' although runbooks exist). Record it as a decision for the owner and a later task; do not change thresholds here.

## Goal

The monitoring module's alerts count what they claim to count and can be created on a first apply.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `infra/terraform/modules/monitoring/metrics.tf` | N1: name the failure outcomes explicitly (a positive list) instead of excluding two success codes |
| Change | `infra/terraform/modules/monitoring/uptime.tf` | N2: group by EXACTLY `metric.label.check_id` (an empty `group_by_fields` is NOT acceptable: it would reduce both probe paths into one series and let two failures at one location trip a threshold meant per path; review finding F2 of PR #87) and select only this module's checks |
| Change | `infra/terraform/envs/staging/main.tf`, `envs/prod/main.tf` | N3: `depends_on = [module.foundation]` on `module "monitoring"` |
| Change | `infra/terraform/modules/monitoring/tests/monitoring.tftest.hcl` | assertions that fail when each behaviour is removed |

(Terraform is owner-merged; the builder prepares the PR.)

## Algorithm

1. **N1.** Failure outcomes are the codes in `outcome.rs` that mean a refused or failed attempt: `not_invited`, `not_registered`, `invite_invalid`, `email_mismatch`, `email_unverified`, `mailbox_linked_elsewhere`, `step_up_wrong_account`, `consent_blocked`, `cancelled`, `failed`, AND `state_invalid` (logged by `state_invalid()` at `auth/callback.rs:812-816` as `event(.., "sign_in", "state_invalid", ..)`, a failure outcome that is not an `Outcome` variant: review finding F1 of PR #87). The source of truth is every outcome string passed to `event(.., "sign_in"|"step_up", <outcome>, ..)` under `backend/crates/api/src/auth/`, not only `Outcome::code()`. Use a positive filter on those for `sign_in` and `step_up`, and keep `csrf_failure` and `authz_failure` as they are (every event of those actions is a failure). Do not list success codes anywhere. Keep the list in one `local` with a comment naming `outcome.rs`.
2. **N2.** Replace the group-by with `metric.label.check_id` and constrain the filter to the two created checks (`metric.label.check_id = one_of(<the ids exposed by the uptime check resources>)`); if the intent is one series per probe path, group by the check id; document the choice in a comment.
3. **N3.** Add the `depends_on` on the module call in both roots (staging and prod).
4. Do not change thresholds, durations, notification routing or any IAM. Do not touch `ci.yml` (finding F2 of PR #82, 'run the monitoring tests in CI', needs an owner-merged `ci.yml` change and is a separate item).

## Behaviours that must hold (no ID-shaped labels: the AC gate resolves IDs against S2/S3/S5/S10/ASVS or a `<STORY> AC<n>` form and reports anything else as unknown, as the review of PR #87 reproduced; the task's own ID is enrolled in `tools/ac_coverage_enforced.txt` when it merges, per CLAUDE.md)

| # | Behaviour (one line) |
| --- | --- |
| B1 | The `auth_failures` metric filter contains no success outcome and matches each failure outcome of `outcome.rs` |
| B2 | The uptime alert aggregates on a label that exists on the metric and only over this module's checks |
| B3 | `module "monitoring"` depends on `module.foundation` in both roots |

## Tests that must pass

- `terraform fmt -check`, `validate` and `terraform test` for `modules/monitoring` and both roots.
- New assertions in `monitoring.tftest.hcl`: B1 (every failure code appears in the filter, none of `signed_in`, `stepped_up`, `joined`, `linked`, `reconnected`, `success`), B2 (`group_by_fields` equals exactly `["metric.label.check_id"]`; the filter names both created checks), each written so that reverting the fix makes it fail.
- A Rust test that scans `backend/crates/api/src/auth/*.rs` for outcome strings passed to the `sign_in`/`step_up` security events and asserts each is either a success code or in the monitoring failure list (so adding an outcome forces a decision); if the module and the Rust crates cannot share the list directly, the Terraform local carries a comment naming this test.
- All required checks green (S10 10.1); Definition of done S10 10.4.

## Edge cases and traps

- Cloud Monitoring cannot be exercised offline: the plan tests prove the strings, not the API's acceptance. The first real plan/apply is the proof; say so in the PR description.
- Do not use `-target` in CI. Do not weaken any existing CI guard.

## Out of scope

- N4 (S11 alert design), Sol finding R5 on PR #72 (a continuously failing sweep still emits the heartbeat; add a failure-outcome alert: a design decision), running monitoring tests in CI (`ci.yml`), `firebase.json` service name (T-1104b), other Low/Info findings of PR #82.

## Done when

- Tests above pass and every required check is green; cross-vendor review; the owner merges (Terraform).
