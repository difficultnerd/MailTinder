# T-401: Gmail adapter: read messages and headers

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | sonnet | about 450 lines of code plus tests | T-203, T-205a, T-306 |

**Read only these spec sections:** S10 4.1, 4.2, 4.3 (`docs/specs/S10-test-strategy.md`); S3 "Message classes" and "Rule matching and counting" (`docs/specs/S3-domain-model.md`); S2 FD-01, FD-04, XC-01, XC-02 (`docs/specs/S2-v1-acceptance-criteria.md`); S6 section 4 "Scopes (Gmail)" (`docs/specs/S6-security.md`); S7 section 4 rows `provider_error`, `provider_unavailable` and section 6 last bullets (`docs/specs/S7-api-contract.md`); `docs/backlog/CONVENTIONS.md`. Nothing else is needed. This file is the S8 contract for the calls it adds.

## Goal

The `adapters-gmail` crate gets a real `GmailProvider` that implements the read half of `MailProvider` (`list_inbox`, `get_meta`, `get_preview`, `inbox_count`) over the Gmail REST API, plus the shared HTTP plumbing, error mapping and header-to-`HeaderFacts` builder every later M4 task reuses. The api Feed (T-602) and Progress (T-603) call it through the trait only.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/src/lib.rs` | `pub mod client; pub mod errors; pub mod headers; pub mod read; pub mod auth_results;` and `pub use read::GmailProvider;` |
| Create | `backend/crates/adapters-gmail/src/client.rs` | `GmailHttp`: base URLs, bearer auth, JSON decode, body cap, `fields=` partial responses |
| Create | `backend/crates/adapters-gmail/src/errors.rs` | Gmail error body to `MailError` mapping, `Retry-After` parsing |
| Create | `backend/crates/adapters-gmail/src/headers.rs` | `RawHeaders`, From parsing, RFC 2047 fallback, `build_header_facts` |
| Create | `backend/crates/adapters-gmail/src/auth_results.rs` | Fail-closed stub `assess_auth` (T-406 replaces the body) |
| Create | `backend/crates/adapters-gmail/src/read.rs` | `GmailProvider` struct and the four read methods |
| Create | `backend/crates/adapters-gmail/tests/contract_read.rs` | `testkit::contract::mail_provider` read cases against `fake-google` |
| Create | `backend/crates/adapters-gmail/tests/read_errors.rs` | Error mapping cases through `fake-google` failure injection |
| Change | `backend/crates/adapters-gmail/Cargo.toml` | Add `mailparse = "0.15"`, `rfc2047-decoder = "1"`, `url = "2"`, `base64 = "0.22"`, `futures = "0.3"` |

The remaining `MailProvider` methods (`set_labels`, `trash`, `report_spam`, `restore_labels`, `ensure_label`, `send_mailto`) return `Err(MailError::Invalid("not_implemented".into()))` in this task and are filled by T-403 and T-404. The contract suite cases for them stay disabled for `GmailProvider` until those tasks land.

## Types and signatures

```rust
// client.rs
pub const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
pub const GMAIL_TIMEOUT_S: u64 = 10;            // [DEFAULT] matches the one-click timeout in S6 6
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024; // [DEFAULT] refuse larger bodies as Transient

pub struct GmailHttp {
    egress: Arc<dyn HttpEgress>,   // every call goes through the egress allowlist (S10 7.2)
    base: Url,                     // GMAIL_BASE in production, fake-google base in tests
    clock: Arc<dyn Clock>,         // only for HTTP-date Retry-After values
}
impl GmailHttp {
    pub fn new(egress: Arc<dyn HttpEgress>, base: Url, clock: Arc<dyn Clock>) -> Self;
    pub async fn get_json<T: DeserializeOwned>(&self, mb: &MailboxCtx, path: &str, query: &[(&str, String)]) -> Result<T, MailError>;
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(&self, mb: &MailboxCtx, path: &str, query: &[(&str, String)], body: &B) -> Result<T, MailError>;
}

// errors.rs
pub const DEFAULT_RETRY_AFTER_S: u64 = 10;      // [DEFAULT] when Gmail sends no Retry-After
pub const MAX_RETRY_AFTER_S: u64 = 300;         // [DEFAULT] clamp so a bad header cannot park a mailbox for hours
pub fn map_gmail_error(status: u16, retry_after: Option<&str>, body: &[u8], now: OffsetDateTime) -> MailError;

// headers.rs
/// Header name and value in message order, exactly as Gmail returned them.
pub struct RawHeaders(pub Vec<(String, String)>);
impl RawHeaders {
    pub fn all<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str>; // case-insensitive name match, message order
    pub fn first(&self, name: &str) -> Option<&str>;
    pub fn count(&self, name: &str) -> usize;
}
pub const METADATA_HEADERS: [&str; 15] = [
    "From", "Subject", "Date", "List-Unsubscribe", "List-Unsubscribe-Post", "List-Id",
    "Feedback-ID", "Precedence", "Auto-Submitted", "Authentication-Results", "DKIM-Signature",
    "In-Reply-To", "References", "Reply-To", "Return-Path",
];
pub struct ParsedFrom { pub display: String, pub address: String }
pub fn parse_from(value: &str) -> Option<ParsedFrom>;
pub fn decode_header_text(value: &str) -> String;   // RFC 2047 fallback, then domain::text::sanitise_plain
pub fn build_header_facts(h: &RawHeaders, from: &ParsedFrom) -> HeaderFacts;

// auth_results.rs (stub; T-406 replaces the body, keeps the signature)
pub struct AuthAssessment { pub unsubscribe: Option<UnsubscribeOptions>, pub list_unsubscribe_present: bool, pub from_authenticated: bool }
pub fn assess_auth(h: &RawHeaders, from_domain: &str) -> AuthAssessment; // stub: { None, count("List-Unsubscribe") > 0, false }

// read.rs
pub const LIST_PAGE_SIZE: u32 = 20;             // [TUNABLE] S7 2 default page size
pub const GET_CONCURRENCY: usize = 10;          // [DEFAULT] keeps one page under Gmail's per-user quota rate
pub struct GmailProvider { http: GmailHttp }
impl GmailProvider { pub fn new(http: GmailHttp) -> Self; }
#[async_trait] impl MailProvider for GmailProvider { /* list_inbox, get_meta, get_preview, inbox_count real; others stubbed */ }

// Gmail wire types (private to the crate, serde, NOT deny_unknown_fields: Gmail adds fields)
#[derive(Deserialize)] struct ListResponse { messages: Option<Vec<IdOnly>>, #[serde(rename = "nextPageToken")] next_page_token: Option<String> }
#[derive(Deserialize)] struct IdOnly { id: String }
#[derive(Deserialize)] struct MetaMessage { id: String, #[serde(rename = "labelIds", default)] label_ids: Vec<String>,
    #[serde(rename = "internalDate")] internal_date: String, payload: MetaPayload }
#[derive(Deserialize)] struct MetaPayload { #[serde(default)] headers: Vec<Header> }
#[derive(Deserialize)] struct Header { name: String, value: String }
#[derive(Deserialize)] struct FullMessage { payload: Part }
#[derive(Deserialize)] struct Part { #[serde(rename = "mimeType", default)] mime_type: String, #[serde(default)] headers: Vec<Header>,
    body: Option<PartBody>, #[serde(default)] parts: Vec<Part> }
#[derive(Deserialize)] struct PartBody { data: Option<String>, #[serde(rename = "attachmentId")] attachment_id: Option<String> }
#[derive(Deserialize)] struct LabelResponse { #[serde(rename = "messagesTotal")] messages_total: Option<u64> }
```

`PageToken`, `MessagePage` and `ListOrder` come from T-201a: `PageToken(pub String)` (opaque provider token), `MessagePage { pub items: Vec<MessageMeta>, pub next: Option<PageToken> }` and `ListOrder::{NewestFirst, NewerThan(OffsetDateTime), OlderThan(OffsetDateTime)}`.

`EgressRequest` (with `HttpMethod`, `headers: Vec<(String, Sensitive<String>)>` and a `timeout: Duration`), `EgressResponse` and `EgressError` come from T-201a. Any `EgressError` maps to `MailError::Transient`, except `HostNotAllowed`, `NotPermitted` and `PermanentDeleteRefused`, which map to `Invalid("egress_refused")` (a configuration bug, never retried).

## Algorithm

### Per-call contract (S8 for Gmail, read calls)

All calls: scheme https, host `gmail.googleapis.com`, header `Authorization: Bearer <mb.access_token>`, `Accept: application/json`, timeout `GMAIL_TIMEOUT_S`, no redirects followed (egress), response over `MAX_RESPONSE_BYTES` is `Transient`. OAuth scope for every call in this task: `https://www.googleapis.com/auth/gmail.modify` (it includes read; S6 4 lists `gmail.modify`, `gmail.send`, `drive.appdata`, `openid`, `email` and nothing else; never request `gmail.readonly` or `https://mail.google.com/`).

| Trait method | Gmail call | Method and path | Query parameters | Fields read | Quota units |
| --- | --- | --- | --- | --- | --- |
| `list_inbox` | `users.messages.list` | `GET /messages` | `labelIds=INBOX`, `maxResults=20`, `includeSpamTrash=false`, `pageToken` (when given), `q` (see step 2), `fields=messages/id,nextPageToken` | `messages[].id`, `nextPageToken` | 5 |
| `list_inbox`, `get_meta` | `users.messages.get` | `GET /messages/{id}` | `format=metadata`, one `metadataHeaders=<name>` per entry of `METADATA_HEADERS`, `fields=id,labelIds,internalDate,payload/headers` | `id`, `labelIds`, `internalDate` (milliseconds since epoch as a string), `payload.headers[].name`, `.value` | 5 |
| `get_preview` | `users.messages.get` | `GET /messages/{id}` | `format=full`, `fields=payload(mimeType,headers,body/data,body/attachmentId,parts)` | the MIME part tree; `body.data` is base64url | 5 |
| `inbox_count` | `users.labels.get` | `GET /labels/INBOX` | `fields=messagesTotal` | `messagesTotal` | 1 |

Gmail's per-user limit is 250 quota units per second (moving average); one Feed page is about 5 + 20 x 5 = 105 units, so `GET_CONCURRENCY` of 10 stays well under it.

### Error mapping (shared by every M4 task)

`map_gmail_error(status, retry_after, body, now)`:

1. Parse `body` as `{"error":{"code":n,"status":"...","errors":[{"reason":"..."}]}}`; ignore parse failure. Collect `reasons`.
2. `401` gives `Unauthorized`.
3. `403` with any reason in `rateLimitExceeded`, `userRateLimitExceeded`, `dailyLimitExceeded`, `quotaExceeded` gives `RateLimited { retry_after_s }`. Any other `403` (for example `insufficientPermissions`, `forbidden`, `domainPolicy`) gives `Forbidden`.
4. `429` gives `RateLimited { retry_after_s }`.
5. `404` and `410` give `NotFound`.
6. `400` and `409` and `412` give `Invalid(code)` where `code` is a fixed string: `"gmail_bad_request"`, `"gmail_conflict"`, `"gmail_precondition"`. Never put Gmail's `message` text into `Invalid`.
7. `500`, `502`, `503`, `504` and any other status give `Transient`. A timeout or connection error from egress also gives `Transient`.
8. `retry_after_s`: if the `Retry-After` header is an integer, use it; if it is an HTTP date, use `max(0, date - now)` in seconds; else `DEFAULT_RETRY_AFTER_S`. Clamp to `1..=MAX_RETRY_AFTER_S`.

Rate-limit handling: the adapter never sleeps and never retries (no `tokio::time::sleep`; the virtual clock cannot drive it). It returns `RateLimited` and the api maps it to `503 provider_unavailable` with `Retry-After` (S7 6). Retries belong to the caller.

### Steps

1. `GmailHttp::get_json`: build the URL from `base` plus `path`, append query pairs with `url::form_urlencoded` (never string concatenation), send through `egress.call`, then on 2xx decode JSON with `serde_json::from_slice`, on non-2xx return `map_gmail_error`. A JSON decode failure is `Transient` (Gmail sent something we do not understand; log the route only).
2. `list_inbox(mb, page, order)`:
   1. `q` by order: `NewestFirst` none; `NewerThan(t)` `after:<t.unix_timestamp()>`; `OlderThan(t)` `before:<t.unix_timestamp()>`. Gmail always returns newest first.
   2. Call `messages.list`. No `messages` key means an empty page.
   3. Fetch each ID with `get_meta`, at most `GET_CONCURRENCY` at a time (`futures::stream::iter(...).buffered(GET_CONCURRENCY)`, which keeps list order).
   4. A `NotFound` for one ID is skipped silently (the message left between list and get, FD-04 AC1). Any other error fails the whole page with that error.
   5. Return items in list order and `next = nextPageToken.map(PageToken)`.
3. `get_meta(mb, id)`:
   1. Call `messages.get` with `format=metadata`.
   2. `internal_date`: parse the string as `i64` milliseconds, `OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)`; a parse failure is `Transient`.
   3. `RawHeaders` from `payload.headers` in order.
   4. `parse_from(first("From"))`; if absent or unparsable, `display = ""`, `address = ""` and `sender` from the empty string; the classifier treats it as suspect later. Do not fail the card.
   5. `subject = decode_header_text(first("Subject").unwrap_or(""))`, truncated to 998 characters (S7 Card).
   6. `facts = build_header_facts(&headers, &from)`.
   7. `labels = LabelSet` from `labelIds` exactly as given (exact IDs, including `CATEGORY_*`, `UNREAD`, `IMPORTANT`).
   8. `sender = domain::SenderKey` built with the T-101 constructor from `from.address` (it lower-cases and unwraps relays).
4. `parse_from(value)`: `mailparse::addrparse(value)`; take the first single address (`MailAddr::Single`); if the first entry is a group, take its first member. `display = decode_header_text(display_name or "")`, `address` = the addr-spec trimmed, at most 320 characters, else `None`.
5. `decode_header_text(v)`: Gmail normally returns decoded UTF-8. If `v` still contains `=?` and `?=`, run `rfc2047_decoder::decode`; on error keep `v`. Then `domain::text::sanitise_plain(v, usize::MAX)` (T-402) to remove control, bidi and zero-width characters. Until T-402 merges, strip `char::is_control` only and leave a `// T-402` note.
6. `build_header_facts(h, from)`:
   - `list_id`: first `List-Id`, take the text inside the last `<...>` if present else the trimmed value; lower-case; at most 255 characters.
   - `feedback_id`: first `Feedback-ID`, trimmed, at most 255 characters.
   - `precedence_bulk`: first `Precedence` lower-cased trimmed is `bulk`, `list` or `junk`.
   - `auto_submitted`: an `Auto-Submitted` header exists and its value lower-cased trimmed is not `no`.
   - `is_reply_or_thread`: `In-Reply-To` or `References` exists.
   - `esp_hint`: the first match of `ESP_HINTS` (below) as a case-insensitive substring of `List-Id`, `Feedback-ID` or `Return-Path`, else `None`.
   - `list_unsubscribe`, `list_unsubscribe_present` and `from_authenticated`: from `assess_auth(h, from_domain)`, where `from_domain` is the part after the last `@` of `from.address`, lower-cased.
7. `ESP_HINTS: [(&str, &str); 8]` `[DEFAULT]` (badge reason only, never a decision input): `("mcsv.net","mailchimp")`, `("list-manage.com","mailchimp")`, `("sendgrid.net","sendgrid")`, `("mailgun.org","mailgun")`, `("amazonses.com","amazon_ses")`, `("exacttarget.com","salesforce")`, `("hubspotemail.net","hubspot")`, `("constantcontact.com","constant_contact")`.
8. `get_preview(mb, id)`:
   1. Call `messages.get` with `format=full`.
   2. Walk the part tree depth first, skipping any part with an `attachmentId` or a `Content-Disposition: attachment` header. Pick the first `text/plain` part; if none, the first `text/html` part.
   3. Decode `body.data` with base64 URL-safe, no padding required (`base64::engine::general_purpose::URL_SAFE_NO_PAD` after stripping `=`). Only the first 256 KiB of decoded bytes are used `[DEFAULT]`.
   4. Charset from the part's `Content-Type` header `charset=` parameter: `utf-8` and `us-ascii` decode as UTF-8 lossy; anything else also lossy UTF-8 `[DEFAULT]` (Gmail transcodes most parts; a rare garbled preview is acceptable).
   5. `text/plain`: `domain::text::sanitise_plain(text, PREVIEW_MAX_CHARS)`. `text/html`: `domain::text::html_to_text(text, PREVIEW_MAX_CHARS)` (T-402). No part found: empty string. Never fetch an attachment or any URL.
9. `inbox_count(mb)`: `labels.get INBOX`, return `messagesTotal` or `0` when absent.
10. Logging: at most `tracing::debug!(route = "gmail.messages.get", status)`; never the message ID, query, header value, address or token (XC-01).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-04 AC1 | A message that disappears between list and get is skipped silently, not an error |
| XC-01 | The adapter logs no message ID, header value, address or token |
| XC-02 | All Gmail types stay private to `adapters-gmail`; callers see only `MailProvider` |
| INV-7 | `domain` gains no Gmail type or dependency from this task |
| V16.5.1 | Provider error text never reaches a `MailError` value |

## Tests that must pass

- `mail_provider_contract_list_and_meta_gmail` (contract, `adapters-gmail` against `fake-google`, read cases of `testkit::contract::mail_provider`)
- `fd_04_ac1_message_gone_between_list_and_get_is_skipped` (contract)
- `xc_01_gmail_read_logs_no_header_values` (service integration: run a page with canary headers, scan captured `tracing` output)
- `xc_02_gmail_types_not_public` (unit: a doc test or compile check that `adapters_gmail` exports only `GmailProvider`, `GmailHttp` and the T-405 store)
- `inv_7_domain_has_no_gmail_dependency` (unit: reads `backend/crates/domain/Cargo.toml` and fails on `adapters-gmail`, `reqwest`)
- `asvs_v16_5_1_gmail_error_text_not_in_mail_error` (unit on `map_gmail_error` with a body whose message holds a canary)
- `gmail_error_401_maps_to_unauthorized`, `gmail_error_403_rate_reason_maps_to_rate_limited`, `gmail_error_403_permission_maps_to_forbidden`, `gmail_error_429_retry_after_seconds_passed_through`, `gmail_error_429_http_date_retry_after_uses_clock`, `gmail_error_500_maps_to_transient`, `gmail_retry_after_clamped` (unit)
- `gmail_list_newer_than_sends_after_query` (contract: fake-google records the `q`)
- `gmail_meta_reads_headers_in_message_order` (contract: two `DKIM-Signature` and two `Authentication-Results` come back in order)
- `gmail_meta_label_ids_exact` (contract)
- `gmail_preview_prefers_text_plain_and_never_fetches_attachment` (contract: fake-google records no attachments call)
- `gmail_preview_html_part_is_stripped` (contract, enabled once T-402 lands)
- `gmail_inbox_count_reads_label_total` (contract)
- `gmail_facts_precedence_auto_submitted_reply` (unit table over `build_header_facts`)
- `gmail_from_parse_display_and_address` (unit table: quoted names, encoded words, group syntax, missing angle brackets)

## Edge cases and traps

- `internalDate` is a string of milliseconds, not seconds and not RFC 3339.
- Header names are case-insensitive (`List-ID`, `list-id`); compare with `eq_ignore_ascii_case`.
- Keep header order and duplicates. T-406 relies on the first `Authentication-Results` being Gmail's own and on counting `List-Unsubscribe` instances.
- Do not use `deny_unknown_fields` on Gmail responses; it is for our own API inputs only.
- Do not build query strings by hand; a message ID or page token with `&` would break the request.
- Do not retry or sleep inside the adapter. Return `RateLimited` and let the caller decide.
- Do not log `MailboxCtx` or any request URL: the URL contains the message ID.
- Never call `messages.delete`, `messages.batchDelete` or `threads.delete` (INV-5). This task has no reason to issue a `DELETE` at all.
- `labelIds=INBOX` already excludes trash and spam, but keep `includeSpamTrash=false` explicit.
- A From header with no address (`From: undisclosed-recipients:;`) must not panic or fail the page.
- Egress dependency: the adapter must call through `HttpEgress::call` so the per-service allowlist holds (S10 7.2). If T-306 has not merged, tests use the passthrough egress the testkit offers (T-202) or a local `ReqwestEgress` in the test file only; production wiring must use T-306. Recommend adding T-306 to this task's dependencies.

## Out of scope

- Label changes, trash, spam and restore: T-403. Send: T-404. Drive: T-405.
- HTML stripping and the text sanitiser: T-402 (this task calls them).
- DKIM coverage and `from_authenticated`: T-406 (this task ships the fail-closed stub).
- Sort rules, classification and the Feed endpoint: T-102, T-609, T-602.
- Live Gmail runs: T-1106.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `fake-google` serves every call in the per-call table with the documented parameters, and a request with an unknown `format` or a missing `Authorization` header gets the Gmail-shaped error the fake documents.
