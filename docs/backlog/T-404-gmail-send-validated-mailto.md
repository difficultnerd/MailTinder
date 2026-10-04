# T-404: Gmail adapter: send with validated mailto

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | strong | about 400 lines of code plus tests | T-401, T-403 |

**Read only these spec sections:** S6 section 3 row T9 and section 6 "Mailto" bullet (`docs/specs/S6-security.md`); S2 UN-03 AC1 to AC3, AU-01 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 section 3.4 "Invite token" bullet and API-ADM-2 (`docs/specs/S7-api-contract.md`); S10 section 6.1 and the "Mailto" row of 6.3 (`docs/specs/S10-test-strategy.md`); ASVS register rows V1.2.2, V1.3.3, V1.3.11 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-401-gmail-read-messages-and-headers.md` "Per-call contract" and "Error mapping". Nothing else is needed. This file is the S8 contract for the calls it adds.

## Goal

Two things exist after this task. First, strict parsing of a `mailto:` unsubscribe target into a `MailtoTarget` that can only name one recipient plus a subject and body, and a validated `EmailAddress` type. Second, `GmailProvider::send_mailto`, which sends that exact message from the user's own mailbox and labels it "Mail Tinder" in Sent, plus a narrow `InviteMailer` port whose only message is the fixed invite email (used by T-505 from the admin's mailbox). The mailto unsubscribe sender (T-703) and DKIM coverage (T-406) build on the parser.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/address.rs` | `EmailAddress`, `AddressError` |
| Create | `backend/crates/domain/src/mailto.rs` | `MailtoTarget::parse`, `MailtoError` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod address; pub mod mailto;` and re-export both types |
| Create | `backend/crates/ports/src/invite_mailer.rs` | `InviteMailer` trait and `InviteLink` |
| Change | `backend/crates/ports/src/lib.rs` | `pub mod invite_mailer;` |
| Create | `backend/crates/adapters-gmail/src/send.rs` | RFC 5322 builder, `send_mailto`, `InviteMailer` impl |
| Change | `backend/crates/adapters-gmail/src/read.rs` | Delegate `send_mailto` to `send.rs` |
| Create | `backend/crates/testkit/src/fakes/invite_mailer.rs` | `FakeInviteMailer` that records `(to, link)` pairs |
| Create | `backend/crates/domain/tests/mailto_parse.rs` | Parser table and property tests |
| Create | `backend/crates/adapters-gmail/tests/contract_send.rs` | Send cases against `fake-google` |
| Change | `backend/crates/domain/Cargo.toml` | Add `percent-encoding = "2"` |

## Types and signatures

```rust
// domain/src/address.rs
pub const ADDRESS_MAX_CHARS: usize = 320;
/// A validated addr-spec: ASCII dot-atom local part, LDH domain. Domain stored
/// lower case, local part kept as given (S7 API-ADM-2).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EmailAddress(String);
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddressError { #[error("address invalid")] Invalid }
impl EmailAddress {
    pub fn parse(input: &str) -> Result<Self, AddressError>;
    pub fn as_str(&self) -> &str;
    pub fn domain(&self) -> &str;
    /// Whole address lower-cased: the input to the email lookup HMAC (T-502b, T-505).
    pub fn lookup_form(&self) -> String;
}
// Debug prints "[address]" so a stray {:?} never logs it.
impl std::fmt::Debug for EmailAddress { /* writes "[address]" */ }

// domain/src/mailto.rs
pub const MAILTO_MAX_CHARS: usize = 2048;
pub const MAILTO_SUBJECT_MAX_CHARS: usize = 255;   // [DEFAULT]
pub const MAILTO_BODY_MAX_CHARS: usize = 2000;     // [DEFAULT]
pub const MAILTO_DEFAULT_TEXT: &str = "unsubscribe"; // [DEFAULT] RFC 2369 practice when subject or body is absent
// MailtoTarget itself (private fields, `new`, accessors `to()`, `subject()`, `body()`, Debug redacted)
// is defined by T-101 in domain/src/message.rs. This task adds only the strict parser below.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MailtoError {
    #[error("not a mailto uri")] NotMailto,
    #[error("too long")] TooLong,
    #[error("bad address")] Address,
    #[error("more than one recipient")] MultipleRecipients,
    #[error("header field not allowed")] FieldNotAllowed,   // cc, bcc, to, in-reply-to, anything but subject and body
    #[error("duplicate field")] DuplicateField,
    #[error("bad percent encoding")] Encoding,
    #[error("control character")] ControlCharacter,         // includes CR and LF (ASVS V1.3.11)
}
impl MailtoTarget { pub fn parse(uri: &str) -> Result<MailtoTarget, MailtoError>; } // second impl block, same crate

// ports/src/invite_mailer.rs
/// `https://<app origin>/#/invite?t=<token>`; built only by the api (T-505).
pub struct InviteLink(pub Sensitive<Url>);
#[async_trait]
pub trait InviteMailer: Send + Sync {
    /// Sends the fixed invite email from `mb` to `to`. No other content is possible.
    async fn send_invite(&self, mb: &MailboxCtx, to: &EmailAddress, link: &InviteLink) -> Result<(), MailError>;
}

// adapters-gmail/src/send.rs
pub const INVITE_SUBJECT: &str = "You're invited to Mail Tinder";
pub const INVITE_BODY_TEMPLATE: &str =
    "You've been invited to try Mail Tinder.\r\n\r\nOpen this link and continue with this Google account:\r\n{link}\r\n\r\nThe link works once and expires in 7 days.\r\n";
fn build_rfc5322(from: &EmailAddress, to: &EmailAddress, subject: &str, body: &str) -> Vec<u8>;
#[async_trait] impl InviteMailer for GmailProvider { /* ... */ }
```

`CONVENTIONS.md` has no `EmailAddress`, `MailtoError` or `InviteMailer`; this task defines them as above. `MailtoTarget` and its constructor `MailtoTarget::new` come from T-101; `parse` ends by calling `new`, so T-101's checks stay the last line of defence.

## Algorithm

### Per-call contract (S8 for Gmail, send calls)

Base, headers, timeout and error mapping exactly as T-401.

| Use | Gmail call | Method and path | Query or body | Fields read | Scope | Quota units |
| --- | --- | --- | --- | --- | --- | --- |
| Own address for `From` | `users.getProfile` | `GET /profile` | `fields=emailAddress` | `emailAddress` | `gmail.modify` | 1 |
| Send | `users.messages.send` | `POST /messages/send` | body `{"raw": "<base64url of RFC 5322 bytes>"}`; `fields=id` | `id` | `https://www.googleapis.com/auth/gmail.send` | 100 |
| Label in Sent | `ensure_label` and `messages.modify` (T-403) | as T-403 | `add = [<Mail Tinder label ID>]` | `labelIds` | `gmail.modify` | 5 + 5 |

Rate limits: Gmail limits sending per user per day (about 500 for consumer accounts) and per second. `403` with `dailyLimitExceeded` or `userRateLimitExceeded`, and `429`, map to `RateLimited` per T-401. The adapter never retries a send. The 100 per mailbox per day cap is T-703's Firestore counter, not this adapter's.

### `EmailAddress::parse`

1. Trim ASCII whitespace. Reject empty or more than `ADDRESS_MAX_CHARS` characters, or any non-ASCII character `[DEFAULT]` (internationalised local parts are rare in unsubscribe headers; IDN domains arrive as `xn--` punycode).
2. Split at the last `@`; exactly one `@` allowed.
3. Local part: 1 to 64 characters from `A-Z a-z 0-9 ! # $ % & ' * + - / = ? ^ _ { | } ~ .` and backtick; no leading, trailing or doubled `.`. Quoted local parts are rejected `[DEFAULT]`.
4. Domain: 1 to 253 characters, at least two labels split by `.`; each label 1 to 63 of `A-Z a-z 0-9 -`, not starting or ending with `-`; the last label is not all digits. IP literals (`[...]`) rejected.
5. Store `local + "@" + domain.to_ascii_lowercase()`.

### `MailtoTarget::parse`

1. Reject more than `MAILTO_MAX_CHARS`. Strip surrounding `<` `>` if present (the List-Unsubscribe syntax), then require a case-insensitive `mailto:` prefix, else `NotMailto`.
2. Split the rest at the first `?` into `path` and `query`.
3. Percent-decode `path` strictly (a `%` not followed by two hex digits is `Encoding`; result must be valid UTF-8). If it contains `,` it is `MultipleRecipients`. Empty path is `Address` (a `to=` field cannot supply the recipient). Parse with `EmailAddress::parse`, else `Address`.
4. Split `query` on `&`; skip empty pieces. Each piece is `name=value` (no `=` is `FieldNotAllowed`). Percent-decode both strictly; `+` stays a literal plus (RFC 6068, not form encoding).
5. `name` lower-cased must be `subject` or `body`, else `FieldNotAllowed`. A repeated name is `DuplicateField`.
6. Any `char::is_control()` in subject or body (this includes CR, LF and NUL) is `ControlCharacter`, tab included `[DEFAULT]`. Subject over `MAILTO_SUBJECT_MAX_CHARS` or body over `MAILTO_BODY_MAX_CHARS` characters is `TooLong`.
7. Return `MailtoTarget::new(to.as_str(), subject.as_deref(), body.as_deref())`; an error from `new` maps to `Address` (it should never happen after the checks above).

### `send_mailto(mb, target)`

1. `from = EmailAddress::parse(getProfile.emailAddress)`; failure is `Transient`.
2. `to = EmailAddress::parse(target.to())` (failure is `Invalid("mailto_target")`); `subject = target.subject() or MAILTO_DEFAULT_TEXT`; `body = target.body() or MAILTO_DEFAULT_TEXT`.
3. `raw = build_rfc5322(&from, &to, subject, body)`:
   - Lines, each ending `\r\n`: `From: <from>`, `To: <to>`, `Subject: <s>`, `MIME-Version: 1.0`, `Content-Type: text/plain; charset=utf-8`, `Content-Transfer-Encoding: base64`, then an empty line, then the body as standard base64 wrapped at 76 characters.
   - Subject: if all printable ASCII, as is; otherwise RFC 2047 `=?UTF-8?B?<base64>?=` in encoded words of at most 75 characters, joined by `\r\n ` folding.
   - Addresses are written bare (no display names), so no quoting is needed. Never write a header from any other input.
4. `POST /messages/send` with `{"raw": URL_SAFE_NO_PAD(raw)}`. Return its error if it fails.
5. After a successful send: `label = ensure_label(mb, MAIL_TINDER_LABEL)` then `messages.modify(id, add=[label])`. If either fails, log `outcome="sent_label_failed"` (route and status only) and still return `Ok(())`. The message is sent; reporting an error would make the caller send again.

### `send_invite(mb, to, link)`

1. Require `link` scheme `https` and a fragment starting `/invite?t=`; else `Invalid("invite_link")`.
2. `from` from `getProfile` as above; `body = INVITE_BODY_TEMPLATE` with `{link}` replaced by the link string; subject `INVITE_SUBJECT`.
3. Build and send as steps 3 and 4 above. No label step.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-03 AC1 | The unsubscribe email goes from the same mailbox to the address, subject and body in the URI |
| UN-03 AC2 | The sent email is left in Sent with the "Mail Tinder" label and is never deleted |
| AU-01 AC1 | The invite email with the sign-in link is sent from the admin's mailbox (send half; T-505 owns the record) |
| V1.2.2 | Only `mailto` targets built by the strict parser can be sent |
| V1.3.3 | Length and character limits are applied before the send call |
| V1.3.11 | Strict mailto parsing: CR and LF refused, no `cc` or `bcc`, one recipient |

## Tests that must pass

- `un_03_ac1_send_uses_uri_address_subject_body` (contract against `fake-google`: decode the `raw` the fake received and assert `To`, `Subject`, body and `From` = profile address)
- `un_03_ac1_default_subject_and_body_when_absent` (contract)
- `un_03_ac2_sent_message_gets_mail_tinder_label` (contract)
- `un_03_ac2_label_failure_still_returns_ok_and_sends_once` (contract: fake-google fails `labels.create`; exactly one send recorded)
- `au_01_ac1_invite_email_sent_with_link` (contract: body holds the link, subject is fixed)
- `asvs_v1_2_2_invite_link_must_be_https_invite_fragment` (unit)
- `asvs_v1_3_3_mailto_length_limits` (unit table)
- `asvs_v1_3_11_mailto_rejects_cr_lf_in_any_field` (unit table: `%0D`, `%0A`, `%00` in path, subject, body)
- `asvs_v1_3_11_mailto_rejects_cc_bcc_to_and_unknown_fields` (unit table)
- `asvs_v1_3_11_mailto_rejects_multiple_recipients` (unit: `a@example.com,b@example.com`)
- `asvs_v1_3_11_mailto_parse_never_panics` (property: random strings)
- `mailto_plus_is_literal_not_space` (unit)
- `mailto_angle_brackets_stripped` (unit)
- `email_address_domain_lowercased_local_kept` (unit)
- `email_address_rejects_ip_literal_and_quoted_local` (unit table)
- `email_address_debug_is_redacted` (unit)
- `gmail_send_rate_limited_maps_and_does_not_retry` (contract: one send attempt recorded)
- `gmail_subject_non_ascii_encoded_word` (unit on `build_rfc5322`)

## Edge cases and traps

- `+` in a mailto is a plus sign, not a space. Do not use `form_urlencoded` to decode.
- Percent-decode before checking for CR and LF; `%0D%0A` is the attack.
- A `to=` field must be refused, not merged: it adds recipients.
- Never put the target address into a display name or any header other than `To`.
- Do not retry the send on `Transient`; a timeout after Gmail accepted the message would send twice. Return the error; T-703 decides (one duplicate unsubscribe email is the worst case there).
- Do not return an error after a successful send because labelling failed.
- Never log the target address, subject, body, invite link or the raw message; `Debug` on `EmailAddress` is redacted on purpose, and `InviteLink` wraps `Sensitive`.
- The invite email stays in the admin's Sent folder with a live token until it is used or expires; that is accepted (the admin is the sender) and is why re-send voids the old token (T-505).
- Use the URL-safe base64 alphabet for `raw` and the standard alphabet inside the MIME body.

## Out of scope

- Choosing a mailto target from headers and the DKIM rule: T-406.
- Running the mailto job, the per-mailbox daily cap and minting the token: T-703, T-503.
- Invite records, tokens and the admin endpoints: T-505.

## Security review checklist

- `MailtoTarget` can be built only through `parse` or T-101's checked `new` (no `Default`, no `From<String>`, no public fields); every caller that reads a target from a header uses `parse`.
- The parser refuses every header field except `subject` and `body`, and refuses duplicates.
- CR, LF, NUL and other controls are refused after percent-decoding in every component.
- `build_rfc5322` writes only `From`, `To`, `Subject`, `MIME-Version`, `Content-Type`, `Content-Transfer-Encoding`, and the subject cannot inject a header (encoded word or validated ASCII without controls).
- `InviteMailer` cannot send arbitrary text: the subject and body are constants and the link is checked.
- No log line, error value or panic message contains an address, subject, body or link.
- The send path issues exactly one `messages.send` per call, with no retry loop.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `fake-google` decodes `raw` on `messages.send` and stores it in Sent with label `SENT`, so the contract tests can read it back.
