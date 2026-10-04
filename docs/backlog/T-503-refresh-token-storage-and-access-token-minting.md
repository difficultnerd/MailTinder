# T-503: Refresh token storage and access token minting

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | strong | about 350 lines of code plus tests | T-301, T-302, T-502a |

**Read only these spec sections:** S6 section 5 table rows `data_key` and the paragraph "Access tokens" (`docs/specs/S6-security.md`); S3 `Mailbox` row and "Mailbox status" state machine (`docs/specs/S3-domain-model.md`); S5 `mailboxes/{id}` refresh token row and JOB-1, DEL-3 (`docs/specs/S5-data-inventory.md`); S2 UN-01 AC4, AC6, ST-03 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 7.2 row "Crypto" and 6.3 row "Jobs without a session" (`docs/specs/S10-test-strategy.md`); ASVS register rows V10.1.1, V11.3.3, V7.6.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-701-unsub-service-skeleton.md` `svc-common/src/mint.rs` signature and its `mint_access_token` steps. Nothing else is needed.

## Goal

Refresh tokens are stored only as ciphertext under the user's `data_key`, bound to user, mailbox and field, and a mailbox access token is minted from them on demand, in memory only. The minting code lives in a new shared crate `svc-common`, so `api`, `unsub` (T-701) and `worker` (T-706) use one implementation. `api` gets a thin `TokenService` that maps mint failures to S7 errors. A revoked or invalid refresh token moves the mailbox to `needs_sign_in`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/svc-common/Cargo.toml` | New library crate, depends on `domain`, `ports`, `obs`; add to workspace members |
| Create | `backend/crates/svc-common/src/lib.rs` | `pub mod mint;` (T-701 adds its other modules to this file) |
| Create | `backend/crates/svc-common/src/mint.rs` | `seal_refresh_token`, `open_refresh_token`, `mint_access_token`, `MintError` |
| Change | `backend/Cargo.toml` | `crates/svc-common` in `members` |
| Create | `backend/crates/api/src/tokens.rs` | `TokenService` |
| Change | `backend/crates/api/src/lib.rs` | `pub mod tokens;` |
| Create | `backend/crates/svc-common/tests/mint.rs` | Integration tests with fakes and `fake-google` |
| Create | `backend/crates/api/tests/tokens.rs` | Mapping tests |

T-701's file says it creates `svc-common`; with this task merged first, T-701 only adds `internal_auth`, `needs_attention` and `job_record` to the existing crate and keeps `mint.rs` as is.

## Types and signatures

```rust
// svc-common/src/mint.rs (signature agreed with T-701)
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    #[error("refresh token revoked or invalid")] Revoked,   // invalid_grant: mailbox set to needs_sign_in
    #[error("mailbox missing")] MailboxMissing,             // also: mailbox not owned by `user`
    #[error("transient")] Transient,                        // identity provider 5xx, network, store unavailable
    #[error("crypto")] Crypto,                              // KeyService failure; never retried silently
}

/// Ciphertext for MailboxRecord.refresh_token. Aad { user, scope: mailbox_id string, field: aad_fields::MAILBOX_REFRESH_TOKEN }.
pub async fn seal_refresh_token(ports: &Ports, user: &UserRecord, mailbox: &MailboxId, token: &Sensitive<String>) -> Result<Ciphertext, MintError>;
pub async fn open_refresh_token(ports: &Ports, user: &UserRecord, mailbox: &MailboxRecord) -> Result<Sensitive<String>, MintError>;
/// Loads user and mailbox, opens the refresh token, calls IdentityProvider::refresh. Never stores the access token.
pub async fn mint_access_token(ports: &Ports, user: &UserId, mailbox: &MailboxId) -> Result<MailboxCtx, MintError>;

// api/src/tokens.rs
pub const ACCESS_TOKEN_CACHE_S: i64 = 300;   // [DEFAULT] in-process only; Google access tokens live 3600 s
pub struct TokenService { /* cache: Mutex<HashMap<MailboxId, (Sensitive<String>, OffsetDateTime)>> */ }
impl TokenService {
    pub fn new() -> Self;
    /// Seals and writes the refresh token on an existing mailbox record (conditional on its version),
    /// and sets status Connected. Used by join, link and reconnect.
    pub async fn store_refresh_token(&self, state: &AppState, user: &UserId, mailbox: &MailboxId, token: Sensitive<String>) -> Result<(), ApiError>;
    /// Access token for one of the user's mailboxes, for a provider call in this request.
    pub async fn mailbox_ctx(&self, state: &AppState, user: &UserId, mailbox: &MailboxId) -> Result<MailboxCtx, ApiError>;
    /// Decrypted refresh token, only for revocation on disconnect or deletion (T-601b, T-803).
    pub async fn refresh_token(&self, state: &AppState, user: &UserId, mailbox: &MailboxId) -> Result<Sensitive<String>, ApiError>;
    /// Drop the cached access token, after a provider 401 or on disconnect.
    pub fn forget(&self, mailbox: &MailboxId);
}
```

`AppState` gains `pub tokens: Arc<TokenService>` in this task.

## Algorithm

### Storing

1. `seal_refresh_token`: `ports.keys.seal(&user.user_id, &user.wrapped_data_key, &Aad { user: user.user_id, scope: mailbox.0.to_string(), field: aad_fields::MAILBOX_REFRESH_TOKEN }, token.expose().as_bytes())`. `KeyError` gives `Crypto`.
2. `TokenService::store_refresh_token`: load the user and mailbox; the mailbox's `user_id` must equal `user` (else `NotFound`, INV-3); set `refresh_token = Some(ciphertext)`, `status = Connected`; put with `Precondition::Matches(version)`; on `PreconditionFailed` re-read and retry once. Then `forget(mailbox)`.
3. The refresh token is never written anywhere else: not to a job, a session, a log or a response.

### Minting (`svc-common`)

1. `users().get(user)`; missing gives `MailboxMissing`.
2. `mailboxes().get(mailbox)`; missing, or `record.user_id != *user`, gives `MailboxMissing`.
3. `refresh_token` is `None` or status is `NeedsSignIn`: `Revoked` (nothing to try).
4. `open_refresh_token` with the same associated data as sealing; `KeyError::Unavailable` gives `Transient`, any other `KeyError` gives `Crypto`.
5. `ports.identity.refresh(&token)`:
   - `Ok(access)`: return `MailboxCtx { mailbox, access_token: access }`.
   - `Err(IdError::InvalidGrant)`: set the mailbox `status = NeedsSignIn` with `Precondition::Matches(version)` (ignore `PreconditionFailed`: someone else already changed it), write `security_event { action: "sign_in_required", outcome: "token_invalid", user: pseudo }`, return `Revoked` (UN-01 AC6, ST-03 AC1, V7.6.1).
   - Any other error: `Transient`.
6. Do not delete the stored refresh token on `Revoked`; reconnect (T-601a) replaces it, disconnect (T-601b) deletes it.

### `TokenService::mailbox_ctx` (api)

1. Cached entry for the mailbox younger than `ACCESS_TOKEN_CACHE_S` by `Clock` and the mailbox belongs to `user` (ownership re-checked through the store on a cache miss only; the cache key includes the user ID): return it.
2. Else `svc_common::mint::mint_access_token`; cache the result with `now`.
3. Errors: `Revoked` gives `ApiError::MailboxNeedsSignIn { mailbox_id }`; `MailboxMissing` gives `NotFound`; `Transient` gives `ProviderUnavailable { mailbox_id, retry_after_s: None }`; `Crypto` gives `Internal`.
4. A route that gets `MailError::Unauthorized` from Gmail calls `forget(mailbox)` and may retry once with a fresh mint `[DEFAULT]`; a second `Unauthorized` gives `MailboxNeedsSignIn`.
5. The cache is process memory only (S5 C3: "memory only once unwrapped"); it is never serialised, logged or shared with `unsub`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-01 AC4 | Access tokens are minted from the stored refresh token on demand; none is stored |
| UN-01 AC6 | A revoked or invalid refresh token never fails silently: the mailbox becomes `needs_sign_in` |
| ST-03 AC1 | Mailbox status `needs_sign_in` is set when the token is invalid |
| INV-3 | A token is opened only for the mailbox's own user |
| JOB-1 | No access token is written to any store record (minting side) |
| DEL-2 | A copied ciphertext cannot be opened without the user's live `data_key` (minting side) |
| V7.6.1 | A revoked Google grant moves the mailbox to `needs_sign_in` |
| V10.1.1 | Provider tokens stay in the backend |
| V11.3.3 | Refresh tokens are AEAD-sealed with user, mailbox and field as associated data |

## Tests that must pass

- `un_01_ac4_mint_returns_fresh_token_and_stores_nothing` (service integration: fake store dump has no access token string)
- `un_01_ac6_invalid_grant_sets_needs_sign_in` (service integration with `fake-google` `revoke_all_for`)
- `st_03_ac1_status_needs_sign_in_after_revocation` (service integration)
- `inv_3_mint_for_other_users_mailbox_is_missing` (service integration)
- `job_1_store_never_holds_access_token` (service integration: after a mint and a store, scan every record)
- `del_2_ciphertext_unusable_without_the_users_key` (service integration: keep a copy of the mailbox record, replace the user's `wrapped_data_key` with a new one, `open_refresh_token` on the copy fails with `Crypto`)
- `asvs_v7_6_1_revoked_grant_needs_sign_in_and_logged` (service integration, `obs::capture`)
- `asvs_v10_1_1_refresh_token_never_in_log_or_record_clear` (service integration: canary refresh token string absent from logs and from the store dump in clear)
- `asvs_v11_3_3_refresh_token_bound_to_mailbox_and_user` (service integration: ciphertext copied to another mailbox, or to another user's mailbox, fails to open)
- `api_mailbox_ctx_maps_errors` (unit table over `MintError` to `ApiError`)
- `api_mailbox_ctx_cache_expires_after_300_s` (service integration, virtual clock, counts `refresh` calls on the fake identity)
- `api_store_refresh_token_sets_connected` (service integration)

## Edge cases and traps

- The associated data scope is the mailbox ID string; using the user ID alone would let a token be swapped between a user's mailboxes.
- Re-check ownership: a mailbox ID from a request body is untrusted (T5 cross-tenant).
- Never put the access token on a job, session or response; `MailboxCtx` lives for one request.
- Distinguish `InvalidGrant` from network errors; marking a mailbox `needs_sign_in` on a timeout would wrongly ask users to sign in again.
- `Sensitive::expose()` only at the seal and refresh call sites.
- `svc-common` must not depend on `api`, `unsub` or `worker`.
- S7 section 3.5 says jobs hold "a short-lived access token minted at queue time"; S3, S5 and S6 say no access token is stored on a job. This task follows S3, S5 and S6.

## Out of scope

- Linking, reconnecting and disconnecting mailboxes: T-601a, T-601b. Creating the first mailbox on join: T-502b (it calls `store_refresh_token`).
- `unsub`'s use of `mint_access_token`: T-701, T-703.
- Destroying `data_key` on deletion: T-803.

## Security review checklist

- Seal and open use identical `Aad` built only from the user ID, mailbox ID and the `aad_fields` constant.
- No code path writes an access token to Firestore, logs, an error body or a response; the api cache is in memory with a short life and is keyed by user and mailbox.
- `invalid_grant` is the only error that changes mailbox status, the change is conditional, and it is logged as a security event.
- Ownership (`mailbox.user_id == user`) is checked before decrypting.
- `KeyError` details never reach the client; `Crypto` maps to `500 internal_error`.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `svc-common` builds on its own and is listed in the workspace; `api` uses it only through `TokenService`.
