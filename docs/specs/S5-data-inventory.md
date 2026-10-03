# S5: Data Inventory and Retention

Status: DRAFT for James's review. 3 October 2026.
Depends on: `S3-domain-model.md`, `S4-architecture.md`, `research/data-handling-and-in-account-ai.md` (Part 1).
Principle: no mail content at rest on our infrastructure. Everything we hold has a purpose, a TTL or a deletion path, and a test.

Sign-in: Google OAuth. The passkey lock is dropped for the trial and moves to v2 pre-CASA hardening (James, 3 October 2026). Key hierarchy (S6 section 5 is the source): one per-user `data_key`, wrapped by Cloud KMS and usable by the server, encrypts refresh tokens, the app folder file, sealed tokens and every other encrypted field.

## Classification levels

| Level | Meaning | Examples | Handling |
| --- | --- | --- | --- |
| **C3 Secret** | Grants access to a mailbox or decrypts user data | Refresh and access tokens, `data_key`, OAuth client secrets, HMAC keys | Envelope encrypted (KMS plus per-user key); never logged; memory only once unwrapped |
| **C2 Personal** | Identifies a person or reveals mail habits | Email addresses, sender names, unsubscribe URLs, sort rules, History | Encrypted at application level; never logged; TTL or user-controlled |
| **C1 Internal** | Pseudonymous operational data | Pseudonymous user ID, request ID, outcome codes | May be logged; 90-day retention |
| **C0 Public** | The app itself | Static web assets | None |

Mail content (bodies, subjects, snippets, headers) is C2 at least and is **never stored**. It exists only in server memory for the length of a request and in browser memory while a card is shown.

## Firestore (server)

| Collection and field | Class | Purpose | Retention | Deletion path | Test |
| --- | --- | --- | --- | --- | --- |
| `users/{id}`: `created_at`, `is_admin` | C1 | Account | Life of account | Account deletion | DEL-1 |
| `users/{id}`: `wrapped_data_key` (KMS) | C3 | Unwraps `data_key`, which encrypts refresh tokens, the app folder file, sealed tokens and encrypted fields | Life of account | Destroyed on deletion (crypto-shred) | DEL-2 |
| `mailboxes/{id}`: `provider` (v1: `gmail`), `provider_subject_id` (Google `sub`; v2 Microsoft rule: `tid` plus `oid`), `status`, `linked_at`, `is_primary` | C2 | Link a mailbox to a user; show when it was linked; mark the mailbox whose Drive holds the app folder | Until disconnected | Disconnect or deletion | DEL-3 |
| `mailboxes/{id}`: `email_address` (encrypted) | C2 | Display in Settings | Until disconnected | Disconnect or deletion | DEL-3 |
| `mailboxes/{id}`: refresh token (encrypted under `data_key`; associated data = user ID plus mailbox ID plus field name) | C3 | Access the mailbox across sessions; lets `unsub` mint an access token when a job runs | Until disconnected or revoked | Revoke at provider, then delete | DEL-3 |
| `sessions/{id}`: session hash, session record ID (random, stable across session ID rotation; sealed tokens bind to it), state, user ID, CSRF token, `recent_auth_at` (last fresh Google sign-in, for step-up), expiry | C1 | Authenticated session; one per user (a new sign-in deletes the old one) | Idle 15 minutes, absolute 12 hours | Expiry, sign-out, new sign-in, admin end, deletion | SES-1 |
| `sessions/{id}`: `pre_auth` fields (OAuth `state`, `nonce`, PKCE verifier, invite token hash, pending email, encrypted) | C2 | Complete the OAuth round trip; hold an invite request's verified email | Until the callback or 10 minutes `[TUNABLE]` | Cleared on state change; TTL | SES-1 |
| `invites/{id}`: email (encrypted plus HMAC for lookup), invite token hash (SHA-256), status, `last_sent_at`, expiry (7 days after sending `[TUNABLE]`) | C2 | Invite check; the token makes redemption need the invite email itself | Until used, revoked or expired, then 30 days; re-send replaces the token hash | Sweeper | INV-T1 |
| `invite_requests/{id}`: email (encrypted plus HMAC), time | C2 | Admin approval | Until approved or declined | Decline deletes; approve converts | INV-T2 |
| `jobs/{id}`: mailbox ID, list key HMAC, method, target (encrypted), due time, status, `outcome` (result code and time, written by `unsub`), `expires_at` | C2 | Delayed unsubscribe; carries the outcome until `api` appends it to History at the next Feed load | Non-terminal: TTL 1 hour after due (`JOB_TTL`). Terminal: target cleared, outcome kept until the next Feed load, at most 30 days `[TUNABLE]` | `api` after the History append, sweeper, Firestore TTL backstop | JOB-1 |
| `needs_attention/{id}`: sender display and link (encrypted), reason code, `expires_at` | C2 | Human help request | Until resolved; TTL 30 days | User action, sweeper, TTL backstop | NA-T1 |
| `rate_limits/{key}`: counters | C1 | Anti-automation | Window length | TTL | RL-1 |
| `classifier_eval/{id}`: eval ID (UUID v5 of the user ID and the swipe's `Idempotency-Key`, S7), pseudonymous user ID, header-rules class and score, Gemini and Jev predictions (class, score, confidence, model version, latency, tokens, error code), swipe outcome, header-fact booleans, segment buckets (`provider`, `age_bucket`, `text_tokens_bucket`, `lang_is_english`), method versions (`input_version`, `question_version`, `price_version`) | C1 | Classifier bake-off (S4 section 5) | TTL 180 days `[TUNABLE]` | TTL; opt-out deletes the user's rows; account deletion | EXP-1 |
| `bakeoff_snapshots/{id}`: saved report after small-cell suppression, its query, creation time | C1 (aggregate only, no pseudonymous IDs) | Reproducible published bake-off figures (CR-01a) | Until an admin deletes it | Admin delete; not affected by opt-out or account deletion, as it holds nothing per user | EXP-3 |
| `users/{id}`: `experiments_consent_version`, `experiments_opted_in_at` | C1 | Bake-off consent: which consent text the user agreed to, and when | Life of account; cleared on opt-out | Opt-out; account deletion | EXP-2 |
| `config/classifiers`: kill switches (`gemini_enabled`, `jev_enabled`), `updated_at` | C1 (no user data) | Turn a bake-off model off within a minute without a deploy (S4 5.7) | Life of the service | Admin change (ADM-9) overwrites | CFG-1 |

No other collections are permitted. A schema test fails the build if a new collection or field appears without an entry here.

## Keys and secrets held outside Firestore

S6 section 5 is the cryptographic inventory; this lists what holds data or unlocks it.

| Key or secret | Class | Purpose | Held in | Retention and rotation | Test |
| --- | --- | --- | --- | --- | --- |
| KMS key encryption key | C3 | Wraps every `data_key` | Cloud KMS only | Yearly, automatic | DEL-2 |
| Email lookup HMAC key | C3 | Keyed hash for invite and invite request lookup | Secret Manager | Yearly | INV-T1 |
| Log pseudonymisation HMAC key | C3 | Pseudonymous user ID in logs | Secret Manager | Yearly | LOG-1 |
| OAuth client secrets, Jev API key | C3 | Provider and vendor access | Secret Manager | On provider rotation; Jev quarterly | LOG-1 |

## User app folder (Google Drive `appDataFolder`; OneDrive app folder in v2)

One file per user, in one primary app folder: the Drive of the user's first linked Gmail mailbox (D10, decided by James, 3 October 2026). Other mailboxes' Drives are unused. Writes use the provider ETag (`If-Match`) and retry on conflict. The file is AES-256-GCM encrypted with the user's `data_key` (only `api`, in session, writes it; `unsub` has no Drive access). The user can delete it; the app rebuilds what it can from labels.

| Content | Class | Retention |
| --- | --- | --- |
| Sort rules (reject list, block person, filing) | C2 | User controlled |
| Categories and sender to category map | C2 | User controlled |
| Sender stats (keep, reject and file counts) | C2 | User controlled |
| History entries | C2 | Trimmed at 12 months `[TUNABLE]` |
| Mail stopped estimate (one integer per rule, yearly rate) | C2 | Lives and dies with its rule |
| Achievements (ID and unlock date) | C2 | User controlled; deleted with account |
| Feed cursor and skip counts | C2 | Overwritten |
| Pending delivery checks | C2 | Cleared after the check |

Deleted on account deletion (AU-06).

## Provider mailbox

Labels and categories the app creates stay with the user's mail. Trash moves are reversible by the provider for its normal trash period (30 days for Gmail).

## Browser

| Data | Storage | Retention |
| --- | --- | --- |
| Session cookie (`__session`, HttpOnly, Secure, SameSite=Lax, no Domain) | Cookie | Session |
| Cards, undo stack, cached app folder file | Memory only | Tab lifetime; cleared on sign-out |
| Anything else | None | Nothing in localStorage, sessionStorage or IndexedDB (ASVS V14.3) |

## Logs and telemetry

| Field allowed | Class |
| --- | --- |
| Request ID, pseudonymous user ID (HMAC of user ID under the log pseudonymisation key), route template, status code, latency, action type, outcome code, rate-limit hit | C1 |

Everything else is banned from logs, including tokens, cookies, message IDs, addresses, names, subjects, snippets, bodies, URLs and page content. Retention 90 days in a locked log bucket. Security events (S6 section 7) live here, not in Firestore. Enforced by a redacting wrapper type in Rust, the template's `optional/privacy` Semgrep rules (extended with these field names) and a log-scanning test over integration test output.

## Third parties that receive data

| Party | What | Why |
| --- | --- | --- |
| Google (Gmail API, Drive API, Cloud) | Tokens, API calls, encrypted app folder file | Provider and host |
| Microsoft (v2) | Same, for Microsoft mailboxes | Provider |
| Unsubscribe targets | One-click POST or mailto email (v1); the user's address on a page form (v2 page handler) | The unsubscribe itself |
| Google Vertex AI (Gemini, United States, `us-central1`) and TypeSafe AI (Jev, United States) | Per message, for consenting pilot users only: allowlisted headers, subject, about 500 tokens of stripped text with URLs, addresses and digit runs replaced (S4 5.5) | Classifier bake-off; requires consent; Jev vendor terms gate before Google verification (S4 5.8) |

No analytics or error-reporting vendors in v1. The only AI processors are Vertex AI and TypeSafe (Jev), for consenting pilot users only. Gemini Nano runs on the user's device.

**Data location:** all server-side data (Firestore, KMS, Secret Manager, logs, Vertex AI calls) is held in the United States (`us-central1`). The privacy notice must say so, to meet Australian Privacy Principle 8 (cross-border disclosure).

## Deletion tests

| ID | Test |
| --- | --- |
| DEL-1 | After account deletion, no document references the user ID |
| DEL-2 | After deletion, a backup copy of an encrypted field cannot be decrypted |
| DEL-3 | Disconnecting a mailbox revokes its token at the provider and removes its document |
| SES-1 | Expired, signed-out and superseded sessions cannot be used; a new sign-in leaves one session record per user; `pre_auth` fields are cleared on the state change and do not outlive their TTL |
| CFG-1 | `config/classifiers` holds only the allowed fields; turning a switch off stops that model's calls within one check |
| JOB-1 | No job document survives past its `expires_at` plus sweeper interval; no job document holds an access token; a terminal job keeps only its outcome |
| NA-T1 | No Needs Attention document survives 30 days |
| INV-T1, INV-T2 | Invite records follow their retention |
| RL-1 | Rate-limit counters expire |
| LOG-1 | Integration test logs contain no value from the fixture mail corpus |
| EXP-1 | `classifier_eval` rows contain no value from the fixture mail corpus and no address; every field matches the allowed schema (buckets, booleans, version strings only); opt-out deletes them |
| EXP-3 | A `bakeoff_snapshots` document contains no pseudonymous user ID, eval ID or cell below the minimum size |
| EXP-2 | Input sent to Gemini or Jev contains no recipient address, URL, message ID or unsubscribe URL from the fixture corpus |
| JEV-1 | A user who has not consented causes zero outbound calls to the Jev or Vertex AI endpoints |
