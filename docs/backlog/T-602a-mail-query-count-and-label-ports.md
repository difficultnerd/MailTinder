# T-602a: MailProvider query, count and label additions

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-203, T-205a, T-401 |

Split from index row T-602 (Feed endpoint). The port in CONVENTIONS.md has `list_inbox` and `inbox_count` only, but the Feed (date-bounded pages), Progress (year counts), boss cards, the mail-stopped estimate and the Filed tab (label counts, label listing, rename, delete label) all need more. This task adds those methods once, to the trait, `FakeMailbox`, `fake-google` and the Gmail adapter. If T-201 or T-401 already added any of them under these names, skip that part.

**Read only these spec sections:** S2 GM-01 AC2, GM-04 AC1, GM-05 AC1, GM-08 AC2, FL-05 AC1, XC-02; S3 INV-5 and INV-7; S7 section 5.6 (API-CAT-1 to API-CAT-5) in `docs/specs/S7-api-contract.md`; S10 sections 4.1 to 4.3 and 6.4. Nothing else is needed.

## Goal

`MailProvider` can list and count messages by a typed query (inbox, label, date range, sender, List-Id), rename and remove a label definition, and build the provider web URL for a message. The shared contract suite proves `FakeMailbox` and the Gmail adapter agree.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/ports/src/mail.rs` | `MessageQuery` and five trait methods |
| Change | `backend/crates/testkit/src/fakes/mailbox.rs` | `FakeMailbox` implementations |
| Change | `backend/crates/testkit/src/contract/mail_provider.rs` | Contract cases for the new methods |
| Change | `backend/crates/adapters-gmail/src/messages.rs` | Gmail `messages.list` with `q` and `labelIds`, `resultSizeEstimate` |
| Change | `backend/crates/adapters-gmail/src/labels.rs` | `labels.get`, `labels.patch`, `labels.delete` |
| Change | `backend/crates/fake-google/src/gmail.rs` | `q` subset parser, `labels.get` counts, `labels.patch`, `labels.delete` |

## Types and signatures

```rust
// backend/crates/ports/src/mail.rs
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageQuery {
    pub in_inbox: bool,                    // only messages carrying the inbox location
    pub label: Option<String>,             // provider label ID
    pub after: Option<OffsetDateTime>,     // received strictly after (second precision)
    pub before: Option<OffsetDateTime>,    // received strictly before (second precision)
    pub from: Option<String>,              // sender address, already normalised (SenderKey)
    pub list_id: Option<String>,           // List-Id value without angle brackets
}

#[async_trait]
pub trait MailProvider: Send + Sync {
    // ... existing methods unchanged ...
    /// Newest first. `max` is 1 to 100.
    async fn list_messages(&self, mb: &MailboxCtx, q: &MessageQuery, page: Option<PageToken>, max: u32)
        -> Result<MessagePage, MailError>;
    /// May be an estimate for queries other than "label only" or "inbox only".
    async fn count_messages(&self, mb: &MailboxCtx, q: &MessageQuery) -> Result<u64, MailError>;
    async fn rename_label(&self, mb: &MailboxCtx, label_id: &str, new_name: &str) -> Result<(), MailError>;
    /// Removes the label definition only. Messages are never touched (INV-5).
    async fn remove_label(&self, mb: &MailboxCtx, label_id: &str) -> Result<(), MailError>;
    /// https URL that opens the message in the provider's web client.
    fn web_url(&self, mailbox_address: &str, id: &MessageId) -> String;
}
```

`MessagePage` is the type `list_inbox` already returns. If it carries IDs only, callers call `get_meta` per ID; do not change its shape here.

## Algorithm

Gmail adapter:

1. `list_messages`: `GET users/me/messages` with `labelIds=INBOX` when `in_inbox`, `labelIds=<label>` when `label` is set (both may be set), `maxResults=max`, `pageToken`, and `q` built from the remaining fields:
   - `after:<unix seconds>` and `before:<unix seconds>`;
   - `from:<address>`;
   - `list:<list_id>`.
   Join with single spaces.
2. Validate every value before building `q`: `from` and `list_id` must be 1 to 320 bytes, each byte in `A-Z a-z 0-9 @ . _ + -`; otherwise return `MailError::Invalid("query")` without calling Gmail. Never pass other text through.
3. `count_messages`: when the query is "label only" or "inbox only" (no other field), `GET users/me/labels/<id>` and return `messagesTotal` (exact). Otherwise `messages.list` with `maxResults=1` and return `resultSizeEstimate` `[DEFAULT]` (one provider call, good enough for meters and estimates; GM-04 AC1 asks for "one provider date-range query").
4. `rename_label`: `PATCH users/me/labels/<id>` with `{ "name": new_name }`. A 409 from Gmail maps to `MailError::Invalid("label_exists")`.
5. `remove_label`: `DELETE users/me/labels/<id>`. This is the label endpoint, not `messages.delete`.
6. `web_url`: `https://mail.google.com/mail/?authuser=<percent-encoded address>#all/<percent-encoded id>` `[DEFAULT]` (opens the right account when several are signed in).
7. Map HTTP errors as `list_inbox` does: 401 `Unauthorized`, 403 `Forbidden`, 404 `NotFound`, 429 `RateLimited { retry_after_s }`, 5xx `Transient`.

`FakeMailbox`: evaluate the query directly over its stored messages (inbox flag, label set, `internal_date` bounds, `sender`, `facts.list_id`); counts are exact. `remove_label` removes the label from the label table and from every message's label set, never removes a message.

`fake-google`: parse only `after:`, `before:`, `from:`, `list:` terms; any other term returns 400 so an adapter bug shows up. Record `labels.delete` calls; a call to `messages.delete` or `messages.batchDelete` still returns 500 and records `PERMANENT_DELETE_ATTEMPTED` (S10 4.1).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| XC-02 | The new provider calls go through the `MailProvider` trait; no Gmail type leaves `adapters-gmail` |
| INV-5 | `remove_label` never deletes a message, in the fake and in the Gmail adapter |
| INV-7 | `MessageQuery` and the new methods use only domain and port types |
| GM-04 AC1 | A date-range count is one provider query |

## Tests that must pass

- `xc_02_list_messages_contract_inbox_and_dates` (contract, run against `FakeMailbox` and Gmail adapter on `fake-google`)
- `xc_02_list_messages_contract_label_and_sender` (contract)
- `xc_02_count_messages_contract_label_exact` (contract)
- `xc_02_rename_label_contract` (contract)
- `xc_02_query_with_quote_refused` (unit, `adapters-gmail`: `from` containing `"` gives `Invalid` and no HTTP call)
- `inv_5_remove_label_keeps_messages` (contract: every message still exists after `remove_label`)
- `inv_5_no_permanent_delete_during_label_removal` (contract against `fake-google`: no `PERMANENT_DELETE_ATTEMPTED`)
- `inv_7_message_query_has_no_provider_types` (unit, `ports`: compile-level check that `MessageQuery` fields are std, `time` or domain types)
- `gm_04_ac1_year_count_is_one_call` (contract against `fake-google`: one recorded request)

## Edge cases and traps

- Do not name the label method `delete_label`: the trait must have no method containing `delete` that a reviewer could mistake for a message delete; `remove_label` is the name.
- Gmail `after:` and `before:` take Unix seconds; passing a date string gives day precision and breaks Feed paging.
- `resultSizeEstimate` is an estimate; never use it where exactness matters (it is fine for meters, levels, bosses and the mail-stopped estimate).
- Build URLs with `reqwest::Url` and its query pair encoder, not string concatenation.
- No message ID, address or query string in logs or error text (XC-01).
- Keep `match` on `Provider` exhaustive with no `_` arm.

## Out of scope

- Using these methods: T-602c (Feed), T-603 (Progress), T-605b (mail-stopped estimate), T-607a (categories).
- Microsoft Graph equivalents: v2.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The contract suite runs the new cases against both implementations.
