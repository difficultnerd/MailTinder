# T-403: Gmail adapter: trash, labels, spam and exact restore

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | sonnet | about 300 lines of code plus tests | T-401 |

**Read only these spec sections:** S3 "Swipe and undo" state machine, "Message classes" table and INV-5, INV-6 (`docs/specs/S3-domain-model.md`); S2 SW-01 to SW-05, FL-02 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 4.1 and 6.4 (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-401-gmail-read-messages-and-headers.md` sections "Per-call contract" and "Error mapping"; `docs/backlog/CONVENTIONS.md` `MailProvider`. Nothing else is needed. This file is the S8 contract for the calls it adds.

## Goal

`GmailProvider` implements the mailbox-changing half of `MailProvider`: `set_labels`, `trash`, `report_spam`, `restore_labels` and `ensure_label`. Every change returns or accepts an exact `LabelSet`, so an undo puts back the exact previous labels (S3), and no method can permanently delete a message (INV-5). Swipes (T-604, T-605) and undo (T-606) use these through the trait.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/src/modify.rs` | The five methods and their wire types |
| Change | `backend/crates/adapters-gmail/src/read.rs` | Delegate the five trait methods to `modify.rs` instead of the T-401 stubs |
| Create | `backend/crates/adapters-gmail/tests/contract_modify.rs` | Enable the modify cases of `testkit::contract::mail_provider` for `GmailProvider` |
| Create | `backend/crates/adapters-gmail/tests/no_delete.rs` | Structural INV-5 test over the crate source |

## Types and signatures

```rust
// modify.rs
pub const MAIL_TINDER_LABEL: &str = "Mail Tinder";   // S2 UN-03 AC2 (used by T-404)
pub const LABEL_NAME_MAX_CHARS: usize = 225;          // Gmail limit
/// System labels Gmail does not let messages.modify add or remove.
pub const IMMUTABLE_LABELS: [&str; 3] = ["SENT", "DRAFT", "CHAT"];

#[derive(Serialize)] struct ModifyRequest<'a> {
    #[serde(rename = "addLabelIds")] add: Vec<&'a str>,
    #[serde(rename = "removeLabelIds")] remove: Vec<&'a str>,
}
#[derive(Deserialize)] struct LabelsOnly { #[serde(rename = "labelIds", default)] label_ids: Vec<String> }
#[derive(Deserialize)] struct LabelList { #[serde(default)] labels: Vec<LabelItem> }
#[derive(Deserialize)] struct LabelItem { id: String, name: String, #[serde(rename = "type")] kind: Option<String> }
#[derive(Serialize)] struct CreateLabel<'a> {
    name: &'a str,
    #[serde(rename = "labelListVisibility")] list_vis: &'static str,      // "labelShow"
    #[serde(rename = "messageListVisibility")] msg_vis: &'static str,     // "show"
}

impl GmailProvider {
    async fn current_labels(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;
}
```

## Algorithm

### Per-call contract (S8 for Gmail, modify calls)

Scope for every call: `https://www.googleapis.com/auth/gmail.modify`. Base, headers, timeout and error mapping exactly as T-401. All bodies are `application/json`.

| Use | Gmail call | Method and path | Query or body | Fields read | Quota units |
| --- | --- | --- | --- | --- | --- |
| Labels before a change | `users.messages.get` | `GET /messages/{id}` | `format=minimal`, `fields=labelIds` | `labelIds` | 5 |
| Add or remove labels | `users.messages.modify` | `POST /messages/{id}/modify` | body `ModifyRequest`; query `fields=labelIds` | `labelIds` after the change | 5 |
| Trash | `users.messages.trash` | `POST /messages/{id}/trash` | no body; `fields=labelIds` | `labelIds` after | 5 |
| Leave trash | `users.messages.untrash` | `POST /messages/{id}/untrash` | no body; `fields=labelIds` | `labelIds` after | 5 |
| Find a label | `users.labels.list` | `GET /labels` | `fields=labels(id,name,type)` | `id`, `name`, `type` | 1 |
| Create a label | `users.labels.create` | `POST /labels` | body `CreateLabel`; `fields=id,name` | `id` | 5 |

Error mapping adds: `404` on a message is `NotFound` (the api turns it into `409 message_changed`, FD-04); `409` on `labels.create` means the label already exists (a race), handled in `ensure_label`.

Forbidden calls (INV-5): `users.messages.delete`, `users.messages.batchDelete`, `users.threads.delete`, `users.messages.batchModify` with no limit on IDs (not needed). No `DELETE` method is ever issued by Gmail code.

### Steps

1. `current_labels(mb, id)`: `messages.get` with `format=minimal`; return `LabelSet` of `labelIds`.
2. `trash(mb, id)`:
   1. `before = current_labels`.
   2. `POST /messages/{id}/trash`.
   3. Return `before`. The caller stores it for undo.
3. `report_spam(mb, id)`:
   1. `before = current_labels`.
   2. `POST /messages/{id}/modify` with `add = ["SPAM"]`, `remove = ["INBOX"]`. Gmail treats adding `SPAM` as a spam report for filter training.
   3. Return `before`. Whether the swipe also trashes is the swipe task's decision (T-605), not this adapter's.
4. `set_labels(mb, id, add, remove)`:
   1. Refuse `TRASH`, `SPAM` or anything in `IMMUTABLE_LABELS` in either set with `Invalid("label_not_allowed")`; trash and spam have their own methods.
   2. Drop entries that appear in both sets from `add` (remove wins) `[DEFAULT]`.
   3. If both are empty, return `current_labels` without a modify call.
   4. `POST modify`; return the `labelIds` from the response as the new `LabelSet`.
5. `restore_labels(mb, id, exact)`:
   1. `now = current_labels`.
   2. If `now` has `TRASH` and `exact` does not, `POST untrash` and set `now` to its returned labels.
   3. If `now` lacks `TRASH` and `exact` has it, `POST trash` and set `now` from a fresh `current_labels` (the undo of an undo).
   4. Compute `add = exact - now` and `remove = now - exact`, both without `TRASH` and without `IMMUTABLE_LABELS`. `SPAM` may be in either (removing it is "not spam").
   5. Drop from `add` any user label ID (Gmail user label IDs start with `Label_`) that no longer exists, found by one `labels.list`. Only call `labels.list` when `add` holds such an ID.
   6. If `add` or `remove` is non-empty, `POST modify`.
   7. Re-read with `current_labels` and compare with `exact` minus dropped labels, ignoring `IMMUTABLE_LABELS`. A difference is `Invalid("restore_mismatch")`.
6. `ensure_label(mb, name)`:
   1. Validate: trimmed, 1 to `LABEL_NAME_MAX_CHARS` characters, no control characters, does not start or end with `/`, no `//`, and is not a system label name (case-insensitive: `INBOX`, `SPAM`, `TRASH`, `UNREAD`, `STARRED`, `IMPORTANT`, `SENT`, `DRAFT`, `CHAT`). Otherwise `Invalid("label_name")`.
   2. `labels.list`; return the `id` of the label whose `name` equals `name` ignoring ASCII case (Gmail names are unique ignoring case).
   3. Otherwise `POST /labels` with `CreateLabel`; return its `id`.
   4. On `409` from create, `labels.list` once more and return the match; if still absent, `Transient`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-03 AC1 | Reject moves the message to Gmail trash, never a permanent delete |
| SW-03 AC3 | Suspected spam is reported to Gmail as spam |
| SW-04 AC2 | Filing applies the label and the message leaves the inbox (`add` label, `remove` `INBOX`) |
| SW-05 AC1 | Undo restores the exact previous labels and location |
| SW-05 AC4a | A spam report is undone by restoring labels (the report itself is not recalled) |
| FL-02 AC1 | A missing label is created in the mailbox, then applied |
| INV-5 | No code path permanently deletes a message |
| INV-6 | Every swipe's mailbox change can be reversed |

## Tests that must pass

- `mail_provider_contract_modify_gmail` (contract, all modify cases of the shared suite against `fake-google`)
- `sw_03_ac1_trash_returns_labels_before` (contract)
- `sw_03_ac3_report_spam_adds_spam_removes_inbox` (contract)
- `sw_04_ac2_file_adds_label_removes_inbox` (contract)
- `sw_05_ac1_restore_after_trash_is_exact` (contract: labels `INBOX`, `UNREAD`, `CATEGORY_PROMOTIONS`, `Label_7` restored byte for byte)
- `sw_05_ac1_restore_after_file_is_exact` (contract)
- `sw_05_ac4a_restore_after_spam_is_exact` (contract)
- `fl_02_ac1_ensure_label_creates_once_then_reuses` (contract, including the `409` race injected by `fake-google`)
- `inv_5_gmail_adapter_has_no_delete_call` (unit: reads every `.rs` file under `adapters-gmail/src` except `drive.rs` and fails on `batchDelete`, `Method::DELETE`, `"DELETE"`, `/delete`)
- `inv_5_fake_google_records_no_permanent_delete` (contract: after the whole suite, `fake-google`'s `PERMANENT_DELETE_ATTEMPTED` count is zero)
- `inv_6_restore_property_any_label_set` (property: for random label sets from the fake's label universe, change then restore gives the original)
- `gmail_set_labels_refuses_trash_and_spam` (unit)
- `gmail_restore_skips_deleted_user_label` (contract)
- `gmail_ensure_label_rejects_system_names` (unit)

## Edge cases and traps

- `trash` and `report_spam` must read the labels before changing them; the response after the change is not the undo state.
- `messages.trash` removes `INBOX` and other labels on some accounts; never "guess" the undo state, always restore from the stored set.
- `untrash` does not put `INBOX` back by itself on every account; the modify step after it does.
- `SENT`, `DRAFT` and `CHAT` cannot be added or removed; leave them out of the diff or Gmail returns `400`.
- Never use `messages.delete` "to clean up"; there is no case for it (INV-5, Semgrep rule in S10 6.4).
- Label IDs are opaque (`Label_123`); never compare a label name to an ID.
- Name comparison for `ensure_label` ignores ASCII case only; do not lower-case non-ASCII names.
- Do not log label names: a category name is personal data (S5 C2).

## Out of scope

- Which labels a swipe adds or removes and whether a suspect is also trashed: T-604, T-605.
- The Mail Tinder label on sent unsubscribe mail: T-404 (it calls `ensure_label(MAIL_TINDER_LABEL)`).
- Category rename and delete in the API: T-607.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The `fake-google` modify, trash, untrash and labels routes behave as the table above, including the `409` race switch.
