# T-505: Invites and invite requests

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | sonnet | about 450 lines of code plus tests | T-107, T-303, T-404, T-504 |

**Read only these spec sections:** S7 section 5.11 API-ADM-1 to API-ADM-7, section 3.4 bullet "Invite token", section 3.2 row `pending_invite_request`, section 6 rows "Invite requests" and "Admin invite sends" (`docs/specs/S7-api-contract.md`); the OpenAPI paths `/invite-requests`, `/admin/invites`, `/admin/invites/{invite_id}/resend`, `/admin/invites/{invite_id}`, `/admin/invite-requests`, `.../approve`, `.../decline` and schema `Invite` (`docs/specs/S7-api-contract.openapi.yaml`); S2 AU-01 AC1 to AC5, AU-02 AC1 to AC4 (`docs/specs/S2-v1-acceptance-criteria.md`); S5 `invites` and `invite_requests` rows and INV-T1, INV-T2 (`docs/specs/S5-data-inventory.md`); S9 section 2 (`docs/specs/S9-functional-screens.md`); `docs/backlog/T-404-gmail-send-validated-mailto.md` `InviteMailer`; `docs/backlog/T-502b-google-sign-in-and-invite-redemption.md` `email_lookup_hash`. Nothing else is needed.

## Goal

James can invite a person by email (the invite email goes from his own Gmail with a single-use link), re-send or revoke an invite, see invite requests and approve or decline them; a person who signed in without an invite can submit a request. Every admin write needs step-up and is logged.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/invites.rs` | API-ADM-1 to API-ADM-4 and the shared `issue_invite` |
| Create | `backend/crates/api/src/routes/invite_requests.rs` | API-INV-1, API-ADM-5 to API-ADM-7 |
| Change | `backend/crates/api/src/state.rs` | `AppState` gains `pub invite_mailer: Arc<dyn InviteMailer>` (Gmail adapter in production, `FakeInviteMailer` in tests) |
| Change | `backend/crates/api/src/lib.rs` | Mount the seven routes; add templates |
| Create | `backend/crates/api/tests/invites.rs` | Service integration tests |

## Types and signatures

```rust
// routes/invites.rs
pub const INVITE_TTL: Duration = Duration::days(7);          // [TUNABLE] S2 glossary
pub const INVITE_PURGE_AFTER: Duration = Duration::days(30); // S5: until used, revoked or expired, then 30 days

#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct CreateInvite { pub email_address: String }
#[derive(Serialize)]
pub struct InviteDto { pub invite_id: Uuid, pub email_address: String, pub status: &'static str,
                       pub created_at: String, pub expires_at: String, pub last_sent_at: String } // RFC 3339 UTC
#[derive(Serialize)] pub struct InviteList { pub invites: Vec<InviteDto>, pub next_cursor: Option<String> }

pub enum IssueResult { Created(InviteDto), Resent(InviteDto) }
/// Create a new invite for `email`, or re-send the pending one with a new token. Sends the email.
pub async fn issue_invite(state: &AppState, admin: &AuthedSession, email: &EmailAddress, request_id: RequestId) -> Result<IssueResult, ApiError>;

// routes/invite_requests.rs
#[derive(Serialize)] pub struct InviteRequestDto { pub request_id: Uuid, pub email_address: String, pub created_at: String }
#[derive(Serialize)] pub struct InviteRequestList { pub requests: Vec<InviteRequestDto>, pub next_cursor: Option<String> }
```

## Algorithm

### Shared rules

- Every admin route uses `AdminSession` (reads) or `SteppedUpAdmin` (writes) from T-501 and T-504. Non-admin: `403 forbidden`, logged (AU-01 AC3).
- Email addresses are sealed with `SystemKeyService` (`SystemAad { scope: <record id>, field: aad_fields::INVITE_EMAIL }` or `INVITE_REQUEST_EMAIL`) and looked up by `email_lookup_hash` (T-502b), so the same address always gives the same hash.
- Cursors: `StoreCursor` is sealed with T-303 `SealedTokens` (`TokenType::Cursor`) under the admin's `data_key` before it leaves, and opened on the way in; a bad cursor gives `400 invalid_request`. `limit` 1 to 50, default 20.
- Display status: a stored `pending` invite whose `expires_at <= now` is shown as `expired` `[DEFAULT]` (the sweeper may not have run yet).

### `issue_invite(state, admin, email)`

1. `hash = email_lookup_hash(config.email_lookup_key, email)`.
2. `existing = invites().by_email_lookup(&hash)`; pick the one with status `Pending` (there is at most one).
3. Token: `raw = base64url(rng.bytes32())` (43 characters); `token_hash = SHA-256(raw.as_bytes())`, exactly as T-502b hashes the presented token.
4. If a pending invite exists (re-send, AU-01 AC2): set `token_hash`, `last_sent_at = now`, `expires_at = now + INVITE_TTL`, `purge_at = expires_at + INVITE_PURGE_AFTER`; put with `Matches(version)`. The old token stops working because its hash is gone. Result `Resent`.
5. Else create: `invite_id = rng.uuid_v4()`, `email_address = system_keys.seal(..)`, `email_lookup = hash`, `status = Pending`, `created_at = last_sent_at = now`, `expires_at`, `purge_at` as above; put with `MustNotExist`. Result `Created`.
6. Send: `ctx = state.tokens.mailbox_ctx(state, &admin.user, &<admin's primary mailbox>)`; `link = InviteLink(Url::parse(&format!("{}/#/invite?t={}", config.app_origin, raw)))`; `state.invite_mailer.send_invite(&ctx, email, &link)`. A send error maps through `From<MailError>` (`502` or `503`); the record stays, and the admin can re-send (which issues another token).
7. `security_event { action: "invite_create", outcome: "created" | "resent", user: pseudo(admin) }`.
8. Drop `raw` immediately; it is never stored, logged or returned.

### API-ADM-2 `POST /admin/invites`

1. `SteppedUpAdmin`; rate limit `ADMIN_INVITE_SENDS` by user.
2. `EmailAddress::parse(body.email_address)`; failure gives `400 invalid_request` with `fields: ["/email_address"]`.
3. `issue_invite`; `Created` gives `201`, `Resent` gives `200`, body `InviteDto`.

### API-ADM-3 `POST /admin/invites/{id}/resend`

1. `SteppedUpAdmin`; rate limit `ADMIN_INVITE_SENDS`.
2. Load the invite; missing or status not `Pending`: `404 not_found` `[DEFAULT]` (no S7 code fits; a used or revoked invite cannot be re-sent).
3. Open the email; run `issue_invite` (takes the re-send branch). `200`.

### API-ADM-4 `DELETE /admin/invites/{id}`

1. `SteppedUpAdmin`.
2. Missing: `404`. `Pending`: `InviteState::apply(InviteEvent::Revoke)` (T-107), store `status = Revoked`, `purge_at = now + INVITE_PURGE_AFTER`, with `Matches(version)`. Already `Revoked`: no change. `Used` or `Expired`: `404` `[DEFAULT]`.
3. `security_event { action: "invite_revoke" }`. `204`.

### API-ADM-1 `GET /admin/invites?status&cursor&limit`

`AdminSession`; `invites().list(status, page)`; open each email; map to `InviteDto` (display status rule). `200`.

### API-INV-1 `POST /invite-requests` (body `{}`)

1. `PendingInviteSession` (T-501); anything else `401`.
2. Rate limit `INVITE_REQUEST_IP` by `ClientIp` (AU-02 AC4: the sixth in an hour gets `429`).
3. Open `pre_auth.pending_email` (T-501 `open_pre_auth`); parse as `EmailAddress`.
4. Rate limit `INVITE_REQUEST_EMAIL` by `Email(lookup_form)`.
5. If `invite_requests().by_email_lookup(hash)` exists, or a pending invite for the hash exists: do nothing.
6. Else create `InviteRequestRecord { request_id: rng.uuid_v4(), email_address: sealed, email_lookup: hash, created_at: now, status: Pending }` with `MustNotExist`.
7. `202` with no body, the same in every case (no account or request enumeration). The session stays `PendingInviteRequest` until its 10-minute life ends.

### API-ADM-5 `GET /admin/invite-requests?cursor&limit`

`AdminSession`; `invite_requests().list(page)` (oldest first); open emails; `200`.

### API-ADM-6 `POST /admin/invite-requests/{id}/approve`

1. `SteppedUpAdmin`; rate limit `ADMIN_INVITE_SENDS`.
2. Load the request (`404` if missing); open the email.
3. `issue_invite` (AU-01 AC1 applies); then delete the request with `Precondition::None`.
4. `security_event { action: "invite_request_approve" }`. `201` with `InviteDto`.

### API-ADM-7 `POST /admin/invite-requests/{id}/decline`

1. `SteppedUpAdmin`. Delete the request (`404` if missing). The requester is not told.
2. `security_event { action: "invite_request_decline" }`. `204`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-01 AC1 | Admin invite creates a record and emails a single-use, hashed, expiring sign-in link |
| AU-01 AC2 | Inviting the same address again re-sends with a new token; the old token stops working |
| AU-01 AC3 | A non-admin gets 403 and a security event |
| AU-01 AC4 | A revoked invite cannot be used to sign in |
| AU-01 AC5 | Invite, re-send, revoke, approve and decline need step-up |
| AU-02 AC2 | Requests are listed with address and time for the admin |
| AU-02 AC3 | Approve converts the request into an invite; decline deletes it silently |
| AU-02 AC4 | More than 5 requests from one IP in an hour gives 429 |
| INV-T1 | Invite records hold only the token hash and carry their purge time |
| INV-T2 | Declined requests are deleted; approved ones become invites |
| V6.4.1 | Invite tokens are 256-bit, single use, stored only as SHA-256, 7-day expiry |
| V2.4.1 | Invite sends and invite requests are rate limited |

## Tests that must pass

Service integration with fakes (`FakeInviteMailer` records `(to, link)`), virtual clock, an admin seeded with a stepped-up session helper.

- `au_01_ac1_invite_created_and_email_sent_with_token_link`
- `au_01_ac1_stored_hash_matches_link_token_and_expires_in_7_days`
- `au_01_ac2_reinvite_resends_new_token_old_refused` (redeem the old token through T-502b's flow: `invite_invalid`)
- `au_01_ac3_non_admin_invite_403_and_logged`
- `au_01_ac4_revoked_invite_sign_in_refused`
- `au_01_ac5_each_admin_write_without_step_up_refused` (ADM-2, 3, 4, 6, 7)
- `au_02_ac2_admin_lists_requests_with_address_and_time`
- `au_02_ac3_approve_creates_invite_and_deletes_request`
- `au_02_ac3_decline_deletes_request`
- `au_02_ac4_sixth_request_from_ip_in_hour_429`
- `inv_t1_no_raw_token_in_store_dump`
- `inv_t1_purge_at_set_on_create_and_revoke`
- `inv_t2_decline_leaves_no_record`
- `asvs_v6_4_1_invite_token_256_bits_hash_only`
- `asvs_v2_4_1_admin_invite_sends_limited_50_per_day`
- `invite_request_same_response_whether_or_not_exists`
- `invite_request_needs_pending_state_401_otherwise`
- `invite_list_shows_expired_for_pending_past_expiry`
- `invite_cursor_from_other_admin_or_tampered_is_400`

## Edge cases and traps

- Never return, log or store the raw invite token; only the email link carries it.
- Hash the token exactly as T-502b does (SHA-256 of the 43-character string bytes), or no invite will ever redeem.
- Use `email_lookup_hash` from T-502b for every lookup so case differences do not create duplicates.
- `API-INV-1` must answer the same way whether a request or invite already exists.
- Write the invite record before sending; if the send fails the admin re-sends, which voids the unsent token.
- Admin routes must check the admin flag before step-up (T-504 `SteppedUpAdmin` does this).
- No email address in any log line, error body or security event.

## Out of scope

- Redeeming invites at sign-in: T-502b. The invite email body and Gmail send: T-404.
- Purging old invite records: T-706. The admin screens: T-1007.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The seven routes match the OpenAPI paths and schemas listed above.
