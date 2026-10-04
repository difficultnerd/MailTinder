# T-703: Mailto unsubscribe sender

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | strong | about 200 lines of code plus about 300 lines of tests | T-404, T-701 |

**Read only these spec sections:** S2 UN-01 AC4 to AC6, UN-03 AC1, AC2 (`docs/specs/S2-v1-acceptance-criteria.md`), S6 6 bullet "Mailto" and threat T9 (`docs/specs/S6-security.md`), S7 6 row "Mailto unsubscribes sent" and the paragraph under the table about the mailto limit (`docs/specs/S7-api-contract.md`), S10 6.1 bullet "No real mailto send" and 6.3 row "Mailto" (`docs/specs/S10-test-strategy.md`), ASVS register rows V1.3.3, V1.3.11, V2.3.2, V2.4.1 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

Add the mailto sender to `unsub`: given a claimed job with method mailto, it re-validates the target, takes one unit of the per-mailbox daily send quota, mints an access token for the job's mailbox, and sends the unsubscribe email from that mailbox through `MailProvider::send_mailto`. The sent message stays in Sent with the "Mail Tinder" label. Failures become a `SendResult` for the runner.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/unsub/src/mailto.rs` | `MailtoSender`, `map_mail_error` |
| Create | `backend/crates/unsub/src/quota.rs` | `take_mailto_quota` |
| Change | `backend/crates/unsub/src/main.rs` | Register `MailtoSender` |
| Change | `backend/crates/unsub/src/lib.rs` | `pub mod mailto; pub mod quota;` |
| Create | `backend/crates/unsub/tests/mailto.rs` | Service integration tests with `FakeMailbox` |

## Types and signatures

```rust
// unsub/src/mailto.rs
pub struct MailtoSender;

#[async_trait]
impl UnsubSender for MailtoSender {
    fn method(&self) -> JobMethod { JobMethod::Mailto }
    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult;
}

/// Pure mapping, unit tested.
pub fn map_mail_error(err: &MailError) -> SendResult;

// unsub/src/quota.rs
pub const MAILTO_DAILY_LIMIT: u32 = 100;   // S7 6, per mailbox per day [TUNABLE]
pub enum QuotaDecision { Allowed, Exceeded }
/// Firestore-backed counter (works across instances):
/// ports.store.rate_limits().hit(&RateLimitKey(format!("mailto:{mailbox_id}")), start of the UTC day, Duration::days(1)).
pub async fn take_mailto_quota(ports: &Ports, mailbox: &MailboxId, now: OffsetDateTime) -> Result<QuotaDecision, SvcError>;
```

`MailtoTarget` (validated type with `parse`) and the labelled send (`send_mailto` labels the sent message "Mail Tinder" and returns `Ok` even if only the label failed) come from T-404. `RateLimitRepo::hit` and `RateLimitKey` come from T-201b. `JobOutcomeCode` values (`MailtoSent`, `HttpRejected`, `TimedOut`, `TokenInvalid`, `Refused`) are from T-201b plus T-701's additions.

## Algorithm

1. `MailtoTarget::parse(job.target)` (T-404). It must refuse CR or LF anywhere, any header parameter other than `subject` and `body`, more than one address, and non-`mailto` schemes. A refusal returns `NeedsAttention { reason: UnsubscribeFailed, code: Refused }` and nothing is sent.
2. `take_mailto_quota`: if the counter after increment exceeds `MAILTO_DAILY_LIMIT`, return `NeedsAttention { reason: UnsubscribeFailed, code: Refused }` and write a security event `rate_limit_hit` (scope `mailto`, pseudonymous user ID). The quota unit is spent even if the send later fails `[DEFAULT: conservative; protects the mailbox's sending reputation]`.
3. `mint_access_token(ports, &job.user_id, &job.mailbox_id)` (T-701):
   - `Revoked` gives `TokenRevoked`;
   - `Transient` gives `Retryable { code: HttpRejected }`;
   - `MailboxMissing` gives `NeedsAttention { reason: UnsubscribeFailed, code: MailboxRemoved }`;
   - `Crypto` gives `Retryable { code: HttpRejected }` and an error log line (no values).
4. `ports.mail.send_mailto(&ctx, &target)`. T-404 sends from the same mailbox with `To`, `Subject` and body from the URI and applies the "Mail Tinder" label to the sent message (UN-03 AC2). Success gives `Sent { code: MailtoSent }`.
5. Errors go through `map_mail_error`:

| `MailError` | `SendResult` |
| --- | --- |
| `Unauthorized` | `TokenRevoked` (the token was minted seconds ago, so the grant is gone) `[DEFAULT]` |
| `Forbidden` | `TokenRevoked` (send scope withdrawn) `[DEFAULT]` |
| `RateLimited { .. }` | `Retryable { code: HttpRejected }` |
| `Transient` | `Retryable { code: HttpRejected }` |
| `NotFound` | `NeedsAttention { reason: UnsubscribeFailed, code: Refused }` |
| `Invalid(_)` | `NeedsAttention { reason: UnsubscribeFailed, code: Refused }` (do not log the inner string) |

6. On `TokenRevoked` from a provider error, also set the mailbox to `needs_sign_in` (conditional write), the same way `mint_access_token` does.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-01 AC4 | The mailto send mints a fresh access token; none is stored |
| UN-01 AC5 | Mailto sends use the access token minted for the job |
| UN-01 AC6 | A revoked grant ends the mailto job `failed` with "Sign in again" |
| UN-03 AC1 | The email goes from the same mailbox with the address, subject and body in the URI |
| UN-03 AC2 | The sent email stays in Sent with the "Mail Tinder" label and is never deleted |
| INV-5 | No permanent delete on this path |
| V1.3.3 | Lengths and characters checked before the send call |
| V1.3.11 | CR and LF refused; no `cc` or `bcc` |
| V2.3.2 | Per-mailbox daily mailto cap enforced |
| V2.4.1 | The cap is a shared counter, not instance memory |

## Tests that must pass

- `un_01_ac4_mailto_token_minted_not_stored` (service integration)
- `un_01_ac5_mailto_uses_minted_token` (service integration: `FakeMailbox` records the access token it was given and it equals the identity fake's minted value)
- `un_01_ac6_mailto_revoked_token_sign_in_item` (service integration)
- `un_03_ac1_sends_to_address_subject_body_from_uri` (service integration: target `mailto:unsub@example.com?subject=Remove%20me&body=Please%20remove`)
- `un_03_ac1_sends_from_same_mailbox` (service integration: two mailboxes, only the job's mailbox sends)
- `un_03_ac2_sent_message_labelled_mail_tinder` (service integration)
- `inv_5_mailto_path_never_deletes` (service integration: `FakeMailbox` has no delete; the HTTP fake records no delete when run against `fake-google`)
- `asvs_v1_3_3_oversized_mailto_refused` (unit: target over 2,048 characters `[DEFAULT]` refused)
- `asvs_v1_3_11_crlf_in_mailto_refused` (unit: CR or LF in address, subject or body, raw and percent-encoded `%0D` `%0A`)
- `asvs_v1_3_11_cc_bcc_refused` (unit)
- `asvs_v2_3_2_mailto_daily_cap_per_mailbox` (service integration: the 101st job of a UTC day ends `needs_attention` with code `Refused` and a `rate_limit_hit` security event; another mailbox is unaffected; next day allowed)
- `asvs_v2_4_1_mailto_quota_shared_across_instances` (service integration: two `UnsubState` values over one fake store share the count)
- `map_mail_error_table` (unit)

## Edge cases and traps

- Percent-decoding happens before the CR and LF check; `%0D%0A` must be refused, not passed through.
- Never send to any address other than the one in the URI, and never add `Cc`, `Bcc` or `Reply-To`.
- Never call `send_mailto` without a fresh token from `mint_access_token`; never read a token from the job.
- The day boundary for the quota is UTC (`now.date()` in UTC), from `Clock`.
- Do not log the address, subject, body or the provider's error text.
- If T-404's `send_mailto` does not apply the label, stop and raise it: the label needs the sent message ID, which only the adapter has. Do not add a second labelling path here.

## Out of scope

- Parsing and validating `MailtoTarget`, and the Gmail send and label calls: T-404.
- Creating mailto jobs only under DKIM cover (UN-03 AC3): T-406 and T-605.
- Runner retries and Needs Attention items: T-701.

## Security review checklist

- The target is re-parsed with the strict T-404 parser at run time, not trusted from the job record.
- The quota is checked before minting and sending, and the counter cannot be bypassed by concurrent jobs (single atomic increment).
- The fresh access token never leaves memory and is wrapped in `Sensitive`.
- `needs_sign_in` is set only for `Unauthorized`, `Forbidden` and `Revoked`.
- No log line carries the address, subject, body or provider text.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
