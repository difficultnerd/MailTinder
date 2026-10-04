# T-601a: Mailboxes: list, link and reconnect

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-405, T-503, T-504 |

Split from index row T-601 (list, link and disconnect). Disconnect is T-601b.

**Read only these spec sections:** S7 sections 3.4 (intents `link` and `reconnect`, outcomes table), 3.6 (step-up), 5.2 (API-AUTH-1, API-AUTH-2), 5.3 (API-MBX-1) in `docs/specs/S7-api-contract.md`; `Mailbox` schema in `docs/specs/S7-api-contract.openapi.yaml`; S2 AU-03 AC7, AU-04 AC1 to AC6, ST-03 AC1; S3 `Mailbox` row, "Mailbox status" state machine and INV-3; S5 `mailboxes/{id}` rows. Nothing else is needed.

## Goal

A signed-in user can list their linked mailboxes (API-MBX-1) and link another Gmail mailbox through Google OAuth with the `link` intent, or refresh a `needs_sign_in` mailbox with the `reconnect` intent. After this task the Settings, Connected accounts screen (T-1006) has a real backend.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/mailboxes.rs` | `GET /api/v1/mailboxes` handler and the `MailboxDto` |
| Create | `backend/crates/api/src/services/mailbox_link.rs` | `link` and `reconnect` callback branches |
| Change | `backend/crates/api/src/routes/auth.rs` | Accept `link` and `reconnect` in API-AUTH-1; call the new branches from API-AUTH-2 |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount the mailboxes route |
| Create | `backend/crates/api/tests/mailboxes_list_link.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks. Keep the merged names; if a merged name differs from
// the one below, use the merged one and say so in the pull request.
// T-500: AppState { ports: Ports, config: ApiConfig }, ApiError (S7 section 4 codes)
// T-501: AuthedSession { user: UserId, session_record_id: Uuid, is_admin: bool, recent_auth_at: Option<OffsetDateTime> }
//        rotate_session(&AppState, &AuthedSession) -> Result<SetCookie, ApiError>
// T-502: OAuthStart / OAuthCallback machinery, GMAIL_SCOPES constant (S8), IdClaims
// T-503: TokenService::store_refresh_token(user, mailbox, Sensitive<String>)
// T-504: require_step_up(&AuthedSession, &dyn Clock) -> Result<(), ApiError>
// T-307: security_event(SecurityEvent { action, outcome, user, request_id })

// backend/crates/api/src/routes/mailboxes.rs
#[derive(Serialize)]
pub struct MailboxDto {
    pub mailbox_id: Uuid,
    pub provider: &'static str,          // "gmail" in v1
    pub email_address: String,           // decrypted for the response only
    pub status: &'static str,            // "connected" | "needs_sign_in" | "consent_blocked"
    pub is_primary: bool,
    pub linked_at: OffsetDateTime,       // RFC 3339 via time::serde::rfc3339
}
#[derive(Serialize)]
pub struct MailboxList { pub mailboxes: Vec<MailboxDto> }

pub async fn list_mailboxes(State(app): State<AppState>, session: AuthedSession)
    -> Result<Json<MailboxList>, ApiError>;

// backend/crates/api/src/services/mailbox_link.rs
pub enum LinkOutcome { Linked, Reconnected, MailboxLinkedElsewhere, Failed }
impl LinkOutcome { pub fn as_code(&self) -> &'static str } // "linked", "reconnected", "mailbox_linked_elsewhere", "failed"

pub async fn complete_link(app: &AppState, session: &AuthedSession, claims: &IdClaims,
    refresh: Sensitive<String>) -> Result<LinkOutcome, ApiError>;
pub async fn complete_reconnect(app: &AppState, session: &AuthedSession, mailbox: &MailboxId,
    claims: &IdClaims, refresh: Sensitive<String>) -> Result<LinkOutcome, ApiError>;
```

Store names come from T-201b: `mailboxes().by_user(&UserId)` (linked_at ascending), `mailboxes().by_subject(Provider, &ProviderSubjectId)`, `mailboxes().get`, `mailboxes().put(&MailboxRecord, Precondition)`, and `mailbox_id_for(Provider, &ProviderSubjectId)` (UUID v5 of provider and `sub`). Uniqueness on (`provider`, `sub`) comes from creating with `Precondition::MustNotExist` on that derived ID; no extra collection.

## Algorithm

API-MBX-1 `GET /mailboxes`:

1. `mailboxes().by_user(session.user)` (already `linked_at` ascending).
2. For each, decrypt `email_address` with `KeyService::open` (AAD: user, scope = mailbox ID, field `"email_address"`).
3. Map `Provider::Gmail` to `"gmail"` with an exhaustive `match` (no `_` arm). Map status the same way.
4. Return `200 { mailboxes }`. Never return the refresh token or `provider_subject_id`.

API-AUTH-1 with `intent: "link"`:

1. Needs an `authenticated` session, else `401 unauthenticated`.
2. `require_step_up` first; without it `403 step_up_required` and no OAuth state is written (AU-04 AC6).
3. Store `state`, `nonce`, PKCE verifier and intent `link` on the session record (T-502 does this for other intents; reuse it). Scopes are exactly `GMAIL_SCOPES` (AU-04 AC5). Return `{ authorization_url }`.

API-AUTH-1 with `intent: "reconnect"`:

1. Needs `authenticated`. `mailbox_id` is required; it must belong to the user, else `404 not_found`. No step-up (S7 3.6 does not list it).
2. Store the mailbox ID with the OAuth state. Add `login_hint` with the mailbox address. Same scopes.

API-AUTH-2 callback, intent `link` (`complete_link`):

1. T-502 has already validated `state`, exchanged the code and verified the ID token; `email_verified` must be true, else outcome `email_unverified`.
2. `by_subject(Gmail, claims.sub)`:
   - Belongs to another user: outcome `mailbox_linked_elsewhere`. Drop the new tokens from memory. Do not store them and do not revoke them (AU-04 AC3).
   - Belongs to this user: treat as reconnect (store the new refresh token, status `connected`), outcome `linked`.
   - None: create a `MailboxRecord` with `mailbox_id` = `mailbox_id_for(Gmail, sub)`, `provider` Gmail, `provider_subject_id` = `sub` (never the email, AU-03 AC7), encrypted `email_address`, `status` connected, `linked_at` = `Clock::now`, `is_primary` = true only if the user has no other mailbox. Write with `put(.., Precondition::MustNotExist)`; `StoreError::AlreadyExists` means a race: re-read and apply the two bullets above.
3. Store the refresh token through `TokenService::store_refresh_token` (AAD user plus mailbox ID plus `"refresh_token"`).
4. Set `recent_auth_at` = now (S7 3.6: a `link` callback also sets it) and rotate the session ID (S7 3.2). `session_record_id` stays the same.
5. Security event `mailbox_link` with outcome. Redirect to `/#/auth/result?outcome=<code>`.

API-AUTH-2 callback, intent `reconnect` (`complete_reconnect`):

1. Load the mailbox named in the OAuth state; it must still belong to the user.
2. `claims.sub` must equal the mailbox's `provider_subject_id`; otherwise outcome `failed`, tokens dropped.
3. Store the new refresh token and set status connected (`put` with `Precondition::Matches(version)`), outcome `reconnected`. Security event `mailbox_reconnect`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-04 AC1 | A signed-in user with step-up links a Gmail mailbox through OAuth `link` and it is listed |
| AU-04 AC2 | Three Gmail mailboxes linked to one user are all listed, each with its own address |
| AU-04 AC3 | A mailbox already linked to another user is refused with `mailbox_linked_elsewhere` and nothing is stored |
| AU-04 AC5 | The `link` authorisation URL requests exactly the S8 Gmail scopes |
| AU-04 AC6 | `link` without a fresh step-up returns `403 step_up_required` and links nothing |
| AU-03 AC7 | The mailbox is keyed by Google `sub`; a changed email with the same `sub` is the same mailbox |
| ST-03 AC1 | API-MBX-1 lists provider, address and status for every linked mailbox |
| INV-3 | A mailbox belongs to exactly one user, even when two users race to link it |

## Tests that must pass

- `au_04_ac1_link_adds_mailbox` (service integration, `api`)
- `au_04_ac2_three_mailboxes_listed_with_addresses` (service integration)
- `au_04_ac3_mailbox_of_other_user_refused` (service integration; asserts no refresh token stored for the second user and no revoke call recorded by `fake-google`)
- `au_04_ac5_link_requests_only_s8_scopes` (service integration)
- `au_04_ac6_link_without_step_up_refused` (service integration)
- `au_03_ac7_mailbox_keyed_by_sub_not_email` (service integration)
- `st_03_ac1_list_shows_provider_address_status` (service integration; one mailbox set to `needs_sign_in` shows that status)
- `st_03_ac1_reconnect_sets_connected` (service integration)
- `st_03_ac1_reconnect_with_other_account_fails` (service integration)
- `inv_3_concurrent_link_one_owner` (service integration; two tasks call `complete_link` for the same `sub` for two users, exactly one wins)
- `asvs_v8_2_2_reconnect_other_users_mailbox_not_found` (service integration)

## Edge cases and traps

- Never revoke a token on `mailbox_linked_elsewhere`: Google revokes the whole grant, which would break the other user's mailbox.
- Never key or look up a mailbox by email address; only by (`provider`, `sub`).
- The response holds addresses: it must carry `Cache-Control: no-store` (T-500 middleware) and nothing may be logged with the address.
- `match` on `Provider` and `MailboxStatus` with every arm written out; no `_` arm (CONVENTIONS).
- `is_primary` is set only for a user's first mailbox. Linking never moves the app folder file.
- The `mailbox_id` for `reconnect` comes in the JSON body, never the URL (S7 principle 3).
- Uniqueness comes from `MustNotExist` on `mailbox_id_for`, never from a read followed by an unconditional write.

## Out of scope

- Disconnecting a mailbox and moving the app folder file: T-601b.
- Microsoft (`graph`, `consent_blocked` producing code): v2.
- The step-up flow itself: T-504.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The OpenAPI `Mailbox` schema round-trips against `MailboxDto` (no extra or missing fields).
