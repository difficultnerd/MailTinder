# T-107: Needs Attention, invite and mailbox state machines

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 300 lines of code plus tests | T-101 |

**Read only these spec sections:** S3 "State machines": `NeedsAttentionItem`, `Invite`, "Mailbox status"; S3 entity rows `Invite`, `InviteRequest`, `NeedsAttentionItem`, INV-2 (`docs/specs/S3-domain-model.md`); S2 AU-01 AC1, AC2, AC4, AU-03 AC2, AC3, AC6, UN-01 AC6, UN-05 AC1, NA-01 AC2, ST-03 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 3.4 outcome table (`invite_invalid`, `email_mismatch`, `email_unverified`) and 5.8 first two bullets (`docs/specs/S7-api-contract.md`). Nothing else is needed.

## Goal

Three small pure state machines in `domain`: the Needs Attention item (open until done, dismissed or 30 days, every exit deletes it), the invite (pending, used, revoked, expired, with re-send replacing the token) including the redemption check that needs both a valid token and a matching verified email, and the mailbox status (connected and needs sign-in, with consent blocked kept for v2). The api and worker tasks call these instead of writing their own status logic.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/needs_attention.rs` | `NeedsAttentionId`, `NeedsAttentionReason`, `NewNeedsAttention`, item rules |
| Create | `backend/crates/domain/src/invite.rs` | `InviteId`, `InviteRequestId`, `InviteStatus`, `InviteState`, `InviteEvent`, `check_redemption` |
| Create | `backend/crates/domain/src/mailbox_status.rs` | `MailboxEvent`, `next_status` for T-101's `MailboxStatus` |
| Change | `backend/crates/domain/src/lib.rs` | Modules and re-exports |

## Types and signatures

```rust
// needs_attention.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NeedsAttentionId(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsAttentionReason {
    HttpsOnlyUnsubscribe, OneClickRedirect, OneClickAddressRefused,
    UnsubscribeFailed, UnsubscribeIgnored, JobExpired,
    SignInRequired,              // UN-01 AC6; not in S7 5.8's list (spec gap, see Edge cases)
}

#[derive(Clone, PartialEq, Eq)]   // Debug by hand: reason and has_link only
pub struct NewNeedsAttention { pub reason: NeedsAttentionReason, pub link: Option<Url>, pub created_at: OffsetDateTime, pub expires_at: OffsetDateTime }
impl NewNeedsAttention {
    /// Drops any link whose scheme is not https (S7 5.8). expires_at = created_at + NEEDS_ATTENTION_TTL (INV-2).
    pub fn new(reason: NeedsAttentionReason, link: Option<Url>, now: OffsetDateTime, t: &Tunables) -> Self;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedsAttentionExit { Resolved, Dismissed, Expired }   // every exit deletes the record
pub fn needs_attention_expired(expires_at: OffsetDateTime, now: OffsetDateTime) -> bool;

// invite.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(transparent)]
pub struct InviteId(pub Uuid);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(transparent)]
pub struct InviteRequestId(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus { Pending, Used, Revoked, Expired }

#[derive(Clone, Copy, PartialEq, Eq)]   // Debug by hand: status and times only
pub struct InviteState {
    pub status: InviteStatus,
    pub token_hash: [u8; 32],           // SHA-256 of the current invite token (hashing happens outside domain)
    pub email_hash: [u8; 32],           // keyed hash of the invited address (HMAC outside domain)
    pub last_sent_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,     // last_sent_at + INVITE_TTL
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteEvent { Resend { new_token_hash: [u8; 32], now: OffsetDateTime }, Revoke, Use { now: OffsetDateTime }, Expire { now: OffsetDateTime } }
impl InviteState {
    pub fn new_pending(token_hash: [u8; 32], email_hash: [u8; 32], now: OffsetDateTime, t: &Tunables) -> Self;
    pub fn apply(self, event: InviteEvent, t: &Tunables) -> Result<InviteState, DomainError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedeemRefusal { EmailUnverified, InviteInvalid, EmailMismatch }  // map to S7 outcomes email_unverified, invite_invalid, email_mismatch
pub fn check_redemption(
    invite: Option<&InviteState>,        // looked up by token hash; None when no invite has this token
    presented_token_hash: Option<&[u8; 32]>,
    verified_email_hash: &[u8; 32],
    email_verified: bool,
    now: OffsetDateTime,
) -> Result<(), RedeemRefusal>;

// mailbox_status.rs (MailboxStatus is T-101's)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxEvent { TokenInvalid, OAuthSucceeded, ConsentBlocked }
pub fn next_status(current: MailboxStatus, event: MailboxEvent) -> MailboxStatus;
```

## Algorithm

1. **Needs Attention.** `NewNeedsAttention::new`: keep `link` only if `link.scheme() == "https"`; `expires_at = now + t.needs_attention_ttl` (30 days). `needs_attention_expired` is `now > expires_at`. There is no stored status: resolve, dismiss and expiry all delete the record (S3), so the exits are an enum for logging and History only.
2. **Invite `new_pending`:** `status Pending`, `last_sent_at = now`, `expires_at = now + t.invite_ttl` (7 days).
3. **Invite `apply`:**
   - `Resend { new_token_hash, now }`: from `Pending` or `Expired` gives `Pending` with the new hash, `last_sent_at = now`, `expires_at = now + invite_ttl` (AU-01 AC2: the old token stops working because its hash is gone). From `Used` or `Revoked`: `Err(TransitionNotAllowed)`. `[DEFAULT]` re-sending an expired invite revives it; S3 lists re-send only from `pending`, but an admin re-sending a lapsed invite is the obvious intent and creates no duplicate (AU-01 AC2).
   - `Revoke`: from `Pending` or `Expired` gives `Revoked` (AU-01 AC4). Else error.
   - `Use { now }`: from `Pending` with `now <= expires_at` gives `Used`. Else error.
   - `Expire { now }`: from `Pending` with `now > expires_at` gives `Expired`. Else error.
4. **`check_redemption`**, in this order:
   1. `email_verified` false: `EmailUnverified` (AU-03 AC3).
   2. No presented token, no invite found, or invite status not `Pending`, or `now > expires_at`, or `invite.token_hash != *presented`: `InviteInvalid` (AU-03 AC6: missing, used, expired, revoked or replaced).
   3. `invite.email_hash != *verified_email_hash`: `EmailMismatch` (AU-03 AC2).
   4. Otherwise `Ok(())`. The caller then applies `Use` in the same storage transaction.
   `[DEFAULT]` the order checks the provider's verification first, then the token, then the email, so an unverified account learns nothing about the invite.
5. **Mailbox `next_status`:** `TokenInvalid` from `Connected` gives `NeedsSignIn` (UN-01 AC6, ST-03 AC1); `OAuthSucceeded` from `NeedsSignIn` or `ConsentBlocked` gives `Connected`; `ConsentBlocked` gives `ConsentBlocked` (v2 Microsoft only; nothing in v1 sends it). Every other pair returns `current` unchanged. "Removed" is not a status: disconnect deletes the record (S3).
6. All functions are pure.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-01 AC1 | A new invite is pending and expires `INVITE_TTL` after sending |
| AU-01 AC2 | Re-sending replaces the token hash, so the old token is refused |
| AU-01 AC4 | A revoked invite refuses redemption |
| AU-03 AC2 | A verified email that does not match the invite is refused |
| AU-03 AC3 | An unverified email is refused |
| AU-03 AC6 | A missing, used, expired, revoked or replaced token is refused even when the email matches |
| UN-01 AC6 | A token failure moves the mailbox to `needs_sign_in` and the job's item reason is "Sign in again" |
| UN-05 AC1 | A Needs Attention item carries its reason and an https link only |
| NA-01 AC2 | Items expire after `NEEDS_ATTENTION_TTL` |
| ST-03 AC1 | Mailbox status is one of connected or needs sign-in (consent blocked v2 only) |
| INV-2 | Every Needs Attention item has an `expires_at` |

## Tests that must pass

- `au_01_ac1_new_invite_expires_after_seven_days` (unit)
- `au_01_ac2_resend_voids_old_token` (unit: `check_redemption` with the old hash gives `InviteInvalid`, the new hash passes)
- `au_01_ac4_revoked_invite_refused` (unit)
- `au_03_ac2_email_mismatch_refused` (unit)
- `au_03_ac3_unverified_email_refused` (unit)
- `au_03_ac6_missing_token_refused_even_if_email_matches` (unit)
- `au_03_ac6_used_expired_revoked_or_replaced_refused` (property: any invite not `Pending`, or past expiry, or with a different hash, is refused)
- `un_01_ac6_token_invalid_sets_needs_sign_in` (unit)
- `un_01_ac6_sign_in_required_reason_serialises` (unit: `"sign_in_required"`)
- `un_05_ac1_item_keeps_https_link_only` (unit: `http`, `javascript` and `data` links dropped)
- `na_01_ac2_item_expires_after_30_days` (unit, both sides of the boundary)
- `st_03_ac1_mailbox_status_values` (unit: serialised `connected`, `needs_sign_in`, `consent_blocked`)
- `inv_2_needs_attention_item_has_expires_at` (unit)
- `invite_used_cannot_be_resent_or_revoked` (unit)
- `mailbox_status_oauth_success_reconnects` (unit)
- `invite_redemption_at_exact_expiry_allowed` (unit: `now == expires_at` passes, one second later refused)

## Edge cases and traps

- S7 5.8 lists no reason code for UN-01 AC6 "Sign in again"; this task adds `SignInRequired` (`sign_in_required`), the same name T-201b uses. T-701 must use it rather than adding `SignInRequired`.
- Reserved v2 reasons (`captcha`, `login_required`, `page_unclear`, `page_failed`) are not added in v1.
- Compare hashes with `==` on `[u8; 32]`; never handle raw invite tokens or email addresses in `domain` (hashing and HMAC need keys and live in the api).
- `InviteState` holds hashes only, but implement `Debug` by hand anyway (print status and times), so a hash never lands in a log.
- Never `match` `MailboxStatus` or `NeedsAttentionReason` with `_`; new variants must force a review.
- `Url::scheme()` is already lower case; compare with `"https"`.

## Out of scope

- Storage records and TTL sweeps: T-201b, T-706. Invite endpoints and emails: T-505. OAuth: T-502. Creating items when jobs end: T-701. Needs Attention endpoints: T-705.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
