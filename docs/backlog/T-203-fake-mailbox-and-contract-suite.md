# T-203: FakeMailbox and the MailProvider and AppFolderStore contract suites

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 450 lines of code plus about 350 lines of contract tests | T-202b |

**Read only these spec sections:** S10 4.1 and 4.3 (`docs/specs/S10-test-strategy.md`); S3 "Swipe and undo" state machine and INV-5, INV-6 (`docs/specs/S3-domain-model.md`); `docs/backlog/T-201a-port-traits.md` ("mail.rs" and "app_folder.rs" blocks); `docs/backlog/T-403-gmail-trash-labels-spam-restore.md` step 4 (which labels `set_labels` refuses). Nothing else is needed.

## Goal

`testkit` gains `FakeMailbox`, an in-memory `MailProvider` that models Gmail's label behaviour closely enough for domain property tests and service integration tests, and two shared contract suites: `contract::mail_provider` (run here against `FakeMailbox`, and by T-401, T-403 and T-404 against the Gmail adapter on `fake-google`) and `contract::app_folder_store` (run here against `InMemoryAppFolder`, and by T-405 against Drive on `fake-google`). Same assertions on both sides, so the fakes cannot drift.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/testkit/src/mailbox/mod.rs` | `FakeMailbox` |
| Create | `backend/crates/testkit/src/mailbox/state.rs` | per-mailbox state: messages, labels, sent log |
| Create | `backend/crates/testkit/src/contract/mail_provider.rs` | shared suite, grouped into read, modify and send cases |
| Create | `backend/crates/testkit/src/contract/app_folder_store.rs` | shared suite |
| Change | `backend/crates/testkit/src/contract/mod.rs` | `pub mod mail_provider; pub mod app_folder_store;` |
| Change | `backend/crates/testkit/src/fake_ports.rs` | `gmail` becomes `FakeMailbox`; add `pub mailbox: Arc<FakeMailbox>` to `Fakes`; delete `NullMailProvider` |
| Create | `backend/crates/testkit/tests/mail_provider_fake.rs` | suite against `FakeMailbox` |
| Create | `backend/crates/testkit/tests/app_folder_fake.rs` | suite against `InMemoryAppFolder` |

## Types and signatures

```rust
// testkit/src/mailbox/mod.rs
pub struct SeedMessage {
    pub from_display: String,          // example.com addresses only
    pub from_address: String,
    pub subject: String,
    pub raw_headers: Vec<(String, String)>, // in order; used by fake-google, ignored by FakeMailbox
    pub facts: HeaderFacts,            // FakeMailbox returns these as-is (adapters compute their own)
    pub preview_text: String,          // plain text
    pub internal_date: OffsetDateTime,
    pub labels: Vec<String>,           // e.g. ["INBOX", "UNREAD"]
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentRecord { pub mailbox: MailboxId, pub to: String, pub subject: Option<String>, pub body: Option<String> }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MailOp { ListInbox, GetMeta, GetPreview, SetLabels, Trash, ReportSpam, RestoreLabels, EnsureLabel, SendMailto, InboxCount }

pub struct FakeMailbox { /* Mutex<HashMap<MailboxId, MailboxState>>, failures, call log */ }
impl FakeMailbox {
    pub fn new() -> Self;
    pub fn seed(&self, mb: &MailboxId, msg: SeedMessage) -> MessageId;          // ids "m0001", "m0002", ...
    pub fn labels_of(&self, mb: &MailboxId, id: &MessageId) -> Option<LabelSet>;
    pub fn sent(&self) -> Vec<SentRecord>;
    pub fn fail_next(&self, op: MailOp, err: MailError);                       // one-shot, FIFO per op
    pub fn calls(&self, op: MailOp) -> u64;
    pub fn revoke_token(&self, token: &str);                                   // later calls with it: Unauthorized
}
#[async_trait] impl MailProvider for FakeMailbox { /* below */ }

// testkit/src/contract/mail_provider.rs
#[async_trait::async_trait]
pub trait MailSeeder: Send + Sync {
    async fn seed(&self, msg: &SeedMessage) -> Result<MessageId, String>;
    async fn labels_of(&self, id: &MessageId) -> Result<LabelSet, String>;    // read back outside the adapter
    async fn sent(&self) -> Result<Vec<SentRecord>, String>;
    async fn permanent_delete_attempts(&self) -> Result<u64, String>;         // FakeMailbox: always 0
}
pub struct MailTarget { pub provider: Arc<dyn MailProvider>, pub ctx: MailboxCtx, pub seeder: Arc<dyn MailSeeder> }
#[derive(Clone, Copy, Debug)] pub struct CaseGroups { pub read: bool, pub modify: bool, pub send: bool }
impl CaseGroups { pub const ALL: CaseGroups = CaseGroups { read: true, modify: true, send: true }; }
/// `make` returns a fresh, empty mailbox each call.
pub async fn mail_provider<F, Fut>(make: F, groups: CaseGroups) -> Result<(), String>
where F: Fn() -> Fut, Fut: Future<Output = MailTarget>;

// testkit/src/contract/app_folder_store.rs
#[async_trait::async_trait]
pub trait AppFolderControl: Send + Sync { async fn user_deletes_file(&self) -> Result<(), String>; }
pub struct AppFolderTarget { pub store: Arc<dyn AppFolderStore>, pub ctx: MailboxCtx, pub control: Arc<dyn AppFolderControl> }
pub async fn app_folder_store<F, Fut>(make: F) -> Result<(), String>
where F: Fn() -> Fut, Fut: Future<Output = AppFolderTarget>;
```

## Algorithm

Gmail model in `FakeMailbox` (S10 4.1), per mailbox:

1. A message holds its `SeedMessage` data and a `BTreeSet<String>` of label IDs. Labels table: system IDs `INBOX`, `TRASH`, `SPAM`, `SENT`, `UNREAD`, `STARRED`, `IMPORTANT`, plus user labels `Label_1`, `Label_2`, ... with names.
2. Access token check: every method first checks `mb.access_token.expose()` against the revoked set; revoked gives `Unauthorized`. Then pops a scripted failure for that `MailOp`, if any.
3. `list_inbox(page, order)`: messages with `INBOX` and without `TRASH` or `SPAM`; filter by `order` (`NewerThan(t)`: `internal_date > t`; `OlderThan(t)`: `internal_date < t`); sort newest first, ties by ID; page size 20 `[DEFAULT]` (matches T-401 `LIST_PAGE_SIZE`); `PageToken` = `"o:<offset>"`; an unparsable token is `Invalid("bad_page_token")`. Each item is the `MessageMeta` built from the seed (labels as they are now).
4. `get_meta`, `get_preview`: unknown ID is `NotFound`. The preview is the stored plain text.
5. `set_labels(add, remove)`: refuse `TRASH`, `SPAM`, `SENT`, `DRAFT`, `CHAT` in either set with `Invalid("label_not_allowed")` (same rule as T-403); an unknown user label ID in `add` is `Invalid("unknown_label")`; apply remove then add; return the new set.
6. `trash`: return the set before; then add `TRASH` and remove `INBOX` `[ASSUMES]` Gmail behaviour; T-1106's live run confirms. `report_spam`: return the set before; add `SPAM`, remove `INBOX`.
7. `restore_labels(exact)`: replace the set with `exact` exactly; a user label ID in `exact` that no longer exists is dropped (same as T-403 step 5.5).
8. `ensure_label(name)`: case-insensitive name match returns the existing ID; otherwise create the next `Label_n`.
9. `send_mailto(to)`: push a `SentRecord` and store a new message labelled `SENT` only. Take field names from T-101's `MailtoTarget`.
10. `inbox_count`: number of messages `list_inbox` would return with no bounds.
11. No delete operation exists anywhere in `FakeMailbox` (INV-5).

Contract suite cases (each on a fresh target; return `Err("<case>: <detail>")`):

- Read: seeded three messages with distinct dates come back newest first; paging with a page token covers all with no duplicates (seed 25); `NewerThan` and `OlderThan` bound correctly; `get_meta` returns from, subject, date and labels; unknown ID is `NotFound`; `inbox_count` equals seeded inbox count; a trashed message is not listed.
- Modify: `trash` returns the exact set before, and `restore_labels(before)` gives back exactly that set (read via `seeder.labels_of`); same for `report_spam`; `set_labels` add then `restore_labels` restores; `set_labels` with `TRASH` is `Invalid`; `ensure_label` twice with different case returns one ID; restore after restore is idempotent.
- Send: `send_mailto` records one sent message with the target's address, subject and body; nothing else is sent.
- Always, at the end: `permanent_delete_attempts()` is 0.

App folder suite cases: empty read is `None`; `write(None)` creates and returns an ETag; second `write(None)` is `Conflict`; `write(Some(current))` succeeds and changes the ETag; `write(Some(stale))` is `Conflict` and the content is unchanged; after `user_deletes_file`, read is `None` and `write(Some(old))` is `Conflict`; `delete` twice is `Ok`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| INV-5 | The fake has no delete path, and the contract suite fails if any target records a permanent delete attempt |
| INV-6 | Every mailbox change the suite makes can be reversed to the exact previous label set |

## Tests that must pass

- `mail_provider_contract_fake_mailbox` (contract, `testkit/tests/mail_provider_fake.rs`, `CaseGroups::ALL`).
- `app_folder_store_contract_in_memory` (contract, `testkit/tests/app_folder_fake.rs`).
- `inv_5_fake_mailbox_records_no_permanent_delete` (contract: run the full suite, then assert zero attempts).
- `inv_6_restore_labels_returns_exact_previous_set` (property, `proptest`: random starting label sets and a random sequence of trash, spam and set_labels; restoring the first "before" set always gives it back exactly).
- `fake_mailbox_revoked_token_is_unauthorized` (unit).
- `fake_mailbox_fail_next_is_one_shot` (unit).

## Edge cases and traps

- `LabelSet` comparison must be exact set equality, not "contains".
- `trash` and `report_spam` return the labels before the change, not after.
- `set_labels` must not be a back door to trash or spam; that is why the refused labels match T-403.
- The suite must not assume Gmail where a capability flag exists: read `provider.capabilities()` before asserting label-set behaviour (all true for Gmail in v1).
- Seed data uses example.com and example.org only, and every subject and preview contains a `CANARY-<case>-<field>` token (S10 5), so later leak scans can use this suite's output.
- Message IDs are opaque strings; never parse them.
- No `unwrap` in the suite; it returns `Result<(), String>` so adapter runs name the failing case.
- `fail_next` is FIFO per operation and one-shot; tests must not leave scripted failures behind (assert the queue is empty at the end of each test that uses it).

## Out of scope

- The Gmail adapter (T-401, T-403, T-404) and Drive (T-405), which run these suites.
- The synthetic `.eml` corpus (T-204a).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `fake_ports()` returns a `Ports` whose `gmail` is the same `FakeMailbox` as `Fakes::mailbox`.
