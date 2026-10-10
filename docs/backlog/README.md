# S12: Delivery Backlog

Status: DRAFT for James's review. 3 October 2026.
Written for a lower-cost build model (James, 3 October 2026). Each task file is self-contained: it names the files to touch, the types and signatures, a "good enough" algorithm, the acceptance criteria, and the tests that must pass. A builder reads its task file and only the spec sections the file cites.

How to work a task: see `docs/specs/S13-agent-working-rules.md`.

## Model tiers

- **sonnet** (default): the build model. Most tasks.
- **strong**: OAuth and sessions, KMS envelope encryption and sealed tokens, SSRF egress, the unsubscribe sender, model input redaction, HTML stripping, logging redaction, IAM in Terraform, account deletion. A strong task is built by the planning-tier model, or by the build model with a strong-model review before merge.

## Spec gaps the backlog covers

- **S8 (provider adapter contracts):** for Gmail, the M4 task files carry the per-call contract (calls, scopes, errors, label behaviour). Microsoft Graph is v2.
- **S11 (operations spec):** not yet drafted. M11 tasks marked "needs S11" wait for it. Everything else can start.

## Release gate

XC-05 (every ASVS Level 2 requirement mapped in S6 has a passing verification before release) is checked by `tools/check_ac_coverage.py` (T-002) with every register row enforced. Before the trial, the planning thread confirms the report shows no uncovered applicable row.

## Milestones

| Milestone | Theme | Can start after |
| --- | --- | --- |
| M0 | Foundations: workspace, CI gates, coverage, app shell | None |
| M1 | Domain core (pure Rust, no I/O) | T-001 |
| M2 | Ports, fakes and testkit | T-101, T-106, T-107 |
| M3 | Platform adapters (Firestore, KMS, Cloud Tasks, egress, logging) | T-202a, T-202b |
| M4 | Gmail adapter (S8 for Gmail) | T-203, T-205a, T-306 |
| M5 | API service, auth and sessions | M3 |
| M6 | Mailboxes, Feed, swipes, rules, filing | M5 |
| M7 | Unsubscribe service and Needs Attention | M6 |
| M8 | History, stats, account, admin | M6 |
| M9 | Classifier bake-off | M6 |
| M10 | Flutter app screens | T-007, matching API task |
| M11 | End to end, infrastructure and operations | varies |
| M12 | Release readiness: whole-codebase review | None (owner-initiated) |

## Task index

Generated from each task file's header (3 October 2026). Dependencies list only what must be merged first.

| ID | Title | Milestone | Tier | Depends on |
| --- | --- | --- | --- | --- |
| [T-001](T-001-cargo-workspace.md) | Cargo workspace and crate skeletons | M0 | sonnet | None |
| [T-002](T-002-ac-coverage-check.md) | AC coverage script and CI job | M0 | sonnet | T-001 |
| [T-003](T-003-privacy-layer.md) | Install the privacy layer and extend its rules | M0 | sonnet | T-001 |
| [T-004](T-004-coverage-floors.md) | Coverage job with floors | M0 | sonnet | T-001 |
| [T-005](T-005-claude-md-and-required-checks.md) | Update CLAUDE.md and required checks list | M0 | sonnet | T-002, T-003, T-004 |
| [T-006](T-006-flutter-web-csp-spike.md) | Flutter web CSP with Trusted Types spike | M0 | strong | T-007 |
| [T-007](T-007-flutter-app-shell.md) | Flutter app shell, routing and ApiClient | M0 | sonnet | None |
| [T-101](T-101-domain-types.md) | Domain identifiers, mailbox and message types | M1 | sonnet | T-001 |
| [T-102](T-102-header-rules.md) | Header facts and the HeaderRules classifier | M1 | sonnet | T-101 |
| [T-103](T-103-header-guard.md) | Header guard | M1 | sonnet | T-102 |
| [T-104](T-104-sort-rules.md) | Sort rules: reject, block and filing matching | M1 | sonnet | T-102 |
| [T-105a](T-105a-swipe-effects.md) | Swipe effects by class | M1 | sonnet | T-102, T-103, T-104 |
| [T-105b](T-105b-undo.md) | Undo plan and the undo stack | M1 | sonnet | T-105a |
| [T-106](T-106-unsubscribe-job-state-machine.md) | Unsubscribe job state machine | M1 | sonnet | T-101 |
| [T-107](T-107-state-machines.md) | Needs Attention, invite and mailbox state machines | M1 | sonnet | T-101 |
| [T-108](T-108-feed-ordering.md) | Feed ordering, skips, phases, levels and bosses | M1 | sonnet | T-101 |
| [T-109](T-109-gamification-calculators.md) | Gamification calculators | M1 | sonnet | T-101 |
| [T-201a](T-201a-port-traits.md) | Port traits (everything except ServerStore) | M2 | sonnet | T-101 |
| [T-201b](T-201b-server-store-traits.md) | ServerStore repository traits and records | M2 | sonnet | T-106, T-107, T-201a |
| [T-202a](T-202a-in-memory-store-and-contract-suite.md) | In-memory ServerStore and the ServerStore contract suite | M2 | sonnet | T-201b |
| [T-202b](T-202b-fakes-ports-and-virtual-clock.md) | Fakes for every other port, the Ports struct and the virtual clock | M2 | sonnet | T-201a, T-202a |
| [T-203](T-203-fake-mailbox-and-contract-suite.md) | FakeMailbox and the MailProvider and AppFolderStore contract suites | M2 | sonnet | T-202b |
| [T-204](T-204-synthetic-mail-corpus.md) | Synthetic mail corpus and fixture loader | M2 | sonnet | T-101, T-203 |
| [T-205a](T-205a-fake-google-gmail.md) | fake-google, part 1: Gmail REST fake and control API | M2 | sonnet | T-203, T-204 |
| [T-205b](T-205b-fake-google-drive.md) | fake-google, part 2: Drive appDataFolder | M2 | sonnet | T-205a |
| [T-206](T-206-fake-google-oauth-oidc.md) | fake-google, part 3: OAuth and OIDC identity fake | M2 | sonnet | T-205a |
| [T-301](T-301-firestore-server-store.md) | Firestore ServerStore adapter | M3 | sonnet | T-202a, T-202b |
| [T-302](T-302-kms-envelope-data-key.md) | KMS envelope encryption and the per-user data key | M3 | strong | T-202b, T-301 |
| [T-303](T-303-sealed-tokens.md) | Sealed tokens | M3 | strong | T-302 |
| [T-304](T-304-cloud-tasks-scheduler.md) | Cloud Tasks JobScheduler | M3 | sonnet | T-202b, T-301 |
| [T-305](T-305-secret-manager-and-classifier-config.md) | Secret Manager and the classifier config document | M3 | sonnet | T-202b, T-301 |
| [T-306](T-306-http-egress-ssrf.md) | HttpEgress: per-service allowlist and SSRF checks | M3 | strong | T-202b, T-307, T-708 |
| [T-307](T-307-structured-logging-redaction.md) | Structured logging with redaction | M3 | strong | T-001, T-201a |
| [T-401](T-401-gmail-read-messages-and-headers.md) | Gmail adapter: read messages and headers | M4 | sonnet | T-203, T-205a, T-306 |
| [T-402](T-402-html-to-plain-text.md) | Server-side HTML to plain text | M4 | strong | T-401 |
| [T-403](T-403-gmail-trash-labels-spam-restore.md) | Gmail adapter: trash, labels, spam and exact restore | M4 | sonnet | T-401 |
| [T-404](T-404-gmail-send-validated-mailto.md) | Gmail adapter: send with validated mailto | M4 | strong | T-401, T-403 |
| [T-405](T-405-drive-app-folder-store.md) | Drive app folder store | M4 | sonnet | T-205b, T-302, T-306, T-401 |
| [T-406](T-406-dkim-header-coverage-check.md) | DKIM header coverage check | M4 | strong | T-401, T-404 |
| [T-500](T-500-api-skeleton-router-errors-headers-rate-limits.md) | api service skeleton: router, errors, headers, rate limits | M5 | sonnet | T-202a, T-202b, T-307 |
| [T-500b](T-500b-api-binary-wiring-and-e2e-config.md) | api binary: production wiring and the end-to-end test configuration | M5 | strong | T-500, T-701, T-205a, T-205b, T-206, T-301 |
| [T-500c](T-500c-api-startup-hardening.md) | api start-up hardening: follow-ups from the T-500b reviews | M5 | sonnet | T-500b |
| [T-501](T-501-sessions-cookie-and-csrf.md) | Sessions, cookie and CSRF | M5 | strong | T-201b, T-202a, T-301, T-302, T-500 |
| [T-502a](T-502a-google-identity-adapter.md) | Google identity adapter (OAuth code flow, ID token validation, refresh, revoke) | M5 | strong | T-201a, T-206, T-305, T-306 |
| [T-502b](T-502b-google-sign-in-and-invite-redemption.md) | Google sign-in and invite redemption (API-AUTH-1, API-AUTH-2) | M5 | strong | T-107, T-206, T-501, T-502a, T-503 |
| [T-503](T-503-refresh-token-storage-and-access-token-minting.md) | Refresh token storage and access token minting | M5 | strong | T-301, T-302, T-502a |
| [T-504](T-504-step-up-by-fresh-google-sign-in.md) | Step-up by fresh Google sign-in | M5 | strong | T-502b |
| [T-505](T-505-invites-and-invite-requests.md) | Invites and invite requests | M5 | sonnet | T-107, T-303, T-404, T-504 |
| [T-506](T-506-session-endpoint-and-sign-out.md) | Session endpoint and sign-out | M5 | sonnet | T-501, T-503, T-504 |
| [T-507](T-507-admin-bootstrap-cli.md) | Admin bootstrap command line tool | M5 | sonnet | T-301, T-302, T-305, T-307, T-505 |
| [T-601a](T-601a-mailboxes-list-and-link.md) | Mailboxes: list, link and reconnect | M6 | sonnet | T-405, T-503, T-504 |
| [T-601b](T-601b-mailbox-disconnect.md) | Mailbox disconnect with app folder move | M6 | sonnet | T-304, T-405, T-601a |
| [T-602a](T-602a-mail-query-count-and-label-ports.md) | MailProvider query, count and label additions | M6 | sonnet | T-203, T-205a, T-401 |
| [T-602b](T-602b-user-state-file.md) | User state file: schema, sealed load and If-Match update | M6 | sonnet | T-101, T-104, T-108, T-302, T-405, T-503 |
| [T-602c](T-602c-feed-endpoint.md) | Feed endpoint | M6 | sonnet | T-103, T-108, T-303, T-402, T-601a, T-602a, T-602b |
| [T-603](T-603-progress-endpoint.md) | Progress endpoint | M6 | sonnet | T-108, T-602a, T-602c |
| [T-604](T-604-swipes-keep-skip-file-undo.md) | Swipes: keep, skip, file and their undo | M6 | sonnet | T-105a, T-105b, T-403, T-602c |
| [T-605](T-605-reject-swipe.md) | Reject swipe: trash, rules, block prompts and queued unsubscribe | M6 | sonnet | T-104, T-106, T-109, T-304, T-406, T-601b, T-602a, T-604 |
| [T-606](T-606-undo-reject-and-job-race.md) | Undo of a reject and the race with the job | M6 | sonnet | T-105b, T-601b, T-605 |
| [T-607a](T-607a-categories-endpoints.md) | Categories endpoints | M6 | sonnet | T-602a, T-604 |
| [T-607b](T-607b-filing-suggestions.md) | Filing suggestions and the keep prompt | M6 | sonnet | T-104, T-602c, T-607a |
| [T-608](T-608-rules-endpoints.md) | Rules endpoints and block prompt decline | M6 | sonnet | T-605, T-607a |
| [T-609](T-609-rules-at-feed-load-and-history-catch-up.md) | Rules applied at Feed load and History catch-up | M6 | sonnet | T-405, T-602c, T-608 |
| [T-701](T-701-unsub-service-skeleton.md) | unsub service skeleton and token minting | M7 | strong | T-106, T-503 |
| [T-702](T-702-one-click-unsubscribe-sender.md) | One-click unsubscribe sender | M7 | strong | T-306, T-406, T-701, T-708 |
| [T-703](T-703-mailto-unsubscribe-sender.md) | Mailto unsubscribe sender | M7 | strong | T-404, T-701 |
| [T-704](T-704-https-only-needs-attention.md) | Https-only links to Needs Attention | M7 | sonnet | T-605, T-701 |
| [T-705](T-705-needs-attention-endpoints.md) | Needs Attention endpoints | M7 | sonnet | T-107, T-500 |
| [T-706](T-706-worker-service-and-sweeps.md) | worker service and sweeps | M7 | sonnet | T-701, T-705 |
| [T-707](T-707-delivery-check-and-confirmed-unsubscribes.md) | Delivery check and confirmed unsubscribes | M7 | sonnet | T-602b, T-609, T-706 |
| [T-708](T-708-unsub-testbed.md) | unsub-testbed local sites | M7 | sonnet | T-001 |
| [T-801](T-801-history-endpoint.md) | History endpoint | M8 | sonnet | T-609 |
| [T-802](T-802-stats-and-achievements.md) | Stats endpoint and achievement unlocking | M8 | sonnet | T-109, T-603, T-605, T-608, T-707 |
| [T-803](T-803-delete-account.md) | Delete account | M8 | strong | T-302, T-504, T-601b, T-602b |
| [T-804](T-804-admin-users-and-end-session.md) | Admin users and ending a session | M8 | sonnet | T-501, T-504 |
| [T-901](T-901-classifier-wiring-and-sealed-classification.md) | Classifier trait wiring and sealed classification | M9 | sonnet | T-103, T-602c, T-604 |
| [T-902](T-902-experiments-consent.md) | Experiments consent | M9 | sonnet | T-500 |
| [T-903](T-903-model-input-redaction.md) | Model input redaction | M9 | strong | T-402, T-901 |
| [T-904](T-904-gemini-classifier-vertex.md) | Gemini classifier on Vertex AI | M9 | sonnet | T-901, T-903 |
| [T-905](T-905-jev-classifier.md) | Jev classifier | M9 | sonnet | T-901, T-903 |
| [T-906a](T-906a-evaluation-records.md) | Evaluation records | M9 | sonnet | T-305, T-904, T-905, T-907 |
| [T-906b](T-906b-kill-switches-and-admin-switch-endpoints.md) | Kill switches and admin switch endpoints | M9 | sonnet | T-305, T-504, T-902, T-906a |
| [T-907](T-907-bakeoff-statistics-library.md) | Bake-off statistics library | M9 | sonnet | T-101 |
| [T-908a](T-908a-bakeoff-report-assembly.md) | Bake-off report assembly | M9 | sonnet | T-906a, T-907 |
| [T-908b](T-908b-bakeoff-endpoints-csv-and-snapshots.md) | Bake-off endpoints, CSV and snapshots | M9 | sonnet | T-504, T-906a, T-908a |
| [T-1001a](T-1001a-sign-in-and-request-invite.md) | Sign-in, invite link and Request invite screens | M10 | sonnet | T-007 |
| [T-1001b](T-1001b-confirm-its-you-step-up.md) | Confirm it's you (step-up overlay) | M10 | sonnet | T-1001a |
| [T-1002a](T-1002a-feed-cards-and-states.md) | Feed screen, cards and Feed states | M10 | sonnet | T-007, T-1001a |
| [T-1002b](T-1002b-swipes-toasts-undo.md) | Swipes, buttons, toasts, undo and the block prompt | M10 | sonnet | T-1002a |
| [T-1003](T-1003-filing-sheet.md) | Filing sheet and keep-learning prompt | M10 | sonnet | T-1002b, T-1004 |
| [T-1004](T-1004-filed-tab.md) | Filed tab | M10 | sonnet | T-007, T-1001a |
| [T-1005](T-1005-needs-attention-tab.md) | Needs Attention tab | M10 | sonnet | T-007, T-1001a |
| [T-1006a](T-1006a-settings-accounts-and-account.md) | Settings home, Connected accounts and Account | M10 | sonnet | T-1001b |
| [T-1006b](T-1006b-settings-history-rules-stats.md) | Settings: History, Rules and Stats | M10 | sonnet | T-1004, T-1006a |
| [T-1007a](T-1007a-experiments-and-admin-screens.md) | Experiments and Admin screens | M10 | sonnet | T-1006a |
| [T-1007b](T-1007b-bakeoff-report-screen.md) | Bake-off report screen | M10 | sonnet | T-1007a |
| [T-1008a](T-1008a-progress-and-round-ui.md) | Progress UI: inbox meter, levels, bosses, round card and celebrations | M10 | sonnet | T-1002b, T-1006b |
| [T-1008b](T-1008b-swipe-feedback-effects.md) | Swipe feedback: animations, sounds, haptics, combo and confetti | M10 | sonnet | T-1006a, T-1008a |
| [T-1009](T-1009-blitz-mode.md) | Blitz mode | M10 | sonnet | T-1008a |
| [T-1010](T-1010-on-device-category-name.md) | On-device category name proposal | M10 | sonnet | T-1003 |
| [T-1101a](T-1101a-e2e-harness-and-required-check.md) | End-to-end harness, first journey and the `e2e` required check | M11 | sonnet | M6, M7, T-205a, T-205b, T-206, T-500b, T-708, T-1001a, T-1002b |
| [T-1101b](T-1101b-e2e-journeys-triage-and-unsubscribe.md) | End-to-end helpers and the sessions journey | M11 | sonnet | T-1003, T-1006a, T-1101a, T-1112c |
| [T-1101c](T-1101c-e2e-journeys-and-leak-scans.md) | End-to-end journeys: Needs Attention, disconnect, delete account; leak, storage and CSP scans | M11 | sonnet | T-006, T-1005, T-1101b, T-1101d, T-1101e, T-1101f |
| [T-1101d](T-1101d-e2e-journey-mixed-feed.md) | End-to-end journey: mixed Feed across two mailboxes | M11 | sonnet | T-1101b |
| [T-1101e](T-1101e-e2e-journeys-reject-undo-and-unsubscribe.md) | End-to-end journeys: reject then undo, reject and unsubscribe | M11 | sonnet | T-1101b, T-1101g |
| [T-1101f](T-1101f-e2e-journey-file.md) | End-to-end journey: file a message | M11 | sonnet | T-1101b |
| [T-1101g](T-1101g-e2e-unsub-stack-virtual-clock-and-testbed-trust.md) | End-to-end unsubscribe stack: virtual clock and testbed trust | M11 | strong | T-500b, T-1101a, T-1101b |
| [T-1102a](T-1102a-terraform-prod-foundation.md) | Terraform, production foundation: service accounts, IAM, KMS, Firestore, secrets, logs | M11 | strong | needs S11 |
| [T-1102b](T-1102b-terraform-prod-runtime-and-deploy-identity.md) | Terraform, production runtime: Cloud Run, Cloud Tasks, Scheduler, Hosting and the deploy identity | M11 | strong | T-1102a, needs S11 |
| [T-1103](T-1103-terraform-staging-project.md) | Terraform, staging project | M11 | sonnet | T-1102b |
| [T-1104](T-1104-build-and-deploy-pipeline.md) | Build and deploy pipeline | M11 | sonnet | T-006, T-1103, needs S11 |
| [T-1105](T-1105-staging-smoke-tests-and-zap.md) | Staging smoke tests and the ZAP baseline | M11 | sonnet | T-301, T-302, T-304, T-1104 |
| [T-1106](T-1106-nightly-live-gmail-contract-run.md) | Nightly live Gmail contract run | M11 | sonnet | T-203, T-401, T-403, T-1103 |
| [T-1107](T-1107-budget-monitoring-and-alerts.md) | Budget, monitoring and alerts | M11 | sonnet | T-307, T-1102b, T-1114, needs S11 |
| [T-1109](T-1109-mutation-testing-pilot.md) | Mutation testing pilot | M11 | sonnet | T-004, T-104, T-105a, T-109 |
| [T-1110](T-1110-property-tests-hostile-input.md) | Property tests for every parser of hostile input | M11 | sonnet | T-102, T-306, T-406, T-701 |
| [T-1111](T-1111-visual-and-accessibility-regression.md) | Screenshot and accessibility regression tests | M11 | sonnet | T-1001a, T-1002b, T-1003, T-1005, T-1006a |
| [T-1111a](T-1111a-pin-flutter-version-in-ci.md) | Pin the Flutter version CI uses | M11 | sonnet | T-1111 |
| [T-1112a](T-1112a-unsub-e2e-startup-mode.md) | unsub service e2e start-up mode | M11 | strong | T-500b, T-500c, T-701, T-708 |
| [T-1112b](T-1112b-local-job-runner.md) | Local job runner for e2e mode | M11 | strong | T-500b, T-500c |
| [T-1112c](T-1112c-reject-reaches-unsubscribe.md) | Wire the runner; reject -> unsubscribe proof | M11 | strong | T-1112a, T-1112b |
| [T-1112d](T-1112d-unsub-no-testkit-ci-gate.md) | Run the unsub no-testkit start-up refusal test in CI | M11 | sonnet | T-1112a |
| [T-1108a](T-1108a-demo-start-stop-check.md) | demo.sh start, stop, status, check | M11 | sonnet | T-500b, T-500c, T-1101a |
| [T-1108b](T-1108b-demo-seed.md) | Demo seed: fake mailbox and invite URL | M11 | sonnet | T-1108a |
| [T-1108c](T-1108c-demo-phone-access.md) | demo --phone: HTTPS tunnel and access code | M11 | sonnet | T-1108a |
| [T-1113](T-1113-front-end-hot-reload-dev-mode.md) | Front-end hot-reload dev mode for the demo | M11 | sonnet | T-1108a, T-1108c |
| [T-1114](T-1114-emit-swipe-undo-metric-events.md) | Emit the swipe, undo and unrecoverable-action metric events | M11 | sonnet | T-307, T-1101b, T-1101e, T-1101f |
| [T-1201](T-1201-complete-code-review.md) | Complete independent code review (cross-model, whole codebase) | M12 | strong | None (owner-initiated) |

Task files live beside this index as `T-xxx-short-name.md`.
