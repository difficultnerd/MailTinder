# S3: Domain Model and State Machines

Status: DRAFT for James's review. 3 October 2026.
Depends on: `S2-v1-acceptance-criteria.md`, `research/data-handling-and-in-account-ai.md` (Part 1).

## Where each entity lives

Three stores, following the data handling design (D10, decided by James, 3 October 2026):

- **Server database (Firestore):** only what the server needs to run jobs and stay secure. Sensitive columns use per-user envelope encryption under the user's `data_key` (S6 section 5), refresh tokens included (passkey lock dropped for the trial, James, 3 October 2026; v2 pre-CASA hardening).
- **User app folder:** one encrypted file per user, holding the user's own state, in one primary app folder: the Drive `appDataFolder` of the user's first linked Gmail mailbox. Other mailboxes' Drives are unused. Only `api`, in session, writes the file. Every write uses the provider ETag (`If-Match`) and re-reads and retries on conflict, so two tabs or requests cannot overwrite each other's History entries. `unsub` never touches Drive: it stores each job's outcome on the job record, and the next Feed load appends it to History. The OneDrive app folder arrives with Microsoft in v2. The browser caches the file for the session. The file is encrypted under `data_key`. Disconnecting the primary mailbox first moves the file to the next linked mailbox's Drive, which becomes primary (S2 AU-05 AC4).
- **Provider mailbox:** the messages themselves, plus labels or categories the app applies.

Nothing that identifies message content is stored on the server except inside a queued job or Needs Attention item, each with a TTL.

## Entities

### Server database (Firestore collections)

| Entity | Key fields | Notes |
| --- | --- | --- |
| `User` | `user_id` (UUID), `created_at`, `is_admin`, `wrapped_data_key` (KMS), `experiments_consent_version`, `experiments_opted_in_at` | One per-user `data_key` (S6 section 5), usable by the server: it encrypts refresh tokens, the app folder file, sealed tokens and encrypted fields. Destroying `wrapped_data_key` crypto-shreds the user |
| `Mailbox` | `mailbox_id`, `user_id`, `provider` (v1: gmail; graph in v2), `provider_subject_id` (Google `sub`; v2 Microsoft rule: `tid` plus `oid`), `email_address` (encrypted), `status`, `linked_at`, `is_primary`, refresh token (encrypted under `data_key`, associated data = user ID plus mailbox ID plus field name) | Unique on (`provider`, `provider_subject_id`); one user, many mailboxes, several per provider allowed. `is_primary` marks the mailbox whose Drive holds the app folder (the first linked). Any linked Gmail mailbox can sign the user in |
| `Invite` | `invite_id`, `email_address` (encrypted plus keyed hash for lookup), `token_hash` (SHA-256 of a single-use 256-bit random invite token), `status`, `created_at`, `last_sent_at`, `expires_at` (7 days after sending `[TUNABLE]`) | Redemption needs the invite token and a matching verified email (v2 Microsoft rule: only with `xms_edov` true). Re-send issues a new token and voids the old |
| `InviteRequest` | `request_id`, `email_address` (encrypted plus keyed hash), `created_at`, `status` | Deleted on decline |
| `UnsubscribeJob` | `job_id`, `user_id`, `mailbox_id`, `list_key_hash`, `method` (v1: one_click, mailto; page in v2), `target` (encrypted), `due_at`, `status`, `attempts` (Cloud Tasks retry count, recorded only), `outcome` (result code and time, written by `unsub`), `expires_at` | No access token is stored: `unsub` mints one from the mailbox's refresh token when the job runs. Hard TTL (`JOB_TTL`) 1 hour after `due_at` for a non-terminal job. A terminal job keeps only its `outcome` (target cleared) until the next Feed load appends it to History, up to 30 days `[TUNABLE]` |
| `NeedsAttentionItem` | `item_id`, `user_id`, `mailbox_id`, `sender_display` (encrypted), `link` (encrypted), `reason_code`, `created_at`, `status` | Hard TTL 30 days |
| `Session` | `session_hash`, `session_record_id` (random, stable across session ID rotation; sealed tokens bind to it, S6 section 5), `recent_auth_at` (time of the last Google sign-in, from `auth_time`; checked for step-up), `state` (`pre_auth`, `pending_invite_request`, `authenticated`), `user_id`, `csrf_token`, `created_at`, `last_seen_at`, `recent_auth_at` (last fresh Google sign-in, for step-up), `expires_at` (TTL); `pre_auth` fields (OAuth `state`, `nonce`, PKCE verifier, invite token hash, pending email, encrypted) | Opaque cookie maps to this; idle 15 minutes and absolute 12 hours. One session per user: a new sign-in deletes the old record. Admin "end this user's session" deletes it too |

Security events are not a Firestore entity. They are structured log entries in the locked log bucket (S5 Logs, S6 section 7): pseudonymous user ID, action, outcome, request ID, time; no addresses, subjects or URLs; 90-day retention.

### User app folder file

| Entity | Key fields |
| --- | --- |
| `SortRule` | `rule_id`, `kind` (reject_list, block_person, file), `match` (sender address, optional List-Id, optional Feedback-ID), `action`, `category_id`, `enabled`, `created_at`, `source_swipe` |
| `Category` | `category_id`, `name`, provider label or category ID per mailbox |
| `SenderStats` | `sender_key`, counts of keep, reject, file per category, `last_seen`, `block_prompt_declined_until` |
| `HistoryEntry` | `entry_id`, `at`, `mailbox_id`, `sender_display`, `action`, `rule_id`, `outcome` |
| `FeedCursor` | per mailbox: newest message seen, oldest message seen, skip counts |
| `PendingDeliveryCheck` | `list_key`, `unsubscribed_at` |

### Transient (browser memory only)

| Entity | Key fields |
| --- | --- |
| `Card` | `mailbox_id`, `message_id`, sender, subject, preview, `bulk_score`, `bulk_reason`, `class` |
| `SwipeRecord` | `card`, `action`, `previous_labels`, `job_id` (if any), `rule_id` (if any) |
| `UndoStack` | session list of `SwipeRecord` |

## Classification

- `ClassifierId`: for example `header_rules@1`, `gemini@flash-lite`, `jev@1.13.0`.
- `Classification` (transient, per card): `class`, `bulk_score`, `bulk_reason`, `confidence`, `probabilities`. Header rules drive the badge; Gemini and Jev predictions ride along in the sealed classification token (S4 5.3; sealed under `data_key` by the one scheme in S6 section 5). Every result is clamped by the header guard (S4 5.2).
- `ClassifierEval` (Firestore `classifier_eval`, pilot only): see S4 5.6 and S5. Includes segment buckets (`provider`, `age_bucket`, `text_tokens_bucket`, `lang_is_english`) and method versions (`input_version`, `question_version`, `price_version`). No content, no addresses, no message IDs.
- `BakeoffSnapshot` (Firestore `bakeoff_snapshots`): a saved, suppressed bake-off report with its query and creation time. Aggregate only, no pseudonymous IDs; deleted only by an admin; never recomputed after an opt-out.
- Swipe-to-label mapping for the bake-off: left with no undo is junk; right or up is wanted; undo flips the label; down is not a label.
- `User` gains `experiments_consent_version` and `experiments_opted_in_at` (S7).

## Message classes

The classifier assigns each card exactly one class. The class decides what a left swipe does.

| Class | Meaning | Left swipe does |
| --- | --- | --- |
| `list` | Mailing list with an unsubscribe mechanism | Trash, queue unsubscribe (one-click or mailto), create reject_list rule. In v1 an https-only header (no one-click) creates no job; it raises a Needs Attention item with "Open unsubscribe page" (link from the DKIM-covered header only) |
| `bulk_no_header` | Looks bulk but has no unsubscribe header | Trash, create reject_list rule; no unsubscribe attempt |
| `notice` | Account or security notice | Trash only |
| `personal` | One-to-one mail from a person | Trash, count toward block prompt |
| `suspect` | Suspected spam or phishing | Report as spam, trash, no unsubscribe |

`bulk_score` (0 to 100) plus `bulk_reason` drives the badge. The class is derived from the score and header facts; the thresholds are set by spike E2.

Classifier inputs (spike E1 findings): a one-click header means a list, not necessarily marketing, so it must not alone push the score to junk; Gmail category labels are absent on some accounts and are not an input; DKIM over the unsubscribe headers is checked instead of DMARC.

`sender_key` normalisation: lower-cased address, with known forwarding relays (for example iCloud Hide My Email) unwrapped to the original sender before any rule match or stats update.

Rule matching and counting (3 October 2026, spec audit):

- A `reject_list` rule with a `List-Id` matches on `List-Id`. A sender-only `reject_list` rule (no `List-Id`) matches only messages that carry `List-Unsubscribe`; where the rejected message carried `Feedback-ID`, the rule stores it as a second key that must also match. Receipts and other mail without `List-Unsubscribe` from the same sender survive.
- Reject rules, block prompts and `SenderStats` reject counts come only from messages with a DKIM-aligned `From` or a provider authentication pass. Otherwise the swipe still acts on that message, but nothing is counted and no rule is created.

## State machines

### UnsubscribeJob

```
queued --(due_at reached)--> running
queued --(undo or mailbox disconnected)--> cancelled
running --(2xx)--> sent
running --(retryable error, Cloud Tasks attempts left)--> running (Cloud Tasks redelivers the same task with backoff)
running --(non-retryable, 3xx on one-click (no redirect followed), or final Cloud Tasks attempt)--> needs_attention
running --(refresh token revoked or invalid when minting the access token)--> failed (Needs Attention "Sign in again")
any non-terminal --(now > expires_at)--> expired (creates Needs Attention)
```

Cloud Tasks (`maxAttempts` 4) is the only retry layer; the job never re-queues itself on error (3 October 2026, spec audit). `unsub` mints the mailbox access token from the stored refresh token at run time; there is no `awaiting_session` state (passkey lock dropped, James, 3 October 2026). Background mailbox work is limited to queued unsubscribe jobs.

Terminal: `sent`, `cancelled`, `needs_attention`, `failed`, `expired`. `unsub` writes the outcome to the job record and the security log. The next Feed load (`api`, in session) appends it to History and then deletes the job record.

### NeedsAttentionItem

```
open --(user marks done)--> resolved (deleted)
open --(user dismisses)--> dismissed (deleted)
open --(30 days)--> expired (deleted)
```

### Invite

```
pending --(valid invite token plus matching verified email)--> used
pending --(admin re-sends)--> pending (new token; old token void)
pending --(admin revokes)--> revoked
pending --(expires_at)--> expired
```

### Mailbox status

```
connected --(token invalid or revoked)--> needs_sign_in
needs_sign_in --(OAuth success)--> connected
(OAuth consent error from tenant, v2 Microsoft) --> consent_blocked
any --(user disconnects)--> removed (record deleted)
```

### Swipe and undo

```
card shown --(swipe or button)--> action applied --> SwipeRecord pushed
undo --> pop SwipeRecord --> reverse provider change, cancel job if queued, remove rule if created --> card shown again
```

Reversal must restore the exact previous label set, not a guessed one.

### Card visibility

A message is excluded from the Feed when any of these hold: it matches an enabled sort rule (the rule acts on it instead), it is no longer in the inbox at fetch time, or its skip count reached the limit this session.

## Invariants (each becomes a test)

- **INV-1.** No server table holds a message body, subject or snippet.
- **INV-2.** Every `UnsubscribeJob` and `NeedsAttentionItem` has an `expires_at`, and a sweeper deletes expired rows.
- **INV-3.** A mailbox belongs to exactly one user.
- **INV-4.** Every automated change to a mailbox has a matching `HistoryEntry`.
- **INV-5.** No code path permanently deletes a message.
- **INV-6.** Every swipe in the current session can be reversed by undo.
- **INV-7.** Core domain code does not import provider-specific types.
