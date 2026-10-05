# S11: Operations

Status: DRAFT for James's review. 5 October 2026.
Depends on: `S4-architecture.md` (services, environments, service account roles), `S5-data-inventory.md` (Firestore retention, logs and telemetry, data location), `S6-security.md` (KMS, deletion, security logging), `S10-test-strategy.md` (staging smoke tests, DAST, trial measures), and the ASVS register `docs/security/asvs-l2-register.md` (rows V13.1, V15.1.1, V16.4 and related).
Feeds: S12 (backlog), the Terraform tasks T-1102a, T-1102b, T-1103, T-1104, T-1105, T-1106 and T-1107, and the ASVS register `review` rows this spec owns.
Prerequisite: this spec unblocks Terraform task T-1102a (`needs S11`). It is written to be concrete enough for a Terraform engineer to build against.

`[ASSUMES]` marks a guess that James or a later spec should confirm. `[TUNABLE]` marks a starting number kept in configuration. `[DEFAULT]` marks a default the task files already use.

## 1. Purpose

This spec defines how MailTinder runs: the two environments and how they differ, how secrets are handled, the deploy pipeline, logging and monitoring that carry no mail content, backup, and the operational ASVS rows no other spec holds. It is the operations contract the Terraform tasks build against and the runbook James follows.

The governing constraint is the privacy principle from S5: **no mail content at rest on our infrastructure, and no mail content in logs.** Every operational control below is designed so that an operator, an alert, or a log reader can never see a message body, subject, snippet, address, URL or token.

## 2. Environments

Production and staging are separate Google Cloud projects built from the same Terraform, each with its own state (S4 "Environments", T5 decided 3 October 2026). Both use the one `region` variable set to `us-central1`; Firestore location is set explicitly and is permanent once created (S4 1).

| Aspect | Production (`prod`) | Staging (`staging`) |
| --- | --- | --- |
| Project | Separate Google Cloud project | Separate Google Cloud project (T-1103) |
| Terraform state | GCS bucket `<project>-tfstate`, prefix `prod` | GCS bucket `<project>-tfstate`, prefix `staging` |
| Log bucket | `locked = true`, 90-day retention (V16.4.2) | `lock_log_bucket = false` (T-1103; the lock is irreversible, so staging stays unlocked) |
| Post-deploy checks | None by default | Staging smoke tests (S10 3.1) and the ZAP baseline scan (S10 7.4) run after each deploy |
| Data | Real user data | Synthetic data only; no real mailboxes, no real users |
| Who applies | James only, from his machine | James only, from his machine |
| Purpose | The trial and beyond | Smoke tests, DAST, and pre-release verification |

Both environments are built from the same Terraform modules so a change is proven in staging before it reaches production. The only intended differences are the ones in the table above; any other divergence is a bug in the Terraform.

**Sandbox mailboxes** (T1, decided 3 October 2026): one Gmail account (v1) and one Outlook.com account (v2), owned by James, holding synthetic mail only. Their OAuth secrets live in GitHub Actions secrets for the nightly live contract run (S4 "Environments"). While the app is in Google Testing mode the Gmail sandbox needs re-authorising weekly, a recurring task for James.

## 3. Secrets handling

### 3.1 Where secrets live

All secrets live in **Secret Manager** (S4 1, S5 "Keys and secrets held outside Firestore", S6 5). The secrets and their accessors are fixed by S4 section 2 and T-1102a step 6:

| Secret | Accessors | Rotation |
| --- | --- | --- |
| `oauth-client-secret` | `api`, `unsub`, `worker` | On provider rotation |
| `jev-api-key` | `api` only | Quarterly |
| `email-lookup-hmac-key` | `api` only | Yearly |
| `log-pseudonym-hmac-key` | `api`, `unsub`, `worker` | Yearly |

KMS keys are not secrets in Secret Manager; they live in Cloud KMS (S6 5). The two KMS keys are `data-key-kek` (wraps every user's `data_key`; encrypt and decrypt held by exactly `api`, `unsub`, `worker`) and `system-fields` (pre-user data; encrypt and decrypt held by `api` only, plus James's account for the admin tool T-507).

### 3.2 No secret values in the repo

- **No secret value ever enters the repository, Terraform state, or a build artifact.** Terraform creates secret *containers* and accessor bindings only; it never creates a `google_secret_manager_secret_version` resource, so values never enter Terraform state (T-1102a step 6, V13.3.1). James adds values with `gcloud secrets versions add` after apply.
- **No service account keys.** Terraform never creates a `google_service_account_key` (T-1102a step 3). Runtime authentication uses Application Default Credentials and OIDC identity tokens between services (V13.2.1).
- **Gitleaks runs in pre-commit and CI** (S6 8, V13.3.1) and fails the build on any committed secret.
- **`terraform.tfvars` is gitignored**; only `terraform.tfvars.example` is committed, with placeholder values and comments, never a real project ID, email or secret (T-1102a step 11, edge cases).
- **GitHub Actions secrets** hold the sandbox mailbox OAuth credentials for the nightly live contract run (S4 "Environments", S10 4.3). These are the only secrets outside Secret Manager, and they are scoped to the nightly test job only.

### 3.3 Who accesses what

Access is least privilege per S4 section 2 and T-1102a step 6, enforced with authoritative `_iam_binding` resources so no extra member can sneak in. The `api`, `unsub` and `worker` service accounts are the only application identities; `mt-tasks-invoker` and `mt-scheduler-invoker` carry OIDC identities only and hold no secret access. No application identity holds any `roles/logging.*` role (V16.4.2). James's project owner role covers the admin tool (T-507) and manual operations.

## 4. Deploy pipeline

The pipeline is: **build → test → deploy**, and deploys happen only from CI (S4 4, "Operational excellence": "deploys only from CI").

### 4.1 Build and test

Every pull request runs the required checks in S10 10.1: the eight existing checks (`rust`, `dart`, `language-policy`, `gitleaks`, `semgrep`, `cargo-audit`, `cargo-deny`, `dart-licenses`) plus `privacy-checks`, `ac-coverage`, `coverage` and `e2e`. The `terraform` job runs `fmt -check`, `init -backend=false`, `validate` and `test` for the module (T-1102a). Nightly: the live provider contract suite (T1), `cargo-mutants`, and the ZAP baseline against staging (S10 10.3). Weekly: the existing scheduled security workflow.

### 4.2 Deploy

- **Containers** are built in CI, scanned by Artifact Registry vulnerability scanning (S4 1), and pushed to the Artifact Registry Docker repo `mailtinder` in `us-central1` (T-1102a step 10). Cloud Run services are deployed from those images.
- **Infrastructure** is Terraform. James reviews the plan before each apply (S4 1, S13 1). `apply` runs from James's machine, never from CI, so a plan is always human-reviewed before it touches production.
- **After each deploy to staging**, the staging smoke tests run (S10 3.1): deployed services in the staging project with real Cloud Tasks, KMS, Firestore, and `unsub` refusing the metadata server as a one-click target. The ZAP baseline scan runs against staging (S10 7.4).
- **Firebase Hosting** serves the static Flutter web build with the security headers from `firebase.json` (S4 1).

### 4.3 Deploy identity

The deploy identity is a service account with only the roles needed to push images and update Cloud Run services (T-1102b). It holds no secret access and no KMS roles. The production-grade end state is Workload Identity Federation (GitHub OIDC + a Workload Identity Pool) with no long-lived keys; the interim service-account key for CI is a documented dev-stage measure to be replaced before production (gcp-setup.md).

## 5. Logging and monitoring (no mail content)

### 5.1 What may be logged

The only fields allowed in logs are the C1 fields in S5 "Logs and telemetry": request ID, pseudonymous user ID (HMAC of the user ID under the log pseudonymisation key), route template, status code, latency, action type, outcome code, rate-limit hit, `amr` on sign-in events, provider, and unsubscribe method. **Everything else is banned from logs**, including tokens, cookies, message IDs, addresses, names, subjects, snippets, bodies, URLs and page content (S5 "Logs and telemetry"). Security events (S6 7) live in the log bucket, not in Firestore.

Enforcement (S5 "Logs and telemetry", S6 8, S10 7.3):
- A redacting wrapper type in Rust.
- The template's `optional/privacy` Semgrep rules, extended with the S5 field names.
- A log-scanning test over integration test output (LOG-1): integration test logs contain no value from the fixture mail corpus.
- The leak tests (XC-01, FD-01 AC3, INV-1) scan captured `tracing` output, error report payloads and metric events for canary tokens, corpus addresses and unsubscribe URLs, and fail the build on any hit (S10 7.3).

### 5.2 Log storage and protection

- Logs go to a **locked Cloud Logging bucket** `mailtinder-logs` in `us-central1` with **90-day retention** (S4 1, S5 "Logs and telemetry", S6 7, V16.4.2). `locked = true` in production; staging passes `lock_log_bucket = false` (T-1102a step 8, T-1103).
- A sink routes `cloud_run_revision`, `cloud_tasks_queue` and `cloud_scheduler_job` logs to that bucket, and an exclusion with the same filter keeps `_Default` from holding a second 30-day copy (T-1102a step 8).
- **No application identity can delete or edit logs** (V16.4.2): no app service account holds any `roles/logging.*` role; Cloud Run writes stdout logs itself. Logs are transmitted to Cloud Logging, which is logically separate from the application, so a breach of the app does not compromise the logs (V16.4.3).
- Audit logs are on for KMS and Secret Manager data access, and `ADMIN_READ` for all services (T-1102a step 9), as evidence for the CASA assessor (S4 1).

### 5.3 Monitoring and alerting

Cloud Monitoring alerts (S4 1) are wired to page James. The alert set is owned by T-1107 (budget, monitoring and alerts); the alert conditions this spec requires are:

| Alert | Condition | Why |
| --- | --- | --- |
| Unrecoverable actions | Any non-zero value in the `undo_failed`, `unsub_after_undo` or `history_missing` counters (S10 8) | The trial's "zero unrecoverable actions" measure; pages James (S10 8, `[ASSUMES]`) |
| Jobs reaching Needs Attention by expiry | A job reaches `needs_attention` by expiry (S4 4, "Reliability") | Nothing fails silently |
| Billing budget | A set monthly figure (S4 1, S4 4 "Cost optimisation") | Stay in the free tier |
| TypeSafe and Vertex AI usage | Billing alerts on both (S4 5.7) | Bake-off cost control |
| Security Command Center findings | Any high-severity finding (S4 4 "Security, privacy and compliance": "findings at zero high") | Posture |

Metric events carry only content-free counters: event type, outcome code, pseudonymous user ID, mailbox provider, and time (S10 8). A test asserts the metric event type has no other fields.

### 5.4 Who reads what

- **Artifact Registry vulnerability scanning** runs in Google Cloud, not in CI (S4 1, S10 12). S11 assigns the reader: James reviews Artifact Registry vulnerability findings as part of the weekly security workflow (S10 10.3). `[ASSUMES]` — S10 12 flagged this as needing an owner; this spec assigns it to James.
- **Security Command Center** findings are reviewed by James in the weekly security workflow.
- **Logs** are read by James for incident investigation; no application identity reads them.

## 6. Backup

**The trial runs with no Firestore backups, point-in-time recovery or scheduled exports** (S6 9, decided James 4 October 2026; S5 DEL-2). This is deliberate: a backup copy of an encrypted field would survive crypto-shredding, so backups would break the deletion guarantee (S5 DEL-2, T-1102a "Waits on S11"). Terraform sets `point_in_time_recovery_enablement = POINT_IN_TIME_RECOVERY_DISABLED` and creates no backup schedule (T-1102a step 5).

The data that must survive is the user's app folder file on Google Drive (S5 "User app folder"), which the user controls and which the app can rebuild from labels if deleted. The KMS keys are the one thing that must not be lost: `data-key-kek` and `system-fields` are Cloud KMS software keys with yearly automatic rotation and old versions kept for unwrap/decrypt (S6 5). KMS keys are managed by Google and are not backed up by us.

**Open question:** whether production (post-trial) should add backups is not decided. Any future backup design must reconcile with crypto-shredding (S5 DEL-2) — a backup of encrypted fields is only acceptable if the wrapped `data_key` is not in it, or if deletion can still destroy the key. This is deferred; the trial has none.

## 7. ASVS rows owned by this spec

These register rows are not held by any other spec; S11 is their `Location` and provides the `review` evidence.

### 7.1 V13.1 Configuration documentation

**V13.1.1** — "Verify that all communication needs for the application are documented. This must include external services which the application relies upon and cases where an end user might be able to provide an external location to which the application will then connect."

The communication needs are documented in S4 section 1 (Services table) and the `HttpEgress` allowlist (S4 5.7, register V13.2.4). The full document is this spec. The external services the application relies on:

| Service | Direction | Used by | Notes |
| --- | --- | --- | --- |
| Gmail API | outbound | `api`, `unsub` | Provider; `unsub` for send and access-token minting |
| Google Drive API | outbound | `api` | App folder file; `unsub` has no Drive access |
| Google OAuth endpoints | outbound | `api`, `unsub` | Sign-in, token minting, certificate verification |
| Vertex AI (Gemini, `us-central1`) | outbound | `api` | Bake-off, consenting users only |
| TypeSafe Jev (`api.typesafe.ai`) | outbound | `api` | Bake-off, consenting users only |
| Firestore, KMS, Secret Manager, Cloud Tasks, Cloud Scheduler | outbound | `api`, `unsub`, `worker` | Google internal, IAM |
| Unsubscribe one-click targets | outbound | `unsub` | **End-user-provided location** (from a mail header); SSRF controls in S6 6 and S4 3.3 |
| Cloud Run `unsub`, `worker` | inbound | Cloud Tasks, Cloud Scheduler | OIDC tokens |
| Cloud Run `pagehandler` (v2) | inbound | `unsub` | v2 only; no-role service account |

The one case where an end user can provide an external location is the one-click unsubscribe target from a mail header. It is handled by the SSRF controls in S6 6 and S4 3.3: https only, host resolved and private/loopback/link-local/CGNAT/metadata ranges refused, resolved IP pinned, no redirects followed, 10-second timeout, no cookies or credentials. The page handler (v2) adds per-hop checks and Direct VPC egress.

### 7.2 V15.1.1 Remediation windows

**V15.1.1** — "Verify that application documentation defines risk based remediation time frames for 3rd party component versions with vulnerabilities and for updating libraries in general, to minimize the risk from these components."

Remediation windows for third-party component vulnerabilities, by severity:

| Severity | Remediation window | Source of the finding |
| --- | --- | --- |
| Critical | 7 days | cargo-audit, Dependabot, Artifact Registry scanning |
| High | 14 days | cargo-audit, Dependabot, Artifact Registry scanning |
| Medium | 30 days | cargo-audit, Dependabot, Artifact Registry scanning |
| Low | Next scheduled dependency update | cargo-audit, Dependabot |

`[TUNABLE]` — starting numbers. The windows are enforced by the existing CI checks (V15.2.1: "the application only contains components which have not breached the documented update and remediation time frames" is verified by cargo-audit, Dependabot and the Dart licence check). Dependabot runs with a cooldown (S6 T14). A component that cannot be updated within its window is recorded as a risk in the ASVS register with a dated review note and a plan.

### 7.3 V16.4 Log protection

**V16.4.1** — structured JSON logging with control characters escaped; a test asserts no log injection (register row, `test`).
**V16.4.2** — logs protected from unauthorised access and modification: locked bucket, 90 days, no delete for app identities (section 5.2).
**V16.4.3** — logs transmitted to a logically separate system (Cloud Logging) for analysis, detection, alerting and escalation (section 5.2, 5.3).

### 7.4 Alerting

Alerting is covered in section 5.3. The alert set is owned by T-1107; the conditions this spec requires are listed there. Alerts page James and carry no mail content (content-free counters only).

### 7.5 Incident handling

An incident is any of: a security event that indicates a breach or attempted breach (S6 7), a Security Command Center high finding, an unrecoverable-actions alert, a job reaching Needs Attention by expiry, or a billing anomaly.

Runbook (S4 4, "Operational excellence": "runbooks for failed jobs, key rotation and incident response"):

1. **Detect.** An alert fires (section 5.3) or James notices a finding in the weekly security workflow.
2. **Triage.** James determines severity. For a suspected data breach, follow section 7.6.
3. **Contain.** For a suspected breach: rotate the affected secrets (Secret Manager) and KMS keys (S6 5), and use the kill switches (`CLASSIFIER_JEV_ENABLED`, `CLASSIFIER_GEMINI_ENABLED` at start-up plus the Firestore `config/classifiers` document checked every minute, S4 5.7) to stop model calls. An admin can end a user's session (S6 4, V7.4.5) and, for all users, via this runbook.
4. **Investigate.** Read the locked log bucket (section 5.2). Logs carry only C1 fields, so the investigation can establish what happened without exposing mail content.
5. **Remediate.** Fix the root cause, deploy through the pipeline (section 4), and record the incident.
6. **Recover.** Confirm the fix in staging, then production.
7. **Post-incident.** Record the incident, the timeline, the root cause and the fix in the ASVS register or a dated review note.

**Key rotation runbook** (S6 5, T-1102a "Waits on S11"): KMS keys rotate yearly automatically; old versions are kept for unwrap/decrypt. Secret rotation is per the table in section 3.1. The runbook for a manual rotation is: add the new secret version in Secret Manager, confirm the services pick it up, then disable the old version after a grace period.

### 7.6 Notifiable data breach handling

MailTinder is an Australian product (APP 8 disclosure, S5 line 103), so a data breach that is likely to result in serious harm to an individual triggers the **Notifiable Data Breaches (NDB) scheme** under the Privacy Act 1988 (Cth).

The privacy design means the realistic breach surface is small: no mail content is at rest (S5), refresh tokens are envelope-encrypted under KMS (S6 5), and a Firestore export alone decrypts nothing (S5 DEL-2, S6 T1). The highest-value asset is mailbox access via refresh tokens (S6 1).

When a breach is suspected:

1. **Assess.** Determine whether the breach is likely to result in serious harm to any individual, considering the data involved (tokens, encrypted fields, C1 logs) and the controls that protect it (envelope encryption, crypto-shredding, no mail content at rest).
2. **Notify.** If serious harm is likely, notify affected individuals and the **Office of the Australian Information Commissioner (OAIC)** as soon as practicable, and in any case within 30 days of becoming aware (NDB scheme requirement). The notification describes what happened, what data was involved, and what the individual can do.
3. **Contain and remediate.** Follow section 7.5 steps 3 to 7.
4. **Record.** Keep a dated record of the assessment and notification for the OAIC and for the CASA assessor.

**Open question:** the exact OAIC notification template and the contact details are not in the existing specs. This spec states the NDB obligation and the 30-day window; the notification template is a James decision.

## 8. Decisions for James

| # | Decision | Proposed |
| --- | --- | --- |
| O1 | Who reads Artifact Registry vulnerability findings | James, in the weekly security workflow (S10 12 flagged this as needing an owner) |
| O2 | Remediation windows (V15.1.1) | Critical 7 days, High 14 days, Medium 30 days, Low next update `[TUNABLE]` |
| O3 | Production backups post-trial | Deferred; the trial has none (S6 9). Any future design must reconcile with crypto-shredding (S5 DEL-2) |
| O4 | OAIC notification template | Not in the specs; James to confirm the template and contact details |

## 9. Changes needed in other documents

- **ASVS register:** mark V13.1.1, V15.1.1, V16.4.2 and V16.4.3 as `review` rows with `Location` `docs/specs/S11`, and note the remediation windows in V15.1.1.
- **S10 12** (S4 section 1 Artifact Registry scanning): the "who reads its findings" question is answered here (O1).
- **T-1102a "Waits on S11"** items are resolved here: backups off (section 6), who may apply (section 2), key rotation runbook and log alert routing (sections 5.3 and 7.5).
