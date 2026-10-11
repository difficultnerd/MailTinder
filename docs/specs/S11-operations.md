# S11: Operations

Status: DRAFT for James's review. 5 October 2026.
Depends on: `S4-architecture.md`, `S6-security.md`, `S10-test-strategy.md`, `S2-v1-acceptance-criteria.md`, `S7-api-contract.md`, and the repo's existing CI (`.github/workflows/`).
Feeds: S12 (backlog), and the M11 task files (`T-1102a`, `T-1102b`, `T-1103`, `T-1104`, `T-1105`, `T-1107`), whose **Waits on S11** points this document resolves.

`[ASSUMES]` marks a guess that James or a later spec should confirm. `[TUNABLE]` marks a starting number kept in configuration. Each requirement cites the task that raised it as `T-xxxx`.

**This repository is public.** This spec holds no real hostname, project ID, address, account number, key or token. Values appear as placeholders such as `<prod-project-id>`, which describe the shape only. Real values live in Terraform variable files (gitignored), GitHub repository variables and secrets, and James's password manager.

**Scale.** The trial has about 20 users and one operator, James. Every decision below is sized for that: simple, cheap, reviewable by one person. Anything that needs a team (a rota, a paging service) is out until the trial ends.

## 1. Environments

Decision (T-1103, T-1102a, S4 1): two Google Cloud projects, built from the same Terraform modules, each with its own state.

| | Production | Staging |
| --- | --- | --- |
| Project ID shape | `mailtinder-prod-<suffix>` | `mailtinder-stg-<suffix>` |
| Display name | `MailTinder Production` | `MailTinder Staging` |
| Terraform root | `infra/terraform/envs/prod` | `infra/terraform/envs/staging` |
| State | Own GCS bucket, `<prod-project-id>-tfstate`, versioned, public access prevented | Own GCS bucket, `<staging-project-id>-tfstate`, same settings |
| Data | Real trial users | Synthetic only; no real person's mailbox is linked (T-1103) |
| Log bucket | 90 days, locked | 90 days, unlocked (`lock_log_bucket = false`) |
| Alert policies | On (`enable_alerts = true`) | Off; the budget stays on (T-1107) |
| Extra identity | None | Smoke-test identity (T-1105), staging only |
| Region | `us-central1` for every regional resource | Same |

The suffix is a short random string so project IDs are globally unique and not guessable from the product name. The real IDs are never committed (T-1103 "naming of the two projects").

**What lives in each.** Each project holds its own full stack: the three Cloud Run services, the queue, the scheduler job, Firestore, KMS keys, Secret Manager secrets, the log bucket, Artifact Registry, Hosting, and the workload identity pool. Nothing is reached across projects at runtime.

**What must never be shared** (T-1103):

- Terraform state buckets.
- KMS keys, wrapped `data_key` values and Firestore data. A production `data_key` never exists in staging.
- Secret values: HMAC keys, the Jev key and every OAuth client secret. Staging generates its own HMAC keys with `openssl rand -base64 32`.
- The Google OAuth client. Staging uses its own client, with its own consent screen in Testing mode and its own redirect URIs.
- Service accounts and their roles. No staging identity holds a role in production, and the reverse.
- The workload identity pool and provider. Each project has its own, and each pins the repository and branch (section 3).
- The alert email channel configuration (the address is passed at apply time, never committed).
- Billing budgets (each project has its own, section 8).

**Exceptions that are deliberate.** One Artifact Registry digest is promoted from staging to production (section 2), so production reads images built for staging. This is read-only: the production runtime reads from the production repository, populated by the deploy job copying by digest. The sandbox Gmail account (T-1106, S4 1) is not an environment resource; it is a test fixture owned by James.

## 2. Release and deploy

### 2.1 Cadence and flow (T-1104)

- **Every merge to `main` deploys to staging.** No schedule. A deploy is one image build, a push to the staging repository, a deploy of that image by digest, a Hosting release, then the smoke job (T-1105). `[ASSUMES]` this matches the T-1104 default.
- **Production is a manual promotion of a digest that passed staging.** There is no fixed release day. James releases when he has reviewed the changes since the last release, and at least once a week while users are active `[TUNABLE]`.
- **Build once, promote by digest** (T-1104). Production runs the bytes staging tested. The production job never rebuilds. It refuses a digest with no passing staging smoke run recorded for it.
- The deploy passes `--image` only. Terraform owns every other setting, and the image is `ignore_changes` there (T-1102b, T-1104).
- Deploy jobs never run on pull requests, and never run for forks.
- Hosting is released with the same commit as the image, so the app and API never skew by more than one release.

### 2.2 Who approves production, and how it is enforced (T-1104, T-1102b)

`[ASSUMES]` James is the sole approver, as the task files default.

Enforcement is layered so no single setting is the only control:

1. The deploy job for production declares the GitHub environment `production`, configured with James as the only required reviewer, and deployment restricted to the `main` branch. Prevent self-review is **off** while James is the only maintainer, and must be turned on the day a second maintainer exists.
2. The workload identity condition pins the repository and the `main` ref, so a token cannot be minted from any other branch or fork (T-1102b `wif_condition_pins_repository_and_ref`).
3. The production deploy service account can deploy images and release Hosting only. It holds no KMS, Secret Manager, Firestore, IAM admin or primitive role (T-1102b, V13.2.2).
4. `main` is protected by the required checks; nothing reaches `main` except by pull request.
5. The production repository variables (the four WIF values and the project ID) live in the `production` environment, not at repository level, so only a job that passed the approval gate can read them.

### 2.3 Terraform apply (T-1102a)

`[ASSUMES]` James only, from his own machine, using his own named account with temporary elevation (section 3.4). `terraform plan` output is reviewed by James and attached to or summarised in the pull request that changed the HCL. No CI job applies Terraform; the CI `terraform` job runs `fmt -check`, `validate` and `test` only. This keeps the highest-privilege identity off GitHub entirely.

Order for any infrastructure change: staging apply, smoke job, then production apply, so a bad change meets staging first.

### 2.4 Rollback runbook (T-1104)

**Runbook: RB-ROLLBACK.** Target: service restored on the previous good release within **15 minutes** of the decision to roll back. Decision rule: roll back first, diagnose afterwards, if any of these holds for more than 5 minutes after a production deploy: the `api` 5xx alert is firing (section 5), sign-in fails for James's own test account, or the unrecoverable-action alert fires.

Procedure (time-boxed):

1. **0 to 2 min.** Note the current and previous revision digests. The previous digest is in the last successful production deploy run and in the Cloud Run revision list for the service.
2. **2 to 6 min.** Re-run the production deploy workflow with the previous digest as input. Because the deploy takes a digest and does not rebuild, no build time is added. The same approval gate applies; James approves his own run.
3. **If the workflow is unavailable** (GitHub outage, broken workflow file): James shifts traffic directly with his temporary elevated access (section 3.4): route 100% of traffic for each affected service to the previous revision, using the Cloud Run console or `gcloud run services update-traffic`. Record the elevation (section 3.4).
4. **6 to 10 min.** Roll back Hosting to the previous release in the Hosting release history if the front end changed.
5. **10 to 15 min.** Run the smoke job against production's public endpoints only for the checks that are safe in production: `hosting_and_api_headers` and `internal_services_not_public` `[ASSUMES]`. The data-writing smoke checks never run in production (T-1105).
6. **After.** Open an issue titled with the date and the digest. Describe cause, detection time, and fix. No message content, no user identifiers.

**Data changes are not rolled back by this runbook.** There are no Firestore backups (S6 5, James, 4 October 2026), so a release that writes bad data cannot be restored. The defence is that schema changes are additive and a new release must read the previous release's data, so rolling back the image is always safe. A change that cannot meet that rule needs James's explicit sign-off in its pull request.

**Terraform rollback.** Revert the commit, run plan, review and apply, staging first. Resources with `prevent_destroy` or `locked = true` cannot be rolled back and are never changed by this runbook.

### 2.5 Image retention in Artifact Registry (T-1104)

`[ASSUMES]` The task default is keep all; at trial scale the storage cost is cents. Decision: keep every image **tagged with a release commit or promoted to production**; delete untagged digests older than 30 days `[TUNABLE]`; for staging, keep the most recent 30 digests `[TUNABLE]`. The rollback runbook needs only the last few production digests, so the production repository keeps at least the last 10 `[TUNABLE]`. Cleanup policies are set in Terraform (a change to T-1102a step 10, to be made by the reviewer; this spec does not edit the task file). Until that is in place, keep all.

## 3. Access and identity

### 3.1 Principals

| Principal | Purpose | Roles | Notes |
| --- | --- | --- | --- |
| `api`, `unsub`, `worker` runtime service accounts | Run the three services | Exactly the S4 2 table | Hold no logging role, no IAM role, no storage role (V16.4.3) |
| `tasks-invoker` | Identity on Cloud Tasks calls to `unsub` | Cloud Run invoker on `unsub` only | `api` holds `actAs` on this account only |
| `scheduler-invoker` | Identity on Cloud Scheduler calls to `worker` | Cloud Run invoker on `worker` only | Cloud Scheduler's service agent holds `actAs` on this account only |
| `deployer-prod` | GitHub Actions deploy to production | Push to the repository, deploy Cloud Run revisions, release Hosting, `actAs` on the three runtime accounts only | No KMS, Secret Manager, Firestore, IAM admin or primitive role |
| `deployer-staging` | Same for staging | Same, plus nothing else | Separate from `smoke` |
| `smoke` (staging only) | Post-deploy smoke tests | Staging-only roles named in T-1105 | Does not exist in the production root |
| James, named human account | Reviews, applies Terraform, break-glass | See 3.4 | No standing owner role `[ASSUMES]` |

Role shape, not names, is binding; T-1102a, T-1102b and T-1105 hold the exact roles. No primitive role (`owner`, `editor`, `viewer`) is granted to any service account or in any module (T-1102a `no_primitive_roles`). No service account has a key (T-1102a `no_service_account_keys`).

### 3.2 Deploy identity: workload identity federation only (T-1102b, T-1104)

- GitHub Actions authenticates through a workload identity pool and an OIDC provider in each project. There are no long-lived keys, no `GOOGLE_CREDENTIALS` secret and no JSON key files, anywhere.
- The provider's attribute condition pins the repository and the `main` ref (and, for production, the `production` environment claim). A token from any other repository, fork, branch or pull request is refused.
- The pool principal is bound to the deployer service account through `roles/iam.workloadIdentityUser` on that one account.
- The four WIF values (project number, pool, provider, deployer account) are stored as GitHub variables. They are identifiers rather than secrets, but they are not committed to the repository.
- The Terraform state bucket is readable only by James. CI never reads it.

### 3.3 Who can read which secret (T-1102a)

| Secret | Accessor |
| --- | --- |
| Google OAuth client secret | `api`, `unsub`, `worker` |
| Jev API key | `api` only |
| Email lookup HMAC key | `api` only |
| Log pseudonymisation HMAC key | `api`, `unsub`, `worker` |
| Sandbox Gmail OAuth secrets (nightly contract run, T-1106) | GitHub Actions secrets in a restricted environment; not in Google Cloud |
| `ZAP_TARGET_URL`, `STAGING_URL` | GitHub repository variables (not secret, not committed) |

Accessor lists are authoritative `_iam_binding` resources, so no member can be added outside Terraform. A human reads a secret value only through the elevation in 3.4, and the read is recorded by Data Access audit logs.

### 3.4 Break-glass and temporary elevated access

`[ASSUMES]` One human operator.

- **Standing access.** James's named account holds read-only access (the ability to view logs, monitoring, Cloud Run and Terraform plans) and the ability to impersonate the Terraform apply service account `tf-apply` `[ASSUMES]`. It holds no standing owner role and no standing access to secret values or KMS decrypt.
- **Temporary elevation.** To apply Terraform, read a secret, or shift traffic by hand, James impersonates a purpose-specific service account or takes a time-limited IAM grant (Privileged Access Manager if the organisation exists; otherwise an IAM binding with a condition expiring in at most **1 hour** `[TUNABLE]`, added by a second Terraform-free path described next). Grants have an expiry condition set at the time, never an open-ended role.
- **Break-glass.** If James's normal path is broken (for example the impersonation grant is lost), the break-glass path is the project owner on the Google Cloud account that owns the billing account and project, used from a hardware-key-protected login, with credentials stored offline. It is used only in an incident or for recovery, and every use is followed by a review the same day.
- **How it is recorded.**
  1. Admin Activity audit logs (always on) and Data Access audit logs for KMS and Secret Manager (T-1102a, on) record who did what.
  2. For every elevation, James opens an issue from a template, `Access elevation`, before or within 24 hours after, recording the date, the reason, the role, the expiry and the ticket or incident it relates to. The issue contains no secret value and no user data. The repository is public, so the issue says "secret read" and the secret's logical name, which is not sensitive, and nothing else.
  3. An alert fires on any use of the break-glass path and on any grant of an owner role (section 5, alert A9).
  4. Monthly, James compares the audit log of elevations to the issues and closes the gaps. `[ASSUMES]` A second reviewer is added if a second maintainer exists.
- **First admin.** The first admin is set with the `mt-admin` tool, not by Terraform (T-507, T-1102a, V6.3.2). James runs it with temporary access to the system key `system-fields` (S6 5), which the tool uses.
- **Production data and secrets are never reachable from GitHub Actions.** The `smoke` identity is staging only.

## 4. Secrets and key rotation

Rotation is a runbook, not a script that runs unattended. Secret values are added with `gcloud secrets versions add` and never by Terraform, so none appear in state (T-1102a V13.3.1). Every rotation is recorded as an `Access elevation`-style issue titled `Rotation`, with the date, the secret's logical name and the new version number. The calendar reminder is James's.

### 4.1 Schedule and blast radius

| Secret or key | Interval | Blast radius if it leaks or is lost | Runbook |
| --- | --- | --- | --- |
| `data-key-kek` (KMS) | Yearly, automatic; old versions kept for unwrap | Unwraps every user's `data_key`, which decrypts refresh tokens and sealed tokens. Highest impact. Destroying a key version makes everything wrapped under it unrecoverable | RB-KMS |
| `system-fields` (KMS) | Yearly, automatic; old versions kept for decrypt | Invite and invite-request email addresses and `pre_auth` fields. No user mail data | RB-KMS |
| Google OAuth client secret | On provider rotation, and yearly `[TUNABLE]` | Attacker can impersonate the app to Google token endpoints for users who consented. Users' refresh tokens remain under `data_key` | RB-OAUTH |
| Jev API key | Quarterly (S4 5.7, S6 T20) | Spend and use of the vendor account; no user data held by the key | RB-JEV |
| Email lookup HMAC key | Yearly | Offline guessing of invited email addresses from lookup hashes. Rotating invalidates existing lookups unless rewritten | RB-HMAC-LOOKUP |
| Log pseudonymisation HMAC key | Yearly | Re-identification of pseudonymous log IDs by guessing. Rotating breaks continuity of pseudonyms across the boundary only | RB-HMAC-LOG |
| Sandbox Gmail OAuth secrets | Re-authorise weekly while in Testing mode (S4 1); rotate on any suspicion | Access to one synthetic mailbox only | RB-SANDBOX |
| Per-user `data_key` | On demand, re-encrypt; destroyed on deletion | One user | Application behaviour (S6 5), not an operator runbook |
| Session IDs, invite tokens | Per use or expiry | One session or invite | Application behaviour |

### 4.2 Runbooks

- **RB-KMS.** Automatic rotation is set in Terraform (T-1102a `kms_key_rotates_yearly_in_region`) and creates a new primary version; old versions stay enabled so existing wrapped keys unwrap. Checking: after each yearly rotation, confirm a new primary version exists and `api` can wrap and unwrap (the staging smoke `kms_round_trip` does this in staging; the same check is run in production by watching the next sign-in). **Compromise:** never destroy a version that wraps live `data_key` values. Instead, create a new version, re-wrap every `data_key` under it using the application's re-encrypt path (S6 5), verify, then disable (not destroy) the old version for 30 days `[TUNABLE]`, then schedule its destruction. If the key is believed exposed, treat as an incident (section 7.3).
- **RB-OAUTH.** Create a new client secret in the provider console, add it as a new secret version, deploy a new revision of each service so it reads the new version, confirm sign-in and a worker sweep, then disable the old client secret. Services read the secret at start, so a restart is required. Time: under 30 minutes.
- **RB-JEV.** Obtain a new key from the vendor, add as a new version, restart `api` by redeploying the same digest, confirm the bake-off call path using the kill switch off-and-on in staging first, then revoke the old key with the vendor. If the key is lost or leaked, set `CLASSIFIER_JEV_ENABLED` off first (S4 5.7) to stop spend, then rotate.
- **RB-HMAC-LOOKUP.** Rotating changes every lookup hash. Procedure: add the new version; the application must accept both versions for lookups for a window of 30 days `[TUNABLE]` `[ASSUMES]` (a code change not yet in the backlog; raise as a task if rotation is wanted before that). Until that exists, rotation happens only on suspected compromise and invites are re-issued.
- **RB-HMAC-LOG.** Add a new version, redeploy all three services, record the changeover date in the rotation issue so analysis across the boundary knows pseudonyms differ. No data migration.
- **RB-SANDBOX.** Re-authorise the sandbox Gmail account in Testing mode weekly; update the GitHub Actions secret; run the nightly contract job once by hand.

### 4.3 Rules for every rotation

- New version first, deploy, verify, then disable the old version. Disable before destroy, and destroy only after the interval in the row above has passed with no use.
- Never paste a secret into an issue, pull request, chat or log. Read from a password manager, pipe from `openssl` to `gcloud`, and clear the terminal history of the session.
- If a rotation fails, roll back by re-enabling the old version and redeploying; this is why old versions stay until verified.

## 5. Monitoring, alerting and log routing

### 5.1 Log routing (T-1102a, T-1107)

- Application and platform logs go to Cloud Logging, then a **dedicated 90-day log bucket in `us-central1`**, locked in production, unlocked in staging (T-1102a, T-1103).
- Application identities hold no logging role (V16.4.3). Only James's named account reads logs, read-only.
- Logs carry the S5 allowed fields only. No message body, subject, snippet, address, URL, token or message ID is ever logged (CLAUDE.md). The alert filters read only event type and outcome (T-1107).
- Log-based metrics extract **no labels except `outcome`** (T-1107 `metrics_extract_no_labels_but_outcome`). Confirm field names against T-307 before writing filters.
- Audit: Admin Activity logs on (default); Data Access logs on for KMS and Secret Manager (T-1102a).

### 5.2 Alert policies (T-1107)

**Channel:** every policy uses the one email channel (T-1107 `every_policy_uses_the_email_channel`). The address is passed at apply time and marked `sensitive`; it is never committed. `[ASSUMES]` email only, no SMS or app page; this is the T-1107 default (see Open conflicts, item 2).

**Owner:** James owns every alert (sole operator). **Runbook:** each policy carries `documentation.content` naming its runbook section by its identifier below (T-1107 requirement), plus a one-line "what to check first". Documentation content contains no identifiers or values.

| ID | Signal | Threshold and window `[TUNABLE]` | Owner | Runbook section |
| --- | --- | --- | --- | --- |
| A1 | Unrecoverable action: log metric for `undo_failed`, `unsub_after_undo`, `history_missing` (S10 8) | Any event: threshold 0, 5-minute window (T-1107 `unrecoverable_alert_fires_on_any_event`) | James | S11 section 7.1 (RB-SMOKE-AND-INCIDENT), then S11 section 2.4 if after a deploy |
| A2 | `api` server errors (5xx) | More than 5% of requests for 5 minutes, with at least 20 requests `[TUNABLE]` | James | S11 section 2.4 (RB-ROLLBACK) |
| A3 | `unsub` or `worker` server errors | Any 5xx burst: 3 or more in 5 minutes `[TUNABLE]` | James | S11 section 2.4 |
| A4 | Sweep absent: `sweep_runs` metric, expected every 15 minutes (T-1102b, T-1107) | No sweep recorded for 45 minutes (three missed runs) `[TUNABLE]` | James | S11 section 7.2 (RB-SWEEP) |
| A5 | Cloud Tasks unsubscribe queue: tasks failing after final retry, or oldest task age beyond its due time | Any task exhausts the 4 attempts; or oldest task more than 15 minutes late `[TUNABLE]` | James | S11 section 7.2 (RB-QUEUE) |
| A6 | Authorisation, CSRF or rate-limit event spike (S6 7 events) | More than 50 in 5 minutes `[TUNABLE]` | James | S11 section 7.3 (RB-INCIDENT) |
| A7 | KMS or Secret Manager access by a principal other than the three runtime accounts and the elevation path (Data Access logs) | Any event | James | S11 section 3.4, then 7.3 |
| A8 | Terraform or IAM policy change outside the expected apply window (Admin Activity logs: IAM `SetIamPolicy`, key creation) | Any service account key creation; any IAM change not matching an open elevation issue | James | S11 section 3.4, then 7.3 |
| A9 | Break-glass use or grant of an owner role | Any event | James | S11 section 3.4 |
| A10 | Kill-switch change (`config/classifiers`) outside a planned change (S6 7) | Any event: informational | James | S4 5.7 |
| B1 | Budget (section 8) | 50%, 90%, 100% actual, and a forecast of 100% | James | S11 section 8 (RB-COST) |

Policies exist in production only (`enable_alerts = true`); staging has the budget but no alert policies (T-1107 `staging_has_budget_but_no_alert_policies`). Alerts above are the minimum; adding a policy needs an entry in this table first.

### 5.3 Outside Google Cloud (T-1107)

- **TypeSafe billing alert:** James sets it in the TypeSafe console (S4 5.7); it is not in Terraform. Owner James. Runbook: RB-COST.
- **Vertex AI budget:** a separate budget filtered to the Vertex AI service (T-1107 `vertex_budget_filtered_to_vertex_ai`).
- **Weekly trial-measures report** (unsubscribe success, undo rate; S10 8, T2): `[ASSUMES]` not an alert; a later report, as T-1107 defaults. Alert A1 covers the zero-unrecoverable target only.
- **Dashboards:** out of scope for S11 (T-1107).

### 5.4 Alert runbook sections (named)

Every alert's `documentation.content` names one of these sections of this document. They are written in sections 2.4, 3.4, 7 and 8 so no policy points at a section that does not exist.

## 6. Scanning posture

### 6.1 Container scanning (T-1102a, T-1104, S10 12)

**Decision: confirm CLAUDE.md. Container scanning is not enabled.** `containerscanning.googleapis.com` is not enabled, no scanning step is added to CI, and the Artifact Registry repository is created without scanning (T-1102a step 10). Rationale: CLAUDE.md keeps container scanning out of the core; the final image holds only the release binary (ASVS V13.4.1), which leaves little for an image scanner to find that `cargo-audit` and `cargo-deny` in CI do not already cover.

Because it is off, no one reads findings. If James turns it on later, it is an optional layer by his explicit decision, and the reader is James, weekly, with Critical findings acted on under the windows in 6.3. This is stated so T-1104 "who reads scanning findings" has an answer. See Open conflicts, item 1, for the S4 wording.

### 6.2 ZAP (T-1105)

- The nightly ZAP baseline runs against staging (`ZAP_TARGET_URL`), as the `dast` verification for V4.1.2, V12.1.1 and V13.4.3 (T-1105, S10 7).
- **Who reads it:** James reads the report from the workflow run summary each **Monday** for the previous week, and within 1 working day after any run in which new findings appear `[ASSUMES]`. A failed or missing nightly run is treated as a finding.
- **Triage:** each finding is recorded as an issue with `zap` label, marked fix, accepted with a reason, or false positive. Accepted findings are listed in a file under `docs/security/` with the reason and review date `[ASSUMES]`; ZAP rule suppressions live in the ZAP rules file in the repository and are reviewed with the pull request that adds them.
- ZAP is passive baseline only until an authenticated context exists (S10 7.4). It never targets production.

### 6.3 Remediation windows for vulnerable components and findings (V15.1.1)

This resolves the register row V15.1.1 ("risk based remediation time frames for third-party components"). Windows run from the day the finding is first seen, in calendar days `[TUNABLE]`:

| Severity | Applies to | Window |
| --- | --- | --- |
| Critical, or known exploited | A dependency or ZAP finding that is reachable in the deployed service | Fix or mitigate in **7 days**; if exposed, treat under section 7.3 immediately |
| High | Same | **30 days** |
| Medium | Same | **90 days** |
| Low or informational | Same | Next scheduled dependency update, or accepted with a written reason |
| Any severity | A vulnerable dependency not reachable (code path not used) | Recorded with the reasoning; reviewed at the next dependency update |
| Routine | All dependencies, base image tags, GitHub Actions pins and `firebase-tools` | Updated at least **quarterly**, using the existing Dependabot or equivalent update flow, with actions pinned by full SHA |

Owner: James. A missed window is recorded in the issue with a reason and a new date; it is not silently extended. Findings come from CI (`cargo-audit`, `cargo-deny`, Semgrep, Gitleaks), the nightly ZAP report, and GitHub security alerts. The first two are required checks and block a merge.

## 7. Failure handling

### 7.1 Smoke test fails after a deploy (T-1105, T-1104)

**Runbook: RB-SMOKE-AND-INCIDENT.** Decision (`[ASSUMES]`, the T-1105 default): **nothing pages a human for a staging smoke failure.** The deploy stops, the workflow run is marked failed, and GitHub notifies the committer and James by its standard notification email.

- **Staging.** A failed smoke job means the production promotion gate for that digest stays closed: production refuses a digest with no passing smoke run (section 2.1). The committer fixes forward or reverts. Staging is not rolled back automatically, because it holds only synthetic data and being broken is cheap.
- **Cleanup.** The smoke run removes every document it created, including on failure (T-1105); a failed run never leaves data behind.
- **Production.** A deploy to production that goes wrong is detected by the alerts in 5.2, not by smoke tests, because smoke never writes in production. See RB-ROLLBACK.
- **Repeated failure.** Two consecutive smoke failures on `main` block further merges by convention until fixed `[ASSUMES]`; James announces it in the open pull request.
- **No retries to turn red green.** A flaky smoke check is fixed or removed, never retried (CLAUDE.md "no retries").

### 7.2 Operational runbooks

- **RB-SWEEP (alert A4).** Check the `worker` revision is healthy and the Scheduler job is enabled with its OIDC audience equal to the service URL; run the job once by hand from the console; if it returns an error, look at `worker` 5xx logs. Expired jobs and Needs Attention TTL are backstopped by Firestore TTL, which deletes within about 24 hours, so a short sweep outage delays erasure but does not retain data indefinitely. Escalate to RB-ROLLBACK if the last deploy caused it.
- **RB-QUEUE (alert A5).** Check `unsub` is healthy and ingress is internal; check queue is not paused; inspect failed attempts by outcome only (no URLs in logs); a job that ends `failed` raises a Needs Attention item by design, so no user is left silent (S6 5 Access tokens). Do not manually re-run a job whose user has undone it.

### 7.3 Suspected secret exposure on a public repository

**Runbook: RB-INCIDENT.** The repository is public, so any secret that reaches a commit, issue, pull request, workflow log or release must be assumed copied within minutes. Deleting it does not un-leak it.

Order is fixed: **revoke first, clean up second.** Time boxes run from first awareness.

1. **0 to 15 min: contain.**
   - Treat the secret as compromised. Do not wait to confirm use.
   - Revoke or disable it at the source: add a new Secret Manager version and disable the old one; revoke the OAuth client secret in the provider console; revoke the Jev key with the vendor; disable a GitHub Actions secret; for a suspected cloud credential, disable the service account or workload identity provider. Where the leaked item is the Jev key, also turn its kill switch off.
   - If the exposed secret is `data-key-kek` or an equivalent (KMS key material cannot be exported, so this means an access path was exposed, not the key): remove the offending IAM binding first, then follow RB-KMS compromise steps.
2. **15 to 60 min: replace and restore.** Issue the replacement through the matching rotation runbook (section 4), deploy, and verify sign-in and a sweep.
3. **Within 24 hours: investigate.** Use Admin Activity and Data Access logs between the time of the leak and the revocation for any use by an unexpected principal. Record results as counts and event types, with no identifiers, in the incident issue.
4. **Clean up history, after revocation.** Remove the value from the repository only if it helps (a rewrite of public history is disruptive and does not remove forks or caches); the value must be treated as public regardless. Use GitHub's secret scanning alert and, if the commit is still in a pull request, close and recreate. Never force-push to `main` (CLAUDE.md); a history rewrite needs James's explicit decision.
5. **Learn.** Find why Gitleaks (pre-commit and CI) did not stop it. Add a rule if it was a new pattern. Gitleaks is a required check; if it was bypassed, record why.
6. **Notifiable data breach.** If the investigation shows, or cannot rule out, access to personal information (for example a path to `data_key` plus data, or a mailbox token), then James assesses it under the Australian Notifiable Data Breaches scheme `[ASSUMES]` James's jurisdiction is Australia, as the S4 region rationale suggests: the assessment is carried out within **30 days** of becoming aware, and affected users and the regulator are notified as soon as practicable if serious harm is likely. Google's Limited Use policy and any Google verification terms may also require notifying Google; check at the time. Notify users using the in-product or email path that holds their address; never copy addresses into the incident issue. Written notices go from the incident issue's decisions, not into it.
7. **Communication.** The public issue says what class of secret, when revoked, and that no user data was accessed (or that it may have been), nothing more. It never contains the secret, a partial value, a user count that allows identification, or logs.

Mail content is not stored (S4, S6), which bounds what a server breach can expose to refresh tokens (sealed under `data_key`), invite email addresses (under `system-fields`) and pseudonymous IDs.

## 8. Cost

**Runbook: RB-COST.**

- **Budget** `[ASSUMES]`: production USD 20 a month, staging USD 10 a month, Vertex AI USD 5 a month `[TUNABLE]`. These are higher than the expected trial cost ("zero to a few dollars", S4 1) so a breach means something changed. Currency is USD because the quoted prices are in USD. James confirms the numbers before the first apply (T-1107 "budget amounts and currency").
- **Thresholds** (T-1107): alerts at 50%, 90% and 100% of actual spend, and a forecast of 100%, for each budget.
- **Who is alerted:** James, by the email channel, for both projects. The billing account owner is also James. The TypeSafe billing alert is James's, set in the TypeSafe console (S4 5.7).
- **Response to a breach:** on 90% or forecast, James checks the service breakdown in Billing; on 100%, he confirms the cause within one working day. The usual causes are a stuck loop (look at request count and queue depth), Vertex AI usage (turn off `CLASSIFIER_GEMINI_ENABLED`), or logging volume. A budget alert does not shut anything down automatically; automatic caps are out of scope because they would take production down.
- **Staging teardown policy** `[ASSUMES]`: staging stays up while the trial runs, because the smoke tests and ZAP need it. Staging is scaled to zero when idle and has no minimum instances (T-1102b `max_instances_three`). Teardown steps if staging cost exceeds its budget for two consecutive months or the project is not needed: James runs `terraform destroy` for the staging root only, after confirming its state bucket is the staging one (the production root and bucket are never touched). The staging log bucket is unlocked so this works. Production's locked log bucket and `prevent_destroy` resources are not destroyed by any runbook.
- **Billing owner changes** and budget edits are applied through Terraform with `billing_account` and `alert_email` passed at apply time and never committed (T-1107).

## 9. Data

### 9.1 Staging (T-1103)

- **No scheduled reset** (T-1103 default). Staging data is synthetic only (S10 corpus, T-204), and smoke tests clean up what they create, so there is nothing to reset.
- A reset happens on demand only, when James decides the data is contaminated or the schema moved incompatibly. Procedure: delete the staging Firestore documents and secret versions created by test users through the elevated path, never the project, and record it in an issue.
- No real person's mailbox is ever linked in staging. If one is, treat it as a data incident under 7.3.
- Staging has no backups either (S6 5, James, 4 October 2026, which applies to the trial).

### 9.2 Production data access

- **No standing human access to production user data.** Firestore documents hold sealed or encrypted fields; no message content is stored.
- James may read production Firestore only through temporary elevation (3.4), for one of: a support request from the user concerned, a security investigation, or a recovery. Each access has an `Access elevation` issue naming the purpose in words, with no user identifier or content.
- Data Access audit logs record reads. Alert A7 covers KMS and Secret Manager reads outside the runtime accounts.
- **Never copy production data out of the project**: no exports, no scheduled exports, no download to a laptop, and no use of production data in tests or fixtures (CLAUDE.md "no real data in fixtures").
- **Deletion:** account deletion destroys the wrapped `data_key` (crypto-shredding, DEL-2, S6 5). Because there are no backups, no backup copy of the wrapped key exists; historical reads inside about one hour remain possible (ADR 0004). This is the reason backups stay off: enabling Firestore backups or point-in-time recovery requires a new decision by James and a change to S6 5.
- **Retention:** per the S5 TTL collections and the 90-day log retention (S6 9).

## 10. Requirements map

Each row is one open point raised by a task's **Waits on S11** list or by the S11 brief.

| # | Open point | Raised by | Resolved in |
| --- | --- | --- | --- |
| 1 | Naming of the two projects | T-1103 | 1 |
| 2 | Staging and production separation; what must never be shared | T-1103, T-1102a | 1 |
| 3 | Release cadence | T-1104 | 2.1 |
| 4 | Whether staging deploys on every merge or on a schedule | T-1104 | 2.1 |
| 5 | Who approves production, and how it is enforced | T-1104, T-1102b | 2.2 |
| 6 | Who may run `apply`, and where state lives | T-1102a | 2.3, 1 |
| 7 | Rollback runbook | T-1104 | 2.4 |
| 8 | Image retention in Artifact Registry | T-1104 | 2.5 |
| 9 | Instance sizes and timeouts | T-1102b | 11 (Terraform defaults) |
| 10 | Queue dispatch rates | T-1102b | 11 (Terraform defaults) |
| 11 | Whether `api` also gets a custom domain | T-1102b | 11 |
| 12 | Cloud Armor and a load balancer | T-1102b (S4 6) | 11 |
| 13 | Organisation Policy constraints and Security Command Center | T-1102a (S4 1) | 11 |
| 14 | Service accounts, least privilege, deploy identity via WIF | T-1102b | 3.1, 3.2 |
| 15 | Break-glass and temporary elevated access, and how recorded | S11 brief | 3.4 |
| 16 | Who can read which secret | T-1102a | 3.3 |
| 17 | First admin (V6.3.2, S7 3.7) | T-1102a | 3.4 (not Terraform; `mt-admin`, T-507) |
| 18 | Key rotation runbook for every secret and KMS key | T-1102a | 4 |
| 19 | Log alert routing | T-1102a, T-1107 | 5.1, 5.2 |
| 20 | Alert thresholds | T-1107 | 5.2 |
| 21 | Whether alerts page or only email | T-1107 | 5.2 (email only), Open conflicts item 2 |
| 22 | Runbook section named by each alert (`documentation.content`) | T-1107 | 5.2, 5.4 |
| 23 | Alerts for unrecoverable-action counters | T-1107 (S10 8) | 5.2 (A1) |
| 24 | TypeSafe billing alert | T-1107 (S4 5.7) | 5.3 |
| 25 | Weekly trial-measures report | T-1107 (S10 8, T2) | 5.3 |
| 26 | Container scanning enabled or not | T-1102a, T-1104 (S10 12) | 6.1 |
| 27 | Who reads Artifact Registry scanning findings | T-1104, T-1102a | 6.1 |
| 28 | Who reads the nightly ZAP report | T-1105 | 6.2 |
| 29 | Remediation window for findings (V15.1.1) | T-1105 | 6.3 |
| 30 | What happens when a smoke test fails after a deploy; whether it pages | T-1105 | 7.1 |
| 31 | Incident response for a suspected secret exposure on a public repository | S11 brief | 7.3 |
| 32 | On-call and incident steps, including notifiable data breach handling | T-1107 | 7.1, 7.3 |
| 33 | Budget amounts and currency | T-1107 | 8 |
| 34 | Budget monitoring and who is alerted on breach | S11 brief, T-1107 | 8, 5.2 (B1) |
| 35 | Staging teardown and cost policy | T-1103 | 8 |
| 36 | Whether staging data is ever reset | T-1103 | 9.1 |
| 37 | Production data access policy | S11 brief | 9.2 |
| 38 | Firestore backups and point-in-time recovery | T-1102a | 9.2 (off, decided; S6 5) |

## 11. Defaults retained for infrastructure sizing

These points are not operational policy. Terraform's own defaults in the task files stand, and S11 only records that no further decision is made here:

- **Instance sizes and timeouts, queue dispatch rates (rows 9 and 10).** `[ASSUMES]` the module defaults in T-1102b apply: `max_instances` of 3 per service (T-1102b `max_instances_three`), scale to zero, and the queue retry policy fixed by S7 (4 attempts, 30 s, 300 s). Dispatch rate and concurrency start at the Cloud Tasks defaults and are `[TUNABLE]` in Terraform variables. The reviewer sets CPU and memory in the module; S11 sets no number.
- **Custom domain for `api` (row 11).** No. `api` keeps its Cloud Run URL behind Firebase Hosting rewrites (S4 1, S4 6 item 2); a custom domain is decided with Cloud Armor before leaving Testing mode.
- **Cloud Armor and a load balancer (row 12).** Not built, as S4 6 item 2 records. The open decision stays open and gates leaving Testing mode.
- **Organisation Policy and Security Command Center (row 13).** `[ASSUMES]` there is no Google Cloud organisation, so neither is in Terraform (T-1102a default). If an organisation exists later, they are added then and S4 4's "Security Command Center findings at zero high" becomes checkable.

## 12. Open conflicts

These are recorded rather than resolved, because they touch S4, S6, S10 or CLAUDE.md.

1. **Container scanning: S4 versus CLAUDE.md.** S4 1 lists "Artifact Registry with vulnerability scanning" in the services table; CLAUDE.md and S10 6 keep container scanning out of the core. Section 6.1 follows CLAUDE.md (scanning off). S4 should drop the words "with vulnerability scanning" or mark them optional.
2. **Paging for unrecoverable actions: S10 versus T-1107.** S10 8 says a non-zero unrecoverable counter "pages James `[ASSUMES]`"; T-1107 defaults to email only. This spec uses email only for every alert, including A1, because no paging channel exists and the trial has one operator. If James wants A1 to page, T-1107 needs an SMS or app channel and a change to its `every_policy_uses_the_email_channel` test.
3. **Locked log bucket versus staging teardown.** S6 7 and T-1102a require a locked 90-day log bucket in production, which cannot be destroyed until its retention ends. This is consistent with section 8 only because teardown is staging-only. Any proposal to tear production down needs a separate decision.
4. **KMS compromise and crypto-shredding.** Re-wrapping every `data_key` after a KMS incident (RB-KMS) relies on an application re-encrypt path that S6 5 lists ("On demand (re-encrypt)") but no backlog task is known to build. The reviewer should confirm a task exists before the trial starts.
5. **HMAC lookup rotation.** RB-HMAC-LOOKUP needs dual-version lookups that S6 5 does not describe. Until a task exists, the yearly rotation in the S6 table cannot be done without re-issuing invites.
6. **Image retention in T-1102a.** Section 2.5 asks for cleanup policies that T-1102a step 10 does not include. The default stays "keep all" until the task is updated.
7. **Jurisdiction for breach notification.** Section 7.3 assumes Australian law; no spec states James's jurisdiction or the users' locations. James should confirm.
