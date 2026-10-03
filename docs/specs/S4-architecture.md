# S4: Architecture

Status: DRAFT for James's review. 3 October 2026.
Brief (James): low-cost serverless hosting that complies with the appropriate well-architected framework. OWASP ASVS Level 2 is mandatory. Gmail first; Microsoft follows behind the same adapter.
Scope (James, 3 October 2026, spec audit Q2 and Q3): v1 builds Gmail only; Microsoft (Graph, Outlook, OneDrive) is v2. Provider abstraction is a requirement: the `MailProvider` and `AppFolderStore` adapter traits and a provider-neutral domain mean adding Microsoft or another provider needs a new adapter, not refactoring. No Gmail types outside the adapter (XC-02); the provider enum and mailbox key design stay generic. The page handler moves to v2 with the open-ended agent.
Decided: **Google Cloud, Iowa region (`us-central1`)** for every regional service (Cloud Run, Cloud Tasks, Firestore, KMS, Secret Manager, Vertex AI, log buckets), assessed against the **Google Cloud Well-Architected Framework** (James, 3 October 2026). Region rationale (James, 3 October 2026): Gmail data is not held in Sydney, so the server sits near Google's US infrastructure. Every Gmail API call is then short, and only the browser's own request crosses the Pacific.
Depends on: `S2`, `S3`, `S9`, `research/data-handling-and-in-account-ai.md`, `research/unsubscribe-feasibility.md`, `research/cheap-email-classification.md`.

## 1. Services

| Need | Google Cloud service | Notes |
| --- | --- | --- |
| Static Flutter web app | Firebase Hosting | Free tier; security headers (CSP, HSTS and others) set in hosting config |
| API | Cloud Run service `api` (Rust container), request-based billing, scale to zero | Free tier: 2 million requests, 180,000 vCPU-seconds, 360,000 GiB-seconds a month, same in every region |
| Delayed, cancellable unsubscribe jobs | Cloud Tasks queue with `scheduleTime` = now plus 5 minutes; task deleted on undo | Task calls the `unsub` service with an OIDC token |
| Job execution | Cloud Run service `unsub` (Rust) for one-click and mailto | Ingress internal only; invoked by Cloud Tasks |
| Page handler (v2) | Cloud Run service `pagehandler` (container with headless Chromium) | v2 only, with the open-ended agent. Own service account with no roles; ingress internal; only `unsub` may invoke |
| Data store | Firestore (Native mode) in `us-central1` (location is permanent once created) with TTL policies | Free tier: 1 GiB, 50,000 reads and 20,000 writes a day. TTL deletes within about 24 hours, so a sweeper is the control and TTL the backstop |
| Encryption | Cloud KMS, one symmetric key wrapping each user's `data_key` (S6 section 5) | USD 0.06 a key version a month plus USD 0.03 per 10,000 operations |
| Secrets | Secret Manager (OAuth client secrets, Jev API key, email lookup and log pseudonymisation HMAC keys) | Jev key: `api` only accessor, quarterly rotation |
| Classifier bake-off | Vertex AI (Gemini Flash-Lite, `us-central1`) and TypeSafe Jev API | Opt-in pilot users only (section 5) |
| Email (invites) | Gmail API send from the admin's own mailbox | Uses the send scope already approved for mailto unsubscribe; no third-party email service |
| Scheduled sweeps | Cloud Scheduler to Cloud Run service `worker` (internal ingress) | Kept off `api`, whose `run.app` URL must accept public traffic for Firebase Hosting rewrites |
| Observability | Cloud Logging (structured, content-free, 90-day retention bucket), Cloud Monitoring alerts, Error Reporting with scrubbed payloads | |
| Audit and posture | Cloud Audit Logs, Security Command Center (Standard tier), Organisation Policy constraints | Evidence for the CASA assessor |
| Edge protection | Trial: application-level rate limiting and request size limits in `api`. Before production: decide on Cloud Armor (needs an external load balancer, which adds a fixed monthly cost) | Recorded as an open decision |
| Container images | Artifact Registry with vulnerability scanning | |
| Infrastructure as code | Terraform | Decided (James, 3 October 2026). Agents write it; James reviews the plan before each apply. One `region` variable, set to `us-central1`, used by every resource; Firestore location set explicitly |

Expected running cost at trial scale (about 20 users): roughly zero to a few dollars a month, mostly KMS and Artifact Registry storage. Estimate only; confirm with the Google Cloud pricing calculator once IaC exists, and set a billing budget alert. v1 needs no Direct VPC egress or Cloud NAT, because the page handler is v2; add their fixed cost to the estimate when it ships.

### Environments

- **Production** and **staging** are separate Google Cloud projects built from the same Terraform, each with its own state (T5, decided 3 October 2026). Staging runs the post-deploy smoke tests and the ZAP baseline scan. S11 holds the details.
- **Sandbox mailboxes** (T1, decided 3 October 2026): one Gmail account (v1) and one Outlook.com account (v2, with Microsoft), owned by James, holding synthetic mail only. Their OAuth secrets live in GitHub Actions for the nightly live contract run. While the app is in Google Testing mode the Gmail sandbox needs re-authorising weekly, a recurring task for James.

## 2. Component view

```
Browser (Flutter web, Gemini Nano when available)
   │  HTTPS, session cookie
   ▼
Firebase Hosting (static app, security headers)
   │  /api/** rewrite
   ▼
Cloud Run: api (Rust)
   ├── auth (Google OAuth sign-in, Microsoft in v2; sessions; invites)
   ├── feed (reads mail through provider adapters, builds cards in memory)
   ├── actions (trash, label, spam report, undo)
   ├── jobs (creates and deletes Cloud Tasks)
   └── provider adapters (trait; gmail in v1, graph in v2)
   │
   ├──> Firestore (users, mailboxes, invites, jobs, Needs Attention, sessions)
   ├──> Cloud KMS (wrap and unwrap per-user `data_key`)
   ├──> Secret Manager
   └──> Gmail API, Google Drive app folder

Cloud Tasks (one task per job, scheduled) ──> Cloud Run: unsub (Rust)
   ├── one-click POST; mailto via Gmail send
   └── (v2) calls Cloud Run: pagehandler (Chromium; no-role service account)

Cloud Scheduler ──> Cloud Run: worker /internal/sweep (expired jobs, Needs Attention TTL)
```

Each Cloud Run service has its own service account with only the roles it needs:

| Service | Roles |
| --- | --- |
| `api` | Firestore user, KMS encrypter and decrypter on the one key, Secret Manager accessor on the OAuth client secrets, the Jev API key and the HMAC keys (email lookup, log pseudonymisation), `roles/aiplatform.user` (Vertex AI), Cloud Tasks enqueuer and deleter on the one queue |
| `unsub` | Firestore user, KMS encrypter and decrypter on the one key, Secret Manager accessor on the OAuth client secrets (to mint access tokens) and the log pseudonymisation HMAC key; no Drive access; in v2, Cloud Run invoker on `pagehandler` |
| `pagehandler` (v2) | None |
| `worker` | Firestore user, KMS encrypter and decrypter on the one key, Secret Manager accessor on the OAuth client secrets and the log pseudonymisation HMAC key |
| Cloud Tasks and Cloud Scheduler callers | Cloud Run invoker on their target only |

Exactly three service accounts (`api`, `unsub`, `worker`) hold KMS encrypt and decrypt (3 October 2026, spec audit).

## 3. Key flows

### 3.1 Sign-in

Passkey lock dropped for the trial (James, 3 October 2026); it returns in v2 as pre-CASA hardening. Keys follow S6 section 5: one per-user `data_key`, wrapped by Cloud KMS and usable by the server, encrypts refresh tokens, the app folder file, sealed tokens and encrypted fields.

1. The browser starts Google OAuth (authorisation code with PKCE, `state` and `nonce`, held on a `pre_auth` session record) through `api`.
2. `api` exchanges the code and validates the ID token (signature, `iss`, `aud`, `exp`, `nonce`).
3. **First sign-in (new user):** requires the single-use invite token from the invite email, `email_verified` true and an email that matches the invite. `api` creates the user with a new `data_key`, links the mailbox keyed by Google `sub` (v2 Microsoft rule: `tid` plus `oid`, and `xms_edov` true) as the primary mailbox whose Drive holds the app folder, and stores the refresh token encrypted under `data_key`.
4. **Existing user:** signs in with any linked Gmail mailbox, matched by Google `sub`. There is one pathway, so every sign-in has the same strength; account recovery is Google's.
5. One session per user: a new sign-in deletes the user's previous session record. The session cookie is named `__session` (Firebase Hosting forwards no other cookie to Cloud Run), with HttpOnly, Secure, SameSite=Lax, Path=/, no Domain attribute, rotated on every state change. Idle 15 minutes, absolute 12 hours.
6. Step-up: a fresh Google sign-in within the last 5 minutes (`prompt=login`, `max_age=300`, `auth_time` checked) before any account, mailbox or admin change (delete account, link or disconnect a mailbox, every admin write including kill switches and snapshots).
7. `api` sets its own security headers on every response, because its `run.app` URL can be reached without passing through Firebase Hosting.

### 3.2 Feed

1. Browser asks `api` for the next page of cards.
2. `api` loads the user's app folder file (rules, cursor) from the primary mailbox's Google Drive `appDataFolder` (one per user; other mailboxes' Drives unused; OneDrive in v2), decrypts it, queries each mailbox through its adapter, applies sort rules, scores each message with header rules (the badge) and, for consenting pilot users, with Gemini and Jev in parallel (section 5), and seals each card's classification, and returns cards. On the first Feed load after unsubscribe jobs finish, `api` appends their outcomes from the job records to History (an `If-Match` write) and deletes those records.
3. Nothing about the messages is written to Firestore or logs. The browser may run Gemini Nano on a card for a better score or a category name.

### 3.3 Reject with unsubscribe

1. `api` trashes the message, writes an `UnsubscribeJob` (encrypted target, `expires_at`; no access token) and creates a Cloud Task named after the job ID with `scheduleTime` five minutes ahead.
2. Undo before the due time deletes the task and the job.
3. At the due time Cloud Tasks calls `unsub`, which runs the job (S2 UN-01 to UN-05), mints an access token from the mailbox's stored refresh token, runs the job (S2 UN-01 to UN-05), writes the outcome to the job record and a pseudonymous security log entry. `unsub` has no Drive access; `api` appends the outcome to History at the next Feed load (section 3.2). If the refresh token is revoked or invalid, the job ends `failed` with a Needs Attention item "Sign in again". Background mailbox work is limited to queued unsubscribe jobs.
4. Cloud Tasks retries with backoff on failure (`maxAttempts` 4; the only retry layer, S3); after the final retry, `unsub` (or the sweeper) moves the job to Needs Attention so nothing fails silently.
5. One-click POST egress controls on `unsub` (v1): the target is an https URL from a DKIM-covered header. `unsub` resolves the host and refuses private, loopback, link-local, CGNAT and metadata ranges (including `169.254.169.254` and `metadata.google.internal`), pins the resolved IP for the connection to defeat DNS rebinding, uses https only, a 10-second timeout, no cookies or credentials and a fixed body. Application-level resolution plus IP pinning is the control; v1 has no Direct VPC egress.
6. The one-click POST follows no redirects. A 3xx response raises a Needs Attention item and is not retried.
7. In v1 an https-only `List-Unsubscribe` (no one-click) creates no job: it raises a Needs Attention item with "Open unsubscribe page" for the user, linking only to the URL from the DKIM-covered header.

### 3.4 Page handler isolation (v2)

The page handler ships in v2 with the open-ended agent. It keeps the `unsub` checks in step 3.3.5 plus its own controls:

- Separate Cloud Run service, service account with no roles, internal ingress, invoked only by `unsub`.
- Receives only the URL and, for "enter your email" forms, the user's address. Never receives tokens.
- Before every request and redirect, resolves the host and refuses private, loopback, link-local and Google metadata addresses (`169.254.169.254`, `metadata.google.internal`). In v2, egress also goes through Direct VPC egress with firewall rules denying private ranges.
- Even if a page reached the metadata server, the token it got would carry no permissions.
- 60-second timeout, fresh browser context per run, no persistent storage. Page content is data only.

## 4. Well-Architected mapping

Google Cloud Well-Architected Framework pillars.

| Pillar | How this design meets it | Verification |
| --- | --- | --- |
| Operational excellence | Infrastructure as code; CI builds, scans and deploys containers; structured logs; runbooks for failed jobs, key rotation and incident response | Pipeline checks; deploys only from CI |
| Security, privacy and compliance | One service account per service with least privilege; KMS envelope encryption and crypto-shredding; no mail content at rest; Audit Logs; Security Command Center; ASVS L2 mapping (S6) | ASVS test suite; Semgrep and privacy rules in CI; Security Command Center findings at zero high |
| Reliability | Managed regional services; idempotent jobs keyed by job ID; Cloud Tasks retries; every job reaches a terminal state within its TTL | Failure injection tests on adapters; alert on jobs reaching Needs Attention by expiry |
| Cost optimisation | Request-based billing, scale to zero, free tiers, budget alerts | Billing budget alert at a set monthly figure |
| Performance optimisation | Rust services with fast start; in-memory classification; card pages fetched ahead; swipes optimistic in the client, since a browser round trip from Australia to `us-central1` is about 200 ms | Latency tests (filing suggestion under 200 ms) |
| Sustainability | Scale to zero; minimal stored data | Covered by the above |

## 5. Classifier plug-in and Gemini versus Jev bake-off

Source: `docs/change-requests/CR-01-pluggable-classifier-jev-pilot.md` (revised after James's clarification, applied 3 October 2026). James wants every email scored by both Google's classifier and Jev, with his swipes judging which is more accurate, starting with his own backlog. Evidence on Jev: `research/jev-classifier-evaluation.md`. This amends D9: no server-side model in v1 except the opt-in bake-off.

### 5.1 Classifier trait

```
trait Classifier {
    fn id(&self) -> ClassifierId;            // "header_rules@1", "gemini@flash-lite", "jev@1.13.0"
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}

Classification { class, bulk_score, bulk_reason, confidence, probabilities }
```

| Implementation | Where | Role |
| --- | --- | --- |
| `HeaderRules` | `api`, in memory | Default. Always runs. Drives the card badge during the bake-off |
| `GeminiClassifier` | Vertex AI, `us-central1` regional endpoint (near Gmail data; regional so processing location and zero-retention settings are fixed), Gemini Flash-Lite by default | Bake-off model ("Google's classifier"). Five-class enum through `responseSchema`; token log probabilities as confidence |
| `JevClassifier` | `POST https://api.typesafe.ai/v1/systemone`, pinned to `jev-1.13.0` | Bake-off model. `class` Choice (fixed option order) and `bulk` Score in one request |
| Gemini Nano | Browser | On-device plan for users who do not opt in; left out of the bake-off because it varies by device |

Both bake-off models get identical input and identical question wording. Responses parse into typed structs with `deny_unknown_fields`; class must be one of the five enums; probabilities finite and in 0 to 1.

### 5.2 Header guard (veto)

Runs after any classifier and clamps the result to what the headers prove:

| Header fact | Guard |
| --- | --- |
| No valid `List-Unsubscribe`, or DKIM does not cover it | Class cannot be `list`; no unsubscribe job is ever created |
| Header rules say `personal` with high confidence | A model alone may not move it to `list` or `suspect`; the disagreement is recorded |
| Model error, timeout or invalid output | Failure recorded for that model; the card is unaffected |

During the bake-off neither model drives any action. The guard matters once a winner is promoted to the badge.

### 5.3 Sealed classification on the card

`feed/next` returns a server-sealed classification token (AEAD over message ID, mailbox ID, badge class, each model's prediction, classifier IDs, expiry). It uses the one sealing scheme for all server-issued tokens in S6 section 5: AES-256-GCM under the user's `data_key`, with associated data holding token type, user ID, session record ID and expiry. There is no separate classification token key. The swipe handler verifies it, re-reads the headers from the provider and re-applies the header guard. Models are not re-called at swipe time.

### 5.4 Bake-off

- For every consenting user, Gemini and Jev classify every card in parallel. No user split.
- The card shows the header-rules badge, not either model's answer, so the swipe is an unbiased label for both.
- Optional later phase: show the leading model's badge to measure whether it speeds up sorting.
- Users who have not consented never reach Gemini or Jev.

### 5.5 What goes to the models

Built in memory, never stored:

- Header allowlist: `From` display name and domain, `List-Id`, `List-Unsubscribe` presence (not the URL), `List-Unsubscribe-Post`, `Precedence`, `Auto-Submitted`, ESP fingerprint header names, summarised `Authentication-Results`.
- Subject.
- Stripped plain text truncated to about 500 tokens, with URLs, email addresses and long digit runs replaced by placeholders.
- Never: `To`, `Cc`, recipient addresses, attachments, message IDs, the unsubscribe URL.

### 5.6 Evaluation record and report

Firestore `classifier_eval` (C1, TTL 180 days `[TUNABLE]`): `eval_id` (UUID v5 of the user ID and the swipe's `Idempotency-Key`, as S7 defines it, so a retried swipe cannot double-count; unrelated to the message), `user_pseudo_id`, `header_rules` (class, score), `gemini` and `jev` (class, score, confidence or probabilities, model version, latency, input tokens, error code), `outcome` (swipe direction, undone, time to swipe), `header_facts` (booleans). Segment buckets (CR-01a): `provider` (`gmail`, `graph`), `age_bucket` (`<7d`, `7d-90d`, `90d-1y`, `1y-5y`, `>5y`, from the message date), `text_tokens_bucket` (`<100`, `100-300`, `>300`), `lang_is_english` (boolean from a local detector; text never stored). Method versions: `input_version` (redaction and truncation rules), `question_version` (prompt and question wording), `price_version` (price table for cost). No sender, subject, text or message ID. Written when the swipe lands.

Label mapping: left with no undo means junk (`list`, `bulk_no_header` or `suspect`); right or up means wanted; undo flips the label; down is not a label.

The admin report (S7 ADM-10) is built for publication (CR-01a). It covers three methods: `header_rules@1` as the free baseline (confidence = `bulk_score / 100`, cost 0), Gemini and Jev. Per method: accuracy, per-class confusion, calibration with ECE, precision, recall and F1 for junk, the false-junk rate (wanted mail predicted junk), latency histogram plus p50 and p95, error and timeout rates, and cost per 1,000 messages. Every rate carries a Wilson 95% interval. Paired comparison on cards where all compared methods answered: counts of only-A-right, only-B-right, both and neither; McNemar's exact test; accuracy difference with a 95% bootstrap interval resampled by participant (cluster bootstrap). It also reports `n_paired` beside `cards`, the participant count and the largest contributor's share of labels, a `by_segment` breakdown (by header-rules class, each header fact and each bucket above) and a daily trend with intervals. Small-cell suppression applies everywhere, per segment included. The report never pools across different `question_version` or `input_version` values unless asked.

Snapshots: the admin can save a computed report (after suppression) to `bakeoff_snapshots` (C1, aggregate only, no pseudonymous IDs, kept until the admin deletes it), so published figures survive the 180-day TTL and later opt-outs. Snapshots are never recomputed after an opt-out. The report and snapshots also export as one tidy CSV table (`section, model, segment, metric, value, lower, upper, n`).

Label noise: James's labelled E2 set gives the class-level benchmark (experiment E4). E4 also reports how far swipe labels agree with E2 labels on overlapping messages, computed locally in `tools/`, with only the agreement figure committed.

### 5.7 Operations

- Egress is enforced at the `HttpEgress` port: each service has a host allowlist, and a test proves any other host is refused (3 October 2026, spec audit). `api`: Gmail, Drive, the Google OAuth endpoints, Vertex AI and `api.typesafe.ai` (v2 adds Graph, OneDrive and the Microsoft OAuth endpoints). `unsub`: Gmail send, the Google OAuth token endpoint (to mint access tokens), and the one-click target (no Drive) (validated per S6 section 6 and section 3.3 step 5; v2 adds Graph send). `pagehandler` (v2): the internet, through its own SSRF controls only (section 3.4).
- Jev API key in Secret Manager, `api` the only accessor, rotated quarterly. Vertex uses the `api` service account with `roles/aiplatform.user`; no key.
- Vertex: prompt caching off for the project; request the abuse-monitoring logging exception for zero data retention.
- Kill switches: `CLASSIFIER_JEV_ENABLED` and `CLASSIFIER_GEMINI_ENABLED` at start-up plus a Firestore config document (`config/classifiers`, S5) checked every minute.
- Model calls run in parallel off the card's critical path; 2-second timeout; Jev capped at 8 in flight per request (TypeSafe limit 80 requests a second).
- Billing alerts on TypeSafe and on Vertex AI usage.

### 5.8 Consent and policy

- Settings, Experiments: off by default; the consent text names Google (Vertex AI, United States) and TypeSafe AI (United States), what is sent, no training, that TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place, and possible publication of anonymous accuracy figures, and that anonymous totals already published or saved stay as they are after opt-out. Jev stays open to all consenting users, friends included, before a DPA (James, 3 October 2026, spec audit Q4). Turning it off stops calls immediately and deletes that user's `classifier_eval` records; saved snapshots are unaffected.
- Google's Limited Use policy applies from the first user: consent, user-facing purpose, no training by the receiver.
- Publishing: decided, James will publish the findings as a blog post (3 October 2026). Aggregates only, from saved snapshots; the headline uses James's own mail and states the participant count and the largest contributor's share.
- Before Google verification: TypeSafe supplies a DPA, retention commitment and security attestation, or Jev is switched off and its code removed from the verified build.

## 6. Open decisions

1. **Infrastructure as code tool.** Decided: Terraform.
2. **Cloud Armor before production.** Needs an external Application Load Balancer, which adds a fixed monthly cost. Decide before leaving Testing mode. The same load balancer removes the accepted `__session` cookie deviation (route `/api` directly to Cloud Run and use `__Host-session`).
3. **Sign-in persistence:** Google OAuth sign-in with refresh tokens under the KMS-wrapped per-user `data_key`. The passkey lock is dropped for the trial and moves to v2 pre-CASA hardening (James, 3 October 2026).
4. **Microsoft support.** Decided: v2 (James, 3 October 2026). Same architecture; the Graph adapter, Graph send for mail, and OneDrive app folder slot in behind the existing traits.

## Sources

- [Cloud Run pricing and free tier](https://cloud.google.com/run/pricing)
- [Google Cloud free tier](https://docs.cloud.google.com/free/docs/free-cloud-features)
- [Cloud KMS pricing](https://cloud.google.com/kms/pricing)
- [Firestore TTL policies](https://docs.cloud.google.com/firestore/native/docs/ttl)
