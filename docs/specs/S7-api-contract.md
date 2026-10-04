# S7: API Contract

Status: DRAFT for James's review. 3 October 2026.
Machine-readable contract: `S7-api-contract.openapi.yaml` (OpenAPI 3.1) in this folder. Where the two differ, this file wins until the YAML is fixed.
Depends on: `docs/change-requests/CR-01-pluggable-classifier-jev-pilot.md` (Gemini versus Jev bake-off), `S2-v1-acceptance-criteria.md` (story IDs), `S3-domain-model.md` (entities, states), `S4-architecture.md` (services), `S9-functional-screens.md` (screen states), `research/data-handling-and-in-account-ai.md` Part 3 (ASVS 5.0 mapping).

## How to read this

- Providers: v1 is Gmail only; Microsoft moves to v2 (James, 3 October 2026). Provider enums stay extensible (`google` and `microsoft` for sign-in, `gmail` and `graph` for mailboxes), but the v1 server accepts only `google` and `gmail`. Microsoft rules in this document are marked v2 and need no reshaping of any request or response to switch on.
- Scope: all traffic from the Flutter web app to the Rust `api` service, plus the internal calls Cloud Tasks and Cloud Scheduler make. Provider calls (Gmail, Graph, Drive) belong to S8.
- Each endpoint lists the story IDs it serves. A contract test name includes the endpoint ID, for example `api_sw_post_swipes_reject_queues_job`.
- `[ASSUMES]` marks a guess that James or a later spec should confirm. `[TUNABLE]` marks a starting value that lives in configuration.
- ASVS references are to ASVS 5.0 chapters (V1 to V17), matching the data handling research.

## 1. Principles

1. **Backend for frontend.** The browser never holds provider tokens or our own bearer tokens. It holds one opaque session cookie. The `api` holds provider tokens and makes every provider call (ASVS V10, V9).
2. **Storage is hidden behind the API.** Whether rules, History and the Feed cursor live in the user's app folder (D10, confirmed by James on 3 October 2026) or on our server, the contract is the same.
3. **No mail content at rest, in URLs or in logs.** Mail content appears only in response bodies, which carry `Cache-Control: no-store`. Request URLs never carry addresses, subjects, message IDs or links; those go in JSON bodies (ASVS V14).
4. **Every state change is a `POST`, `PUT`, `PATCH` or `DELETE`** and is CSRF-checked. `GET` never changes anything, including the Feed cursor, which is why fetching the Feed is a `POST` (section 5.4).
5. **Ownership is checked on every call.** A resource that belongs to another user returns `404 not_found`, never `403`, so IDs cannot be probed (ASVS V8).
6. **Nothing fails silently.** Partial provider failures come back as data the UI shows (S2 XC-04).

## 2. Transport and conventions

| Item | Rule |
| --- | --- |
| Base path | `/api/v1`. Firebase Hosting rewrites `/api/**` to the `api` Cloud Run service, so the app and API share one origin |
| Versioning | Breaking changes go to `/api/v2`. Additive fields do not bump the version; clients ignore unknown fields |
| Format | `application/json; charset=utf-8` only. Other request types get `415` |
| Request size | 16 KiB maximum body `[TUNABLE]`; larger gets `413` |
| Unknown fields | Rejected with `400 invalid_request` (serde `deny_unknown_fields`) (ASVS V2) |
| Strings | UTF-8, trimmed, length-limited per field in the OpenAPI schema |
| Times | RFC 3339 UTC, for example `2026-10-03T14:09:18Z` |
| IDs | Server-issued records use UUID v4, except mailbox IDs: UUID v5 of provider plus provider subject, so uniqueness on (`provider`, `provider_subject_id`) needs no extra collection. Provider message IDs are passed through as opaque strings |
| Unmatched routes | An unknown path or an unsupported method on a known path returns a `404 not_found` problem. No `405` is sent |
| Opaque tokens | Cursors, undo tokens, classification tokens and prompt references are sealed by the server (section 2.1). Clients treat them as opaque strings |
| Pagination | Cursor based: `limit` (default 20, maximum 50) and `cursor` in, `next_cursor` out (`null` at the end) |
| Caching | Every `/api` response sets `Cache-Control: no-store` and `Pragma: no-cache` so Firebase Hosting's CDN and the browser keep nothing |
| Request ID | Every response carries `X-Request-Id` (UUID). The same ID appears in logs and in error bodies so James can trace a report without seeing content |
| Security headers | Firebase Hosting sets the app's CSP and HSTS. The `api` also sets `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'` on its own responses, because the service is also reachable on its `run.app` URL, which bypasses Hosting |

### 2.1 Keys and sealed tokens (ASVS V9, V11)

One key per user, `data_key` (AES-256), wrapped by a Cloud KMS key and unwrapped by the server when needed (James, 3 October 2026: no passkey lock for the trial). It encrypts provider refresh tokens, sealed tokens, encrypted Firestore fields and the app folder file. No access token is ever stored. Because it is server-usable, queued unsubscribe jobs run without a live session. S6 section 5 owns the inventory.

- **Sealed tokens.** One scheme for every token type: AES-256-GCM under `data_key`, with associated data holding the token type (`cursor`, `undo`, `classification`, `prompt_ref`), the user ID, the session record ID and the expiry. Type and expiry travel in clear and are authenticated; each route checks the type it expects before opening. A token presented as another type, for another user or session, or after expiry fails to open and is treated as `400 invalid_request` (or `410 undo_expired` for undo; a classification token that fails to open only skips the eval record, section 5.5). This stops one token type being replayed as another (ASVS V9.2.2).
- **Session record ID.** Each session record has a random `session_record_id` (UUID) that stays the same when the cookie value rotates, so rotation on step-up or mailbox linking does not void the undo stack. It is never the cookie value or `session_hash`.
- No JWT or other bearer token is issued to the browser.

## 3. Authentication and session (ASVS V6, V7, V10)

### 3.1 Sign-in model

Sign-in is Google OAuth (James, 3 October 2026: the passkey lock is dropped for the trial; Microsoft is v2). Any mailbox linked to the user signs them in, and the OAuth grant is also the mailbox access grant.

| Path | Steps | Session state after |
| --- | --- | --- |
| New user | Invite link, OAuth (`join` with invite token) | `authenticated` |
| Returning user | OAuth (`sign_in`) on any linked mailbox | `authenticated` |
| Request an invite | OAuth (`join`) with no invite token, for an address with no account | `pending_invite_request` |

Accepted risk, recorded for S6 and the ASVS register: whoever controls any one linked Google account can sign in and reach every mailbox linked to that user. Step-up (section 3.6) limits what they can change without a fresh Google sign-in.

### 3.2 Session cookie and states

- One cookie, named `__session`. Firebase Hosting strips every other cookie before forwarding to Cloud Run, so the name is fixed by the platform. This rules out the `__Host-` and `__Secure-` prefixes (a recorded deviation from ASVS V3.3.1, removed if a load balancer is added; S4 open decision 2). The protections are set by attribute instead: `Secure; HttpOnly; SameSite=Lax; Path=/`, no `Domain` attribute.
- The value is a 256-bit random ID. The server stores the session record in Firestore keyed by `session_hash` (SHA-256 of the ID), so a database read does not yield live session IDs. The record also carries `session_record_id` (section 2.1).
- States:

| State | Holds | Allowed calls |
| --- | --- | --- |
| `pre_auth` | OAuth `state`, `nonce`, PKCE verifier, intent, invite token hash. Expires after 10 minutes `[TUNABLE]` | OAuth callback, API-AUTH-1, API-AUTH-3 |
| `pending_invite_request` | The verified email, encrypted | API-INV-1 only |
| `authenticated` | User ID, `recent_auth_at` | Everything the role allows |

- Any other call returns `401 unauthenticated`.
- **One session per user** (James, 3 October 2026). A new sign-in ends the user's previous session and writes a security event. The old browser gets `401 unauthenticated` on its next call.
- The ID rotates on sign-in, on step-up and on mailbox linking. `session_record_id` does not change within a session.
- Timeouts: idle 15 minutes, absolute 12 hours `[TUNABLE]` (decided by James, 3 October 2026). Either one deletes the record.
- `SameSite=Lax` is required so the cookie survives the top-level redirect back from Google. `Strict` would drop it on the callback.

### 3.3 CSRF

- Every `POST`, `PATCH`, `PUT` and `DELETE` requires header `X-CSRF-Token` matching the token bound to the session (synchroniser token). API-AUTH-3 returns it, creating a `pre_auth` session if none exists.
- The `api` also rejects any unsafe request whose `Origin` header is absent or not the app origin. Both sit on top of `SameSite=Lax`; S6 and the ASVS register name the same design.
- Failure returns `403 csrf_failed`. The OAuth callback is the one exception: it is a `GET` protected by the OAuth `state` parameter.
- CORS: no `Access-Control-Allow-Origin` is ever sent. The app is same-origin, so cross-origin calls fail by default.

### 3.4 OAuth flows

Authorisation code flow with PKCE (S256), `state` and `nonce`, exact redirect URI match. The `api` verifies the ID token signature, `iss`, `aud`, `exp` and `nonce`.

- **Verified email.** Google: `email_verified` must be true. (v2, Microsoft: the `email` claim counts only when the optional `xms_edov` claim is true; spec audit H4, ASVS V6.8.1.)
- **Mailbox identity.** Google mailboxes are keyed by `sub`, never by email (S8). The key field is generic (`provider_subject_id`); v2 Microsoft mailboxes use `tid` plus `oid`.
- **Invite token.** The invite email carries a single-use, random 256-bit token in the URL fragment (`/#/invite?t=...`), so it never reaches server logs or `Referer` headers. The app passes it in the body of API-AUTH-1. The server stores only its SHA-256. It expires after `INVITE_TTL` (7 days `[TUNABLE]`), and re-sending an invite issues a new token and voids the old one. Creating a user needs both a valid token and a verified email matching the invite (ASVS V6.4.1).
- **Refresh tokens** are stored encrypted under the user's `data_key` (section 2.1), one per mailbox.

`intent` values on API-AUTH-1:

| Intent | Who | Result on success |
| --- | --- | --- |
| `sign_in` | No session, or `pre_auth` | For a mailbox linked to a user: signs in (`authenticated`), ending any other session. Otherwise `not_registered` |
| `join` | No session, or `pre_auth` | With a valid invite token and matching verified email: creates the user (`authenticated`). Without a token, for an address with no account: `pending_invite_request`. For an existing account: signs in as `sign_in` would |
| `link` | `authenticated`, with step-up | Links another mailbox to the current user (AU-04) |
| `reconnect` | `authenticated` | Refreshes the tokens of a mailbox in `needs_sign_in` state |
| `step_up` | `authenticated` | Fresh sign-in for section 3.6 |

The callback never puts personal data in the redirect URL. It redirects to `/#/auth/result?outcome=<code>`, where `outcome` is one of the codes below. S9 owns the screen copy; where a quoted string here differs from S9, S9 wins.

| Outcome | Screen (S9) | Story |
| --- | --- | --- |
| `signed_in` | Feed | AU-03 |
| `joined` | Feed (first run) | AU-03 |
| `linked` | Settings, Connected accounts | AU-04 AC1, AC2 |
| `reconnected` | Back where the user was | ST-03 |
| `stepped_up` | Back to the waiting action | Section 3.6 |
| `not_invited` | Request invite | AU-02 AC1 |
| `not_registered` | Sign-in error ("No account uses this Google account. Ask for an invite.") | AU-03 |
| `invite_invalid` | Sign-in error ("This invite link has expired or been used") | AU-01, AU-03 |
| `email_mismatch` | Sign-in error | AU-03 |
| `email_unverified` | Sign-in error | AU-03 |
| `mailbox_linked_elsewhere` | Connected accounts error | AU-04 AC3 |
| `step_up_wrong_account` | Back to the waiting action with "Use one of your linked Google accounts" | Section 3.6 |
| `consent_blocked` | Connected accounts error (Microsoft work tenants, v2) | AU-04 |
| `cancelled` | Sign-in error with retry | S9 section 1 |
| `failed` | Sign-in error with retry | S9 section 1 |

On every outcome other than `signed_in`, `joined`, `linked` and `reconnected`, new provider tokens are discarded before the redirect (AU-02 AC1, AU-03). A `step_up` grant is used only for its ID token.

### 3.5 Unsubscribe jobs without a session

A job stores no access token. When a job runs, the `unsub` service mints one from the mailbox's stored refresh token (decrypted under `data_key`, through `svc-common`) and keeps it in memory for that run only. If the refresh token is revoked or invalid, the job ends `failed` with a `sign_in_required` Needs Attention item (S2 UN-01 AC6). A job never needs the user to be signed in, so S3's `awaiting_session` state is not used by this contract.

### 3.6 Step-up authentication (ASVS V7.5.1)

One rule: account, mailbox and admin changes need a fresh Google sign-in within `STEP_UP_WINDOW` (5 minutes `[TUNABLE]`; James decided the window on 3 October 2026). That covers:

- delete account (API-ACCT-1);
- link a mailbox (API-AUTH-1 `link`) or disconnect one (API-MBX-2);
- every admin write (`POST`, `PUT`, `PATCH`, `DELETE` under `/admin`), including kill switches and snapshots.

Without it they return `403 step_up_required`. The app shows the step-up overlay (S9 section 1.1), keeps the waiting request, calls API-AUTH-1 with `intent: "step_up"`, and the server sends the user to Google with `prompt=login` and `max_age=300` (S4 section 3.1, S6 section 4, ASVS V6.8.4). The callback accepts the ID token only if `auth_time` is within the last 5 minutes and its `sub` belongs to one of the user's linked mailboxes; then it sets `recent_auth_at` from the ID token's `auth_time` (never the server clock), rotates the session and redirects with `stepped_up`. The app resends the waiting request unchanged, with the same `Idempotency-Key` where the route takes one. Nothing about the waiting action is stored server side. A `join` or `link` callback also sets `recent_auth_at`.

### 3.7 Roles

Two roles: `user` and `admin`. `admin` is a flag on the `User` record set by the `mt-admin` tool (backlog T-507), run by James outside the API (ASVS V8). The tool also prints the first invite link. Every `/api/v1/admin/**` route checks it server side and logs refusals (AU-01 AC3).

## 4. Errors (ASVS V16)

Errors use RFC 9457 Problem Details, `Content-Type: application/problem+json`:

```json
{
  "type": "https://mailtinder.app/problems/rate_limited",
  "title": "Too many requests",
  "status": 429,
  "code": "rate_limited",
  "request_id": "6f1c2a1e-1d7e-4b8e-9f62-0d4a3c2b1a90",
  "retry_after_seconds": 30
}
```

- `code` is the stable field clients switch on. `title` is fixed per code and carries no request data.
- No stack traces, SQL, provider error text, addresses, subjects or URLs ever appear in an error body. Provider errors are mapped to the codes below and the raw error is logged only after scrubbing (S2 XC-01).
- Optional fields: `mailbox_id` (which mailbox failed), `retry_after_seconds`, `fields` (list of invalid JSON pointer paths for `invalid_request`, never their values).

| Status | `code` | Meaning | UI response (S9) |
| --- | --- | --- | --- |
| 400 | `invalid_request` | Schema, type or length violation | Bug; generic error |
| 401 | `unauthenticated` | No session, session expired, or a state that does not allow the call | Wipe in-memory state; sign-in screen |
| 403 | `csrf_failed` | Missing or wrong CSRF token or bad `Origin` | Reload session, retry once |
| 403 | `forbidden` | Not an admin | Generic error |
| 403 | `step_up_required` | Sensitive action needs a fresh Google sign-in in the last 5 minutes (section 3.6) | Run step-up, then retry |
| 404 | `not_found` | Missing, or belongs to someone else | Generic error |
| 409 | `app_folder_move_failed` | Disconnecting the primary mailbox, but the app folder file could not move to the next mailbox; nothing disconnected | "Couldn't move your settings. Try again." |
| 409 | `last_mailbox` | Cannot disconnect the only mailbox | "Delete your account instead" (AU-05 AC2) |
| 409 | `mailbox_needs_sign_in` | Provider token invalid or missing | "Can't reach <address>. Sign in again" |
| 409 | `message_changed` | Message no longer where the card said (moved or deleted elsewhere) | Card dropped silently (FD-04) |
| 409 | `category_exists` | Name clash on create or rename | Select existing (S9 section 4) |
| 409 | `consent_outdated` | Opt-in sent with an old consent text version | Show the current consent text again |
| 409 | `experiment_unavailable` | Opt-in while the pilot is switched off | "The experiment is paused" |
| 409 | `versions_mixed` | Report range spans more than one `input_version` or `question_version` and pooling was not asked for | "Pick one version" (S9 section 7.7) |
| 409 | `snapshot_limit` | 50 snapshots already saved `[TUNABLE]` | "Delete a snapshot first" |
| 406 | `not_acceptable` | `Accept` is neither JSON nor `text/csv` on a report route | Bug; generic error |
| 410 | `undo_expired` | Undo token from an earlier session | Undo disabled |
| 413 | `payload_too_large` | Body over limit | Bug; generic error |
| 415 | `unsupported_media_type` | Not JSON | Bug; generic error |
| 429 | `rate_limited` | Our limit hit (section 6) | "Too many requests. Try again later." |
| 502 | `provider_error` | Provider refused or returned an error | "Couldn't do that. Try again." (XC-04) |
| 503 | `provider_unavailable` | Provider down or rate limiting us | Same, honour `retry_after_seconds` |
| 500 | `internal_error` | Anything else | Generic error with request ID |

Every `401`, `403`, `404` on an admin route, OAuth callback failure and `429` is written to the security log with the pseudonymous user ID, route template (not the raw path), outcome and request ID (S3 `SecurityEvent`).

## 5. Endpoints

`Auth` column: `none` (no session needed), `state:<name>` (the session state from section 3.2), `user` (`authenticated`), `admin`, `step-up` (section 3.6), `oidc` (Google-signed OIDC token from a named service account). Every non-`GET` except INT routes needs CSRF.

### 5.1 Inventory

| ID | Method and path | Auth | Stories |
| --- | --- | --- | --- |
| API-AUTH-1 | `POST /api/v1/auth/{provider}/start` | none, or user (step-up for `link`) | AU-02, AU-03, AU-04, ST-03, section 3.6 |
| API-AUTH-2 | `GET /api/v1/auth/{provider}/callback` | state:pre_auth | AU-02, AU-03, AU-04 |
| API-AUTH-3 | `GET /api/v1/session` | none | AU-03 AC5, AU-02 |
| API-AUTH-4 | `POST /api/v1/auth/sign-out` | any session | AU-03 AC5 |
| API-INV-1 | `POST /api/v1/invite-requests` | state:pending_invite_request | AU-02 AC1, AC4 |
| API-MBX-1 | `GET /api/v1/mailboxes` | user | ST-03, AU-04 AC2 |
| API-MBX-2 | `DELETE /api/v1/mailboxes/{mailbox_id}` | step-up | AU-05 |
| API-PROG-1 | `GET /api/v1/progress` | user | GM-01, GM-04 |
| API-FEED-1 | `POST /api/v1/feed/next` | user | FD-01 to FD-04, SR-01 AC2, UN-06, SW-02 AC2, FL-01, FL-04 AC1 |
| API-SW-1 | `POST /api/v1/swipes` | user | SW-01 to SW-04, SR-01 AC1, PB-01 AC1, UN-01 |
| API-SW-2 | `POST /api/v1/swipes/undo` | user | SW-05 |
| API-CAT-1 | `GET /api/v1/categories` | user | FL-05, SW-04 AC1 |
| API-CAT-2 | `POST /api/v1/categories` | user | FL-02 |
| API-CAT-3 | `PATCH /api/v1/categories/{category_id}` | user | S9 section 5 (rename) |
| API-CAT-4 | `DELETE /api/v1/categories/{category_id}` | user | S9 section 5 (delete label only) |
| API-CAT-5 | `GET /api/v1/categories/{category_id}/messages` | user | FL-05 |
| API-RULE-1 | `GET /api/v1/rules` | user | SR-01, PB-01, FL-04, S9 section 7.3 |
| API-RULE-2 | `POST /api/v1/rules` | user | PB-01 AC2, FL-04 AC2 |
| API-RULE-3 | `PATCH /api/v1/rules/{rule_id}` | user | SR-01 AC4, ST-01 AC2 |
| API-RULE-4 | `DELETE /api/v1/rules/{rule_id}` | user | S9 section 7.3 |
| API-RULE-5 | `POST /api/v1/block-prompts/decline` | user | PB-01 AC3 |
| API-NA-1 | `GET /api/v1/needs-attention` | user | NA-01 AC1, UN-05 |
| API-NA-2 | `POST /api/v1/needs-attention/{item_id}/resolve` | user | NA-01 AC2 |
| API-NA-3 | `POST /api/v1/needs-attention/{item_id}/dismiss` | user | NA-01 AC2 |
| API-HIST-1 | `GET /api/v1/history` | user | ST-01, UN-01 AC3 |
| API-STAT-1 | `GET /api/v1/stats` | user | ST-02 |
| API-ACCT-1 | `DELETE /api/v1/account` | step-up | AU-06 |
| API-ADM-1 | `GET /api/v1/admin/invites` | admin | AU-01 |
| API-ADM-2 | `POST /api/v1/admin/invites` | admin, step-up | AU-01 AC1, AC2 |
| API-ADM-3 | `POST /api/v1/admin/invites/{invite_id}/resend` | admin, step-up | AU-01 AC2 |
| API-ADM-4 | `DELETE /api/v1/admin/invites/{invite_id}` | admin, step-up | AU-01 AC4 |
| API-ADM-5 | `GET /api/v1/admin/invite-requests` | admin | AU-02 AC2 |
| API-ADM-6 | `POST /api/v1/admin/invite-requests/{request_id}/approve` | admin, step-up | AU-02 AC3 |
| API-ADM-7 | `POST /api/v1/admin/invite-requests/{request_id}/decline` | admin, step-up | AU-02 AC3 |
| API-EXP-1 | `GET /api/v1/me/experiments` | user | CL-02 |
| API-EXP-2 | `PUT /api/v1/me/experiments` | user | CL-02 |
| API-ADM-8 | `GET /api/v1/admin/experiments/classifier` | admin | CL-04 |
| API-ADM-9 | `PATCH /api/v1/admin/experiments/classifier` | admin, step-up | CL-04 |
| API-ADM-10 | `GET /api/v1/admin/bakeoff` | admin | CL-03, CL-04, CR-01a |
| API-ADM-11 | `POST /api/v1/admin/bakeoff/snapshots` | admin, step-up | CL-04, CR-01a G6 |
| API-ADM-12 | `GET /api/v1/admin/bakeoff/snapshots/{snapshot_id}` | admin | CL-04, CR-01a G6, G7 |
| API-ADM-13 | `GET /api/v1/admin/bakeoff/snapshots` | admin | CL-04, S9 section 7.7 |
| API-ADM-14 | `DELETE /api/v1/admin/bakeoff/snapshots/{snapshot_id}` | admin, step-up | CL-04, S9 section 7.7 |
| API-ADM-15 | `DELETE /api/v1/admin/users/{user_id}/sessions` | admin, step-up | ASVS V7.4.5 |
| API-ADM-16 | `GET /api/v1/admin/users` | admin | S9 section 7.6 |
| API-INT-1 | `POST /internal/v1/unsubscribe-jobs/{job_id}/run` (service `unsub`) | oidc: Cloud Tasks | UN-01 to UN-05 |
| API-INT-2 | `POST /internal/v1/sweep` | oidc: Cloud Scheduler | UN-05 AC2, NA-01 AC2, S3 INV-2 |

Stories with no endpoint of their own: XC-01 to XC-05 are cross-cutting rules this contract enforces; FL-03 is carried by the `suggestion.confidence` field on cards; UN-02 and UN-03 run inside API-INT-1. UN-04 (the page handler) is deferred to v2 (James, 3 October 2026).

### 5.2 Session and auth

**API-AUTH-1** `POST /auth/{provider}/start` → `200 { "authorization_url": "https://..." }`; the app then navigates to it. A `POST` so that `link` and `reconnect` are CSRF-checked (ASVS V3.5.3).

```json
{ "intent": "sign_in", "invite_token": null, "mailbox_id": null }
```

`provider` is `google` in v1; `microsoft` returns `400 invalid_request` until v2. `intent` is one of section 3.4's intents. `invite_token` (from the URL fragment) only with `join`; `mailbox_id` only with `reconnect`. From no session or `pre_auth`, creates or rotates a `pre_auth` record holding `state`, `nonce`, the PKCE verifier, the intent and the invite token hash; from `authenticated`, the same values sit on the session record. `link`, `reconnect` and `step_up` need an `authenticated` session (`link` also needs step-up). For `step_up` the authorisation URL carries `prompt=login`, `max_age=300` and a `login_hint` of the primary mailbox, and requests no new scopes. Scopes come from S8 only (AU-04 AC5).

**API-AUTH-2** `GET /auth/{provider}/callback?code&state` (or `error`)
Validates `state` against the session, exchanges the code, verifies the ID token (section 3.4), applies the intent's rules (section 3.4), stores refresh tokens under `data_key`, rotates the session and `302`s to `/#/auth/result?outcome=...`. A `state` mismatch gives `outcome=failed` and a security event.

**API-AUTH-3** `GET /session`
Never `401`; reports the state so the app can route. Creates a `pre_auth` session when there is none, so the app always has a CSRF token.

```json
{
  "state": "authenticated",
  "csrf_token": "opaque",
  "user": { "user_id": "uuid", "is_admin": false },
  "pending_invite_email": null,
  "step_up_valid_until": null,
  "mailboxes": [{ "mailbox_id": "uuid", "provider": "gmail", "email_address": "a@example.com", "status": "connected" }],
  "idle_expires_at": "2026-10-03T14:24:18Z",
  "absolute_expires_at": "2026-10-04T02:09:18Z"
}
```

`state` is one of section 3.2's states, or `anonymous` for a bare `pre_auth` record. `pending_invite_email` is set only in `pending_invite_request`. `step_up_valid_until` is `recent_auth_at` plus 5 minutes, or null, so the app can skip a round trip that would return `403 step_up_required`.

**API-AUTH-4** `POST /auth/sign-out` → `204`. Deletes the session record, clears the cookie and sends `Clear-Site-Data: "cache", "storage"` (ASVS V14.3.1). The app also clears its in-memory state on sign-out and on any `401 unauthenticated`. Stored refresh tokens stay, so queued unsubscribe jobs still run (section 3.5).

### 5.3 Mailboxes and progress

**API-MBX-1** `GET /mailboxes` → `{ "mailboxes": [Mailbox] }`, where `Mailbox` is `mailbox_id`, `provider` (`gmail`, `graph`), `email_address`, `status` (`connected`, `needs_sign_in`; `consent_blocked` is v2, Microsoft only), `linked_at`, `is_primary` (true for the mailbox whose Drive holds the app folder file).

**API-MBX-2** `DELETE /mailboxes/{mailbox_id}` → `204`. Needs step-up. If it is the primary mailbox, the app folder file first moves to the next linked mailbox's Drive, which becomes primary; if the move fails, nothing is disconnected and the call returns `409 app_folder_move_failed` (S2 AU-05 AC4). Then it cancels the mailbox's queued jobs and Cloud Tasks, deletes its Needs Attention items, then revokes its provider tokens where supported and deletes them. `409 last_mailbox` when it is the only one.

**API-PROG-1** `GET /progress` → the inbox meter and level (GM-01, GM-04). The app calls it on open and after every 10 swipes `[TUNABLE]`, and keeps the session's starting count in memory to show "down 214 today".

```json
{
  "inbox_count": 12431,
  "mailbox_errors": [{ "mailbox_id": "uuid", "code": "mailbox_needs_sign_in" }],
  "level": { "year": 2023, "remaining": 1840 }
}
```

`inbox_count` sums the mailboxes that answered; `mailbox_errors` marks the rest (GM-01 AC2). `level` is `null` until new mail is cleared (GM-04 AC1). One provider folder-total call and one date-range count per mailbox.

### 5.4 Feed

**API-FEED-1** `POST /feed/next`

Request:

```json
{ "cursor": null, "limit": 20, "refresh": true }
```

- `cursor: null` starts from the stored Feed position (FD-03 AC3). `refresh: true` checks every mailbox for new mail first (FD-03 AC5): set it on app open and pull to refresh.
- This is a `POST` because it changes state: it advances the stored cursor, applies enabled sort rules to newly fetched mail (trashes matches and writes History, SR-01 AC2), runs the delivery check (UN-06), appends the outcomes of finished unsubscribe jobs to History and deletes those jobs (S3: a terminal job keeps only its outcome, for up to 30 days `[TUNABLE]`), and re-inserts skipped cards (SW-02 AC2).

Response:

```json
{
  "cards": [Card],
  "next_cursor": "opaque or null",
  "phase": "new",
  "phase_changed": false,
  "mailbox_errors": [{ "mailbox_id": "uuid", "code": "mailbox_needs_sign_in" }],
  "rule_actions_applied": 3
}
```

- `phase` is `new` or `backlog`. `phase_changed: true` on the first page after new mail runs out, so the app shows the divider card (FD-03 AC2, S9 section 3).
- `mailbox_errors` lists mailboxes that failed; their cards are missing but the others load (FD-02 AC3). If every mailbox fails the response is still `200` with no cards and every mailbox listed, and the app shows the full-screen sign-in prompt.
- Cards are ordered by `received_at`, newest first, across mailboxes (FD-02 AC1). Messages no longer in the inbox are left out (FD-04).

`Card`:

| Field | Type | Notes |
| --- | --- | --- |
| `mailbox_id` | UUID | Badge (FD-02 AC2) |
| `message_id` | string, at most 256 | Provider message ID |
| `received_at` | time | |
| `sender_name` | string, at most 256 | Plain text |
| `sender_address` | string, at most 320 | |
| `subject` | string, at most 998 | Plain text |
| `preview` | string, at most 300 | HTML stripped, no remote content (FD-01 AC2) |
| `bulk_score` | integer 0 to 100 | Badge grade |
| `bulk_reason` | string, at most 200 | Shown on badge tap |
| `class` | enum `list`, `bulk_no_header`, `notice`, `personal`, `suspect` | S3 message classes; tells the app what the reject toast will say |
| `unsubscribe_method` | enum `one_click`, `mailto`, `manual`, `none` | What a reject will do, from header rules: `one_click` and `mailto` queue an unsubscribe, so the toast says "Unsubscribing in 5 minutes"; `manual` (https-only header) means the link goes to Needs Attention; `none` means no usable header, so trash only. Drives the optimistic toast |
| `has_one_click` | boolean | True only when `unsubscribe_method` is `one_click`. Kept for the bake-off segments (`header_facts`); the toast uses `unsubscribe_method` |
| `suggestion` | object or null | Filing suggestion, below. Shipped with the card so the filing sheet meets the 200 ms target (FL-01 AC3) without a round trip |
| `keep_prompt` | object or null | `{ "category_id": "uuid", "category_name": "Receipts" }` when FL-04 AC1 applies |
| `skip_count` | integer 0 to 2 | |
| `boss` | object or null | `{ "remaining": 214 }` when the sender is a boss (GM-08): inbox mail left from that sender, for the health bar. One provider count query per boss card |
| `provider_web_url` | https URL | Opens the message in Gmail or Outlook on the web |
| `classification_token` | opaque string | Sealed record of the classification the card shows (section 5.13). Sent back on API-SW-1 |
| `classifier_id` | string, optional | Present only when the session user is an admin, for debugging: the classifier behind the badge, `header_rules@1` during the bake-off. Absent for everyone else, not `null`. Gemini's and Jev's predictions are never sent to the browser, even for admins, so James's own swipes stay unbiased labels |

All string fields are plain text. The app must render them as text, never as HTML or Markdown (ASVS V1). The server strips control characters and bidirectional override characters.

`suggestion`: `{ "category_id": "uuid or null", "name": "Tax invoices", "alternates": [{ "category_id": "uuid", "name": "..." }], "confidence": "learned" }`. `confidence` is `learned` (one-tap confirm, FL-03 AC1), `suggested`, or `none` (go straight to naming, FL-02). At most two alternates. The browser may override with Gemini Nano output; that never reaches the server except as the name the user confirms.

During the bake-off, `bulk_score`, `bulk_reason`, `class`, `unsubscribe_method` and `has_one_click` all come from header rules (CR-01 1.4). Destructive and outbound actions depend on header facts only, whichever classifier later drives the badge (S4 section 5.2).

### 5.5 Swipes and undo

**API-SW-1** `POST /swipes`

Headers: `Idempotency-Key: <uuid v4>` required. The app generates one per swipe and reuses it on retry.

Swipes are optimistic. Hosting is in `us-central1`, so a round trip from Australia takes about 200 ms; the app removes the card and shows the toast at once (the toast text comes from `class` and `unsubscribe_method` on the card), then sends API-SW-1 in the background. The app keeps rules for the gap:
- Swipe requests go out one at a time, in order, so undo and later swipes never overtake an earlier swipe.
- If Undo is tapped before the ack arrives, the app waits for that ack (or its failure) and then sends API-SW-2 with the returned `undo_token`.
- On a network error the app retries with the same `Idempotency-Key`. On `409 message_changed` the card stays gone. On any other error the card returns to the top of the Feed with "Couldn't do that. Try again." (S9 section 3, XC-04), and nothing is recorded as done.

```json
{
  "mailbox_id": "uuid",
  "message_id": "provider id",
  "action": "reject",
  "category_id": null,
  "new_category_name": null,
  "classification_token": "opaque"
}
```

- `action`: `keep`, `skip`, `reject`, `file`.
- `file` needs exactly one of `category_id` or `new_category_name` (1 to 100 characters, no `/` at the start or end, since Gmail treats `/` as nesting) (SW-04, FL-02 AC1).
- The server re-reads the message from the provider before acting and classifies it itself. It never trusts a class, score or unsubscribe target from the client. If the message has moved, `409 message_changed`.
- `classification_token` is required. If its clear type field is not `classification`, the server returns `400 invalid_request`. Any other failure to open it (an earlier session, expiry or tampering; AES-GCM cannot tell these apart) is not an error: the swipe proceeds and no `classifier_eval` record is written. A token that opens but names a different mailbox or message gives `400 invalid_request`. Its contents are used only to write the `classifier_eval` record (section 5.13). Header rules are re-run on the freshly read headers to decide the action.

Response `200`:

```json
{
  "outcome": "trashed_unsubscribe_queued",
  "unsubscribe_due_at": "2026-10-03T14:14:18Z",
  "undo_token": "opaque",
  "prompts": [{ "type": "block_person", "prompt_ref": "opaque", "sender_name": "Sam" }],
  "achievements_unlocked": [{ "achievement_id": "first_unsubscribe", "unlocked_at": "2026-10-03T14:09:19Z" }],
  "boss_defeated": false
}
```

`achievements_unlocked` lists achievements this swipe unlocked (GM-06; IDs from the fixed list in S2). `boss_defeated` is true when a reject removed a boss (GM-08 AC3). Both drive celebrations only; the app shows nothing else for them.

`outcome` values and what the server did:

| `outcome` | Server actions | Story |
| --- | --- | --- |
| `kept` | Counts the keep in sender stats | SW-01 |
| `skipped` | Increments skip count; schedules re-insertion | SW-02 |
| `trashed_unsubscribe_queued` | Trash; `UnsubscribeJob` plus Cloud Task due at now plus `UNSUB_DELAY`; `reject_list` rule | SW-03 AC1, AC2, SR-01 AC1 |
| `trashed_unsubscribe_manual` | Trash; `reject_list` rule; Needs Attention item `https_only_unsubscribe` with an "Open unsubscribe page" link taken from the DKIM-covered `List-Unsubscribe` header only (v1 has no page handler) | SW-03, UN-05 |
| `trashed_list_no_unsubscribe` | Trash; `reject_list` rule. No Needs Attention item: body links are never parsed (S2 SW-03 AC5, S6 section 6) | S3 `bulk_no_header` |
| `trashed` | Trash only (personal or notice); personal rejects counted for PB-01 | SW-03 AC4, AC5 |
| `reported_spam` | Report as spam and trash; no unsubscribe | SW-03 AC3 |
| `filed` | Label or category applied, message leaves the inbox, category created if new | SW-04 AC2, FL-02 AC1 |

`filed_category` (`{ "category_id", "name" }`) is set for `filed`, so the toast can name it. `prompts` contains `block_person` when PB-01 AC1 triggers. The app shows the block prompt; Block calls API-RULE-2 with the `prompt_ref`, Not now calls API-RULE-5.

Idempotency: the server derives `job_id` and `rule_id` as UUID v5 of the user ID and the `Idempotency-Key`, and names the Cloud Task after `job_id`. A retried request finds the existing task (`ALREADY_EXISTS`), treats it as success and returns the same result. No idempotency table is needed. Task names must not be derived from the message ID alone: Cloud Tasks blocks reuse of a deleted task's name for some days, so undo then reject again on the same message would fail.

**API-SW-2** `POST /swipes/undo`

```json
{ "undo_token": "opaque" }
```

The undo token is sealed by the server and holds the mailbox ID, message ID, action, the exact previous label or folder set, and any `job_id` and `rule_id`. It is bound to the user and session, so a token from another user or an earlier session is refused (`410 undo_expired`, SW-05 AC4). No undo state is kept on the server; the app keeps the stack of tokens in memory (S3 `UndoStack`).

Response `200`:

```json
{ "restored": true, "unsubscribe_already_sent": false }
```

- Restores the previous label set exactly (S3 swipe and undo). Deletes the Cloud Task and job if still queued and removes the rule (SW-05 AC2). If the job already ran, `unsubscribe_already_sent: true` and the rule is still removed (SW-05 AC3).
- Reports from `reported_spam` are reversed by removing the spam label; the report itself cannot be recalled.
- Undoing a `skip` or `keep` only reverses the sender stats and skip count.
- If the provider refuses the restore, `502 provider_error` and the token stays valid for a retry.

### 5.6 Categories

**API-CAT-1** `GET /categories` → `{ "categories": [{ "category_id", "name", "message_count", "per_mailbox": [{ "mailbox_id", "message_count" }] }] }`. Counts come from provider label counts (FL-05 AC1).

**API-CAT-2** `POST /categories` `{ "name": "Tax invoices" }` → `201` Category. Creates the label or category in each connected mailbox lazily, on first use. `409 category_exists` on a case-insensitive clash.

**API-CAT-3** `PATCH /categories/{id}` `{ "name": "..." }` → `200` Category. Renames the provider label in every mailbox.

**API-CAT-4** `DELETE /categories/{id}` → `204`. Removes the label from mailboxes and deletes filing rules that point at it. Messages are never deleted (S3 INV-5).

**API-CAT-5** `GET /categories/{id}/messages?cursor&limit` → `{ "messages": [{ "mailbox_id", "message_id", "sender_name", "subject", "received_at", "provider_web_url" }], "next_cursor", "mailbox_errors" }`.

### 5.7 Rules and block prompts

**API-RULE-1** `GET /rules?kind=reject_list|block_person|file` → `{ "rules": [Rule] }`. `Rule` is `rule_id`, `kind`, `match` (`{ "sender_address", "list_id" }`, `list_id` nullable), `category_id` (for `file`), `enabled`, `created_at`, `times_applied`, `yearly_rate` (integer estimate of messages a year this rule stops, or `null` when the count query failed; GM-05).

**API-RULE-2** `POST /rules`. Two shapes, both built from a server-sealed reference so the client cannot invent a match:

```json
{ "kind": "block_person", "prompt_ref": "opaque" }
{ "kind": "file", "mailbox_id": "uuid", "message_id": "id", "category_id": "uuid" }
```

The `file` form takes the sender from the named message, re-read from the provider (FL-04 AC2). `reject_list` rules are created only by API-SW-1. Returns `201` Rule.

**API-RULE-3** `PATCH /rules/{id}` `{ "enabled": false }` → `200` Rule (SR-01 AC4).

**API-RULE-4** `DELETE /rules/{id}` → `204`.

**API-RULE-5** `POST /block-prompts/decline` `{ "prompt_ref": "opaque" }` → `204`. Suppresses the prompt for that sender for 90 days `[TUNABLE]` (PB-01 AC3).

### 5.8 Needs Attention

**API-NA-1** `GET /needs-attention?cursor&limit` → `{ "items": [{ "item_id", "mailbox_id", "sender_display", "reason_code", "link", "created_at" }], "open_count", "next_cursor" }`.

- `reason_code` in v1: `https_only_unsubscribe` (a DKIM-covered https link without one-click; v1 does not open it), `one_click_redirect` (the one-click target answered 3xx; no redirect is followed and the job is not retried), `one_click_address_refused` (the target resolved to a private, loopback, link-local, CGNAT or metadata address), `unsubscribe_failed` (one-click or mailto failed after Cloud Tasks retries), `unsubscribe_ignored`, `job_expired`, `sign_in_required` (the mailbox's refresh token was revoked or invalid when the job ran; the item offers "Sign in again", which calls API-AUTH-1 `reconnect`; S2 UN-01 AC6). Reserved for v2 with the page handler: `captcha`, `login_required`, `page_unclear`, `page_failed`. The app maps each to the plain-words copy in S9 section 6.
- `link` is an `https` URL from the `List-Unsubscribe` header, never from the body, or `null`; the server drops any other scheme when it creates the item, so the app never opens `javascript:` or `data:` URLs. The app opens it with `rel="noopener noreferrer"`.
- `open_count` drives the tab badge.

**API-NA-2** `POST /needs-attention/{id}/resolve` and **API-NA-3** `POST /needs-attention/{id}/dismiss` → `204`. Both delete the item (S3 state machine).

### 5.9 History and stats

**API-HIST-1** `GET /history?filter=all|unsubscribes|rule_actions|filing&cursor&limit` → `{ "entries": [{ "entry_id", "at", "mailbox_id", "sender_display", "action", "outcome", "rule_id" }], "next_cursor" }`. `action` is one of `trashed_by_rule`, `unsubscribe`, `filed`, `filed_by_rule`, `blocked`, `reported_spam`; `outcome` is `sent`, `needs_attention`, `failed`, `cancelled`, `expired`, `done` (UN-01 AC3, ST-01). `needs_attention` means the job ended with a Needs Attention item.

**API-STAT-1** `GET /stats` (ST-02):

```json
{
  "emails_triaged": 4210,
  "senders_unsubscribed": 312,
  "unsubscribes_confirmed": 280,
  "mail_stopped_per_year": 3200,
  "achievements": [{ "achievement_id": "first_unsubscribe", "unlocked_at": "2026-10-03T14:09:19Z" }]
}
```

`mail_stopped_per_year` is the sum of `yearly_rate` over enabled rules, ignoring `null` (ST-02 AC2, GM-05). `achievements` lists unlocked ones only; the app holds the fixed list and greys the rest (ST-02 AC3). Streaks are deferred and not in the contract.

### 5.10 Account

**API-ACCT-1** `DELETE /account` → `202` with `{ "deletion_due_by": time }`. Needs step-up. Runs in this order, because each step needs what the next one removes (spec audit M7):

1. In the request: delete the app folder file from each connected Drive or OneDrive while tokens still work; cancel queued jobs and their Cloud Tasks; revoke provider tokens.
2. Delete every session (including this one), clear the cookie and send `Clear-Site-Data: "cache", "storage"`.
3. In the background, within 24 hours: destroy `data_key` (crypto-shredding) and sweep remaining records (AU-06 AC1).

If step 1 cannot reach a provider, deletion still goes ahead, and the `202` body lists `app_folders_not_deleted: [{ "mailbox_id", "email_address" }]` so the app can tell the user which app folder file to remove by hand. A Needs Attention item would be deleted with the account, so the response is the only place to say it.

### 5.11 Admin

**API-ADM-1** `GET /admin/invites?status=pending|used|revoked|expired&cursor&limit` → `{ "invites": [{ "invite_id", "email_address", "status", "created_at", "expires_at", "last_sent_at" }], "next_cursor" }`.

**API-ADM-2** `POST /admin/invites` `{ "email_address": "x@example.com" }` → `201` Invite, or `200` with the existing invite after re-sending it (AU-01 AC2). Address validated and normalised (lower-cased domain; local part kept as given). Invite email sent through the admin's own Gmail (S4).

**API-ADM-3** `POST /admin/invites/{id}/resend` → `200` Invite. A used or revoked invite returns `404 not_found`.

**API-ADM-4** `DELETE /admin/invites/{id}` → `204`. Sets `revoked` (AU-01 AC4). A used or already revoked invite returns `404 not_found`.

**API-ADM-5** `GET /admin/invite-requests?cursor&limit` → `{ "requests": [{ "request_id", "email_address", "created_at" }], "next_cursor" }`.

**API-ADM-6** `POST /admin/invite-requests/{id}/approve` → `201` Invite (AU-02 AC3).

**API-ADM-7** `POST /admin/invite-requests/{id}/decline` → `204`. Deletes the request; the requester is not told (AU-02 AC3).

**API-ADM-16** `GET /admin/users?cursor&limit` → `{ "users": [{ "user_id", "email_address", "created_at", "is_admin", "mailbox_count", "signed_in", "last_seen_at" }], "next_cursor" }`. Feeds the admin user list (S9 section 7.6) and the target of API-ADM-15. `email_address` is the earliest linked mailbox still present (usually the one the user joined with, which may since have been disconnected).

**API-ADM-15** `DELETE /admin/users/{user_id}/sessions` → `204`. Ends that user's session, for a suspected compromise. Queued unsubscribe jobs keep running, because they need no session (section 3.5). Written to the security log.

Every admin write (API-ADM-2 to API-ADM-7, API-ADM-9, API-ADM-11, API-ADM-14, API-ADM-15) needs step-up (section 3.6). API-ADM-2 now also generates the invite token (section 3.4) and puts it in the invite email link; API-ADM-3 issues a new token and voids the old one.

### 5.12 Internal endpoints

Called only by Google Cloud services. No session, no CSRF; each checks a Google-signed OIDC token: signature, `aud` equal to the endpoint's configured audience, and `email` equal to the one caller service account allowed. Anything else gets `401` and a security log entry.

**API-INT-1** `POST /internal/v1/unsubscribe-jobs/{job_id}/run` on the `unsub` service (ingress internal). Body `{}`; the job is loaded by ID. Runs UN-01 to UN-03 and UN-05 and returns:
- `200` when the job reached a terminal state (`sent`, `needs_attention`, `cancelled`). Cloud Tasks stops.
- `503` for a retryable failure while `attempts < 3`. Cloud Tasks retries with backoff (queue config: `maxAttempts` 4, `minBackoff` 30 s, `maxBackoff` 5 min `[TUNABLE]`).
- A missing job (undo raced the task) returns `200` and does nothing.
- Idempotent, using Cloud Tasks' `X-CloudTasks-TaskRetryCount` as the attempt number. A `queued` job is claimed (`running`). A `running` job is reclaimed only by a delivery whose retry count is higher than its recorded `attempts`, so a retry after a `503` or a crash can finish it. A terminal job, or a `running` one redelivered with the same count, returns `200` without acting, so a duplicate delivery never sends twice (UN-01 AC1).

**API-INT-2** `POST /internal/v1/sweep`: moves overdue jobs to `expired` and creates Needs Attention items, deletes expired Needs Attention items and sessions, and deletes terminal jobs whose outcome no Feed load has collected within 30 days (S3). Every 15 minutes `[TUNABLE]`. Returns counts only.

No digest or push endpoint: James decided on 3 October 2026 that v1 sends no push messages and no daily digest email; the user comes to the app, and the Needs Attention tab badge is the only signal. Ways to bring people back through gamification are a separate design question for S9.

`[ASSUMES]` API-INT-2 runs on a separate internal-ingress service (call it `worker`), not on `api`.

### 5.13 Classifier bake-off (CR-01)

James wants every email sent to both Google's classifier (Gemini on Vertex AI, us-central1) and Jev, and to see which is more accurate from the swipes (3 October 2026). There is no user split: every opted-in user's cards go to both models, and the badge stays on header rules so the swipe is an unbiased label for both. Storage and model calls are S4's business; this section is the API side.

**How cards carry the bake-off.** For an opted-in user, API-FEED-1 sends each card's minimised input (CR-01 1.5) to Gemini and Jev in parallel. Neither call is on the card's critical path, because the badge is header rules. A model error, invalid output or timeout (2 s `[TUNABLE]`, CR-01 T-new-3) is recorded for that model and changes nothing on the card; API-FEED-1 never fails because of either model. The server seals into `classification_token` the header-rules result and each model's prediction (class, score, confidence, model version, latency, input tokens, error code), bound to user, session, mailbox and message, under the per-user data key (AES-256-GCM, like undo tokens).

On API-SW-1 the server opens the token and writes one `classifier_eval` record with the swipe direction and time to swipe. On API-SW-2 it marks that record undone, which flips the label (CR-01 1.6). The `eval_id` is a UUID v5 of the user ID and the swipe's `Idempotency-Key`, so a retried swipe writes one record and the undo token can name it; it is never derived from the message ID. Cards never swiped leave nothing. For users who have not opted in, the token carries only the header-rules result, no model is called and no record is written. No sender, subject, text or message ID goes into the record.

**API-EXP-1** `GET /me/experiments`

```json
{
  "classifier_bakeoff": {
    "available": true,
    "opted_in": false,
    "consent_version": null,
    "current_consent_version": "2026-10-03",
    "opted_in_at": null
  }
}
```

`available` is false while both models are switched off. The consent text lives in the app (S9 Settings, Experiments); the API carries only its version.

**API-EXP-2** `PUT /me/experiments`

```json
{ "classifier_bakeoff": { "opted_in": true, "consent_version": "2026-10-03" } }
```

- Opting in needs `consent_version` equal to the current version, otherwise `409 consent_outdated`. While both models are off, opting in returns `409 experiment_unavailable`.
- Opting out takes effect at once: no further Gemini or Jev calls for this user, and the user's `classifier_eval` records are deleted before the response returns (CR-01 section 2). Opting out always works.
- If the consent text version changes, the user is treated as opted out until they accept the new version.
- The consent text (S9 section 7.8, version `2026-10-03`; S9 owns the wording and wins if the two differ) is: "Try an experimental classifier. Card text (sender, subject and the first part of the message) is sent to Google (Vertex AI, United States) and TypeSafe AI (United States) to compare two classifiers. Neither trains on it. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out." Opt-out deletes raw `classifier_eval` records but not snapshots (API-ADM-11), which hold aggregates only.
- Returns `200` with the API-EXP-1 body. Opt-in and opt-out go to the security log (pseudonymous).

**API-ADM-8** `GET /admin/experiments/classifier`

```json
{
  "models": [
    { "model": "gemini", "classifier_id": "gemini@flash-lite", "enabled": true, "changed_at": "2026-10-03T21:00:00Z" },
    { "model": "jev", "classifier_id": "jev@1.13.0", "enabled": true, "changed_at": "2026-10-03T21:00:00Z" }
  ],
  "participants": 12
}
```

**API-ADM-9** `PATCH /admin/experiments/classifier` `{ "models": [{ "model": "jev", "enabled": false }] }` → `200` with the API-ADM-8 body. One kill switch per model (`CLASSIFIER_GEMINI_ENABLED`, `CLASSIFIER_JEV_ENABLED`, plus the Firestore document `config/classifiers` read every minute, CR-01 1.7). Off stops that model's calls within one refresh; the other model carries on. Every change goes to the security log.

**API-ADM-10** `GET /admin/bakeoff` → aggregates only, built for publication (CR-01a).

Query parameters: `from`, `to` (dates, required); `input_version`, `question_version`, `price_version` (optional filters); `pool_versions` (boolean, default `false`).

- If the range holds records from more than one `input_version` or `question_version` and no filter narrows it, the call returns `409 versions_mixed` with the versions present in the body (`versions_present`), unless `pool_versions=true`. Pooled reports say so in `query.pool_versions`.
- `price_version` does not block pooling; cost is computed per record from its own price table, and the filter only narrows the records.

Response (shape; numbers illustrative):

```json
{
  "query": { "from": "2026-10-03", "to": "2026-11-03", "input_version": "1", "question_version": "1", "price_version": null, "pool_versions": false },
  "versions_present": { "input": ["1"], "question": ["1"], "price": ["1", "2"] },
  "min_cell_size": 10,
  "ground_truth_version": "1",
  "interval_method": "wilson_95",
  "labelled_swipes": 8350,
  "participants": { "count": 4, "top_contributor_share": { "value": 0.91, "n": 8350 } },
  "models": [{
    "model": "jev",
    "classifier_id": "jev@1.13.0",
    "cards": 8350,
    "valid_answers": 8312,
    "accuracy": { "value": 0.87, "lower": 0.863, "upper": 0.877, "n": 8312 },
    "junk_precision": { "value": 0.91, "lower": 0.90, "upper": 0.92, "n": 5100 },
    "junk_recall": { "value": 0.89, "lower": 0.88, "upper": 0.90, "n": 5210 },
    "junk_f1": 0.90,
    "false_junk_rate": { "value": 0.06, "lower": 0.052, "upper": 0.069, "n": 3102 },
    "calibration": { "ece": 0.04, "bins": [{ "confidence_from": 0.9, "confidence_to": 1.0, "count": 3100, "observed_accuracy": 0.95 }] },
    "latency": { "p50_ms": 340, "p95_ms": 720, "histogram": [{ "from_ms": 250, "to_ms": 500, "count": 5400 }] },
    "timeout_rate": { "value": 0.004, "lower": 0.003, "upper": 0.006, "n": 8350 },
    "error_rate": { "value": 0.001, "lower": 0.0005, "upper": 0.002, "n": 8350 },
    "input_tokens": 7400000,
    "cost_per_1000_messages_usd": 0.42
  }],
  "paired": [{
    "model_a": "gemini", "model_b": "jev",
    "n_paired": 8290,
    "only_a_correct": 310, "only_b_correct": 420, "both_correct": 6900, "neither_correct": 660,
    "mcnemar_exact_p": 0.00006,
    "accuracy_difference": { "value": -0.013, "lower": -0.024, "upper": -0.003 },
    "bootstrap": { "method": "cluster_by_participant", "resamples": 2000, "seed": 20261003, "participants": 4 }
  }],
  "agreement": [{ "model_a": "gemini", "model_b": "jev", "rate": { "value": 0.81, "lower": 0.80, "upper": 0.82, "n": 8290 } }],
  "by_segment": [{ "dimension": "age_bucket", "value": "1y-5y", "model": "jev", "accuracy": { "value": 0.84, "lower": 0.82, "upper": 0.86, "n": 1900 }, "false_junk_rate": { "value": 0.07, "lower": 0.05, "upper": 0.09, "n": 640 } }],
  "confusion": [{ "model": "jev", "class": "list", "label": "junk", "count": 2900 }],
  "trend": [{ "date": "2026-10-04", "model": "jev", "accuracy": { "value": 0.86, "lower": 0.83, "upper": 0.89, "n": 600 } }]
}
```

Rules for every section:

- **Methods.** `models` always lists `header_rules`, `gemini` and `jev` (CR-01a G1). `classifier_id` follows S3 (`header_rules@1`, `gemini@<model>`, `jev@<version>`). For header rules, confidence is `bulk_score / 100`, which is the probability that the message is bulk, not confidence in the predicted class; calibration for header rules reads it that way. Cost is 0 and latency is measured in process. `agreement` is five-class agreement: the share of paired cards where both models predicted the same S3 class. `paired` covers each pair of the three. `trend` has one row per model per day.
- **Labels.** CR-01 1.6: a left swipe not undone is `junk`; a right or up swipe is `wanted`; an undo flips the label; skips are not labels. `ground_truth_version` names the mapping, which is fixed in code.
- **Intervals.** Every rate is an object `{ value, lower, upper, n }` with a Wilson 95% interval. F1 is reported without an interval.
- **Paired comparison.** Only cards where both models gave a valid answer count (`n_paired`). McNemar's exact (binomial) test on `only_a_correct` and `only_b_correct`. The accuracy difference (`a` minus `b`) has a 95% percentile bootstrap interval resampled by participant; `resamples` and `seed` are in the response so a run can be repeated. With fewer than 5 participants `[TUNABLE]` the cluster bootstrap is meaningless: `accuracy_difference.lower` and `upper` are `null` and `bootstrap.method` is `insufficient_participants`, and the write-up must lean on McNemar and say the data is mostly one inbox.
- **Participants.** `participants.count` and `top_contributor_share` (labels from the largest contributor divided by all labels). Both are `null` when `labelled_swipes` is below `min_cell_size`.
- **Segments.** `by_segment` covers, for each model, the header-rules class, each `header_facts` boolean, `provider`, `age_bucket`, `text_tokens_bucket` and `lang_is_english` (CR-01a G4).
- **Latency histogram.** Fixed bins in ms: 0 to 100, 100 to 250, 250 to 500, 500 to 1,000 and 1,000 to 2,000 `[TUNABLE]`, each including its lower bound and excluding its upper, so the last ends below the 2,000 ms timeout. Timed-out calls are in no bin; they are counted in `timeout_rate`.
- **Suppression.** Any count below `min_cell_size` (10 `[TUNABLE]`) is `null`, and any rate built on it is `null` with its bounds, everywhere including segments, paired counts and trend days. No per-user rows, no pseudonymous IDs, no event-level export.
- **Complementary suppression** (S10 STAT-5). Suppressing one cell is not enough when a total and its other parts are shown: the hidden value is the difference. Within every group that sums to a published total (a segment dimension's values for one model, a confusion row, the four paired counts, a day's trend rows), if exactly one cell is suppressed, the next smallest cell in that group is suppressed too, and the rule repeats until no suppressed cell can be derived by subtraction. Rates are suppressed with their counts. The same pass runs before CSV output and before a snapshot is stored.

**CSV.** `Accept: text/csv` on API-ADM-10 or API-ADM-12 returns one tidy table with header `section,model,segment,metric,value,lower,upper,n` and `Content-Disposition: attachment; filename="bakeoff-report.csv"` (API-ADM-10) or `filename="bakeoff-snapshot.csv"` (API-ADM-12). The filename is fixed ASCII set by the server, never built from query parameters or snapshot names (ASVS V5.4.1; CR-01a G7). `section` is the JSON section name; `model` is a model or `a|b` for pairs; `segment` is `dimension=value` or empty; suppressed cells are empty. Same suppression as JSON, and a test checks the CSV matches the JSON. Any text cell starting with `=`, `+`, `-`, `@`, tab or carriage return is prefixed with `'` so a spreadsheet does not run it as a formula (ASVS V1 output encoding). Other `Accept` values return `406 not_acceptable`.

**API-ADM-11** `POST /admin/bakeoff/snapshots`

```json
{ "name": "Blog draft, first month", "query": { "from": "2026-10-03", "to": "2026-11-03", "input_version": "1", "question_version": "1", "price_version": null, "pool_versions": false } }
```

Computes the API-ADM-10 report for that query (same `409 versions_mixed` rule), applies suppression, and stores it in `bakeoff_snapshots` (aggregate only, no pseudonymous IDs, kept until an admin deletes it). Opt-outs after this point do not change it (CR-01a G6). `name` is 1 to 100 characters of plain text. Returns `201` with the full snapshot: `{ "snapshot_id", "name", "created_at", "labelled_swipes", "report": <API-ADM-10 body> }`. At most 50 snapshots `[TUNABLE]`, otherwise `409 snapshot_limit`.

**API-ADM-12** `GET /admin/bakeoff/snapshots/{snapshot_id}` → `200` with the snapshot, or the CSV of its report with `Accept: text/csv`.

**API-ADM-13** `GET /admin/bakeoff/snapshots?cursor&limit` → `{ "snapshots": [{ "snapshot_id", "name", "created_at", "labelled_swipes" }], "next_cursor" }`, newest first. Feeds the snapshot list in S9 section 7.7.

**API-ADM-14** `DELETE /admin/bakeoff/snapshots/{snapshot_id}` → `204`. Written to the security log.

## 6. Rate limits (ASVS V2 anti-automation)

Limits return `429 rate_limited` with `Retry-After` (seconds) and `retry_after_seconds` in the body. Successful responses on limited routes carry `RateLimit-Policy` and `RateLimit` headers (IETF draft `draft-ietf-httpapi-ratelimit-headers`) so the app can slow down before hitting the wall.

| Scope | Routes | Limit `[TUNABLE]` | Key | Store |
| --- | --- | --- | --- | --- |
| Admin session kill | API-ADM-15 | 20 per day | User | Firestore |
| Admin user list | API-ADM-16 | as other reads | Session | Instance memory |
| Sign-in start and callback | API-AUTH-1, API-AUTH-2 | 20 per 10 minutes per IP; 10 failed callbacks per hour per mailbox subject | IP (`X-Forwarded-For` as set by Google's front end, rightmost trusted hop) | Firestore |
| Invite requests | API-INV-1 | 5 per hour per IP (AU-02 AC4); 1 per day per email | IP, email hash | Firestore |
| Swipes and undo | API-SW-1, API-SW-2 | 60 per minute, burst 10 | User | Instance memory |
| Feed | API-FEED-1 | 30 per minute | User | Instance memory |
| Unsubscribe jobs created | API-SW-1 with a queued job | 300 per day | User | Firestore |
| Mailto unsubscribes sent | API-INT-1, mailto method | 100 per day per mailbox | Mailbox | Firestore |
| Other reads | All other `GET` | 120 per minute | Session | Instance memory |
| Other writes | All other `POST`, `PATCH`, `DELETE` | 30 per minute | User | Instance memory |
| Admin invite sends | API-ADM-2, API-ADM-3, API-ADM-6 | 50 per day | User | Firestore |
| Account deletion | API-ACCT-1 | 3 per day | User | Firestore |
| Experiment opt-in changes | API-EXP-2 | 10 per day | User | Firestore |
| Experiment admin changes | API-ADM-9 | 50 per day | User | Firestore |
| Bake-off report | API-ADM-10, API-ADM-12 with CSV | 30 per hour | User | Instance memory |
| Bake-off snapshots saved | API-ADM-11 | 20 per day | User | Firestore |
| Bake-off snapshots deleted | API-ADM-14 | 50 per day | User | Firestore |

- Instance-memory limits are per Cloud Run instance. With `max-instances` set low for the trial (`[ASSUMES]` 3), the effective limit is at most three times the figure, which is acceptable for abuse by a signed-in invitee. The security-relevant limits (sign-in, invite requests, unsubscribe and mailto volume, admin, deletion) use Firestore counters so they hold across instances.
- The mailto limit protects the user's own mailbox from Gmail sending limits and reputation damage if something loops.
- Provider rate limits (Gmail per-user quota, Graph throttling) come back as `503 provider_unavailable` with the provider's `Retry-After` passed through.
- Edge protection (Cloud Armor) is an open S4 decision. These limits stand on their own either way.

## 7. ASVS Level 2 coverage in this contract

What this contract fixes, by chapter. The full register belongs to S6.

| ASVS 5.0 | Control in this contract | Verification |
| --- | --- | --- |
| V1 Encoding | All mail fields plain text, control and bidi characters stripped; app renders as text | Contract test with hostile fixtures (HTML, RTL override, very long strings); Flutter widget test |
| V2 Validation and business logic | Strict schemas, `deny_unknown_fields`, size limits, server-side classification, sealed references, rate limits | Schema fuzz tests from the OpenAPI file; limit tests per row in section 6 |
| V3 Web frontend | `no-store`, `nosniff`, `frame-ancestors 'none'` on API responses; no CORS | Header assertion tests on every route |
| V4 API | Same-origin JSON API, CSRF token and `Origin` check on unsafe methods, `GET` is safe | CSRF negative tests on every unsafe route (generated from the OpenAPI file) |
| V6 Authentication | Google OAuth as the only sign-in (authentication delegated to Google, including its MFA); account creation needs a single-use invite token plus a matching verified email; step-up is a fresh Google sign-in checked by `auth_time`; accepted risk in section 3.1 | Integration test per outcome in section 3.4 against a fake provider, including stale `auth_time`, wrong `sub` on step-up, reused and expired invite tokens (S10) |
| V7 Session | Opaque cookie, hashed at rest, rotation, idle and absolute timeout, server-side sign-out, one session per user (a new sign-in ends the old one), admin kill, step-up for sensitive actions | Session tests: fixation, expiry, reuse after sign-out, old session refused after a new sign-in, step-up expiry |
| V8 Authorisation | Ownership on every ID, `404` for others' resources, admin flag server-side | Two-user isolation suite run against every route with an ID |
| V9 Tokens | Sealed tokens with type, user, session record and expiry in the associated data (section 2.1); no JWTs to the browser; provider ID tokens fully validated | Unit tests: each token type replayed as each other type, across users and sessions, after expiry; forged and expired ID tokens |
| V10 OAuth | Code flow with PKCE, `state`, `nonce`, exact redirect URIs, tokens server-side only | Integration tests against a fake provider |
| V14 Data protection | No sensitive data in URLs; `no-store`; nothing from mail logged; experiment results aggregate only with small-cell suppression; opt-out deletes `classifier_eval` records; no model call without consent | Log-scanning test (XC-01); URL lint over the OpenAPI paths; API-ADM-10 to API-ADM-12 tests that no fixture string or pseudonymous ID appears, small cells are null in JSON and CSV, and the CSV matches the JSON; mock Gemini and Jev servers assert zero calls for a non-consenting user |
| V16 Logging and errors | Problem Details with fixed titles, request IDs, security events for auth and authorisation failures | Error body snapshot tests; security log assertions |

## 8. Contract testing

- The OpenAPI file is the source for generated tests: every route gets an unauthenticated test, a CSRF test (unsafe methods), a cross-user test (routes with IDs) and a schema round trip.
- The Rust handlers and the Dart client types are generated from or checked against the OpenAPI file in CI, so drift fails the build. Tool choice belongs to S10 `[ASSUMES]`.
- Provider behaviour is faked behind the S8 adapter traits; no real mail in tests.

## 9. Changes for other documents (applied)

Historical record. As of 3 October 2026 every item below has been applied by its owning thread, along with the earlier items, the spec audit fixes and James's answers to audit Q1 to Q4 (`docs/change-requests/audit-knock-ons-S7-S10.md`). Nothing here is open. New cross-spec changes go through `docs/change-requests/`.

1. **S9 copy** for the new outcomes and codes: `not_registered`, `invite_invalid`, `step_up_wrong_account`, the `trashed_unsubscribe_manual` toast ("Trashed. The unsubscribe link is in Needs Attention."), the Needs Attention reasons `https_only_unsubscribe`, `one_click_redirect` and `one_click_address_refused`, and `409 app_folder_move_failed`.
2. **OAuth only for the trial** (James, 3 October 2026). Owners matched:
   - CONTEXT and S6: drop passkeys, PRF, `token_key` and lost-passkey recovery; one KMS-wrapped `data_key` per user; record the accepted risk in section 3.1 and the passkey lock as a hardening step before CASA.
   - S2: AU-03 becomes Google sign-in (drop AC4a to AC4c on passkeys and the lock); UN-01 AC6 (jobs waiting for a session) no longer applies.
   - S3: drop the `Passkey` entity, the session list fields and `awaiting_session`; Session gains `recent_auth_at`; one session per user.
   - S9: drop passkey enrolment, unlock, the Security passkey list and the session list; add the `step_up_wrong_account` and `not_registered` copy.
3. **S10** items are owned by the test strategy thread (same knock-ons file).
4. **Backlog knock-ons** (4 October 2026, `docs/change-requests/backlog-knock-ons-S7-S10.md`): the 13 S7 items are applied in this file and the OpenAPI. History `outcome` gains `needs_attention` rather than mapping it to `failed`. S10 items are owned by the test strategy thread.
