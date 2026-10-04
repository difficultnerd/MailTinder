# T-607a: Categories endpoints

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 350 lines of code plus tests | T-602a, T-604 |

Split from index row T-607 (categories and filing suggestions). Suggestions are T-607b.

**Read only these spec sections:** S7 section 5.6 (API-CAT-1 to API-CAT-5) in `docs/specs/S7-api-contract.md`; `Category`, `CategoryName`, `FiledMessage` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 FL-02 AC1, FL-05 AC1; S3 INV-5; S9 section 5. Nothing else is needed.

## Goal

The Filed tab's backend: list categories with label counts per mailbox, create, rename and delete a category, and list the messages in one category across mailboxes.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/categories.rs` | Five handlers and DTOs |
| Change | `backend/crates/api/src/services/categories.rs` | Create, rename, delete, list messages |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount routes |
| Create | `backend/crates/api/tests/categories.rs` | Service integration tests |

## Types and signatures

```rust
#[derive(Serialize)] pub struct CategoryDto { pub category_id: Uuid, pub name: String, pub message_count: u64, pub per_mailbox: Vec<PerMailboxCount> }
#[derive(Serialize)] pub struct PerMailboxCount { pub mailbox_id: Uuid, pub message_count: u64 }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct CategoryNameDto { pub name: String }
#[derive(Serialize)] pub struct FiledMessageDto { pub mailbox_id: Uuid, pub message_id: String, pub sender_name: String,
    pub subject: String, pub received_at: OffsetDateTime, pub provider_web_url: String }
#[derive(Serialize)] pub struct FiledPage { pub messages: Vec<FiledMessageDto>, pub next_cursor: Option<String>, pub mailbox_errors: Vec<MailboxErrorDto> }

#[derive(Serialize, Deserialize)]
pub struct CategoryCursor { pub category_id: Uuid, pub pages: BTreeMap<Uuid, Option<String>>, pub exhausted: BTreeSet<Uuid> } // sealed TokenType::Cursor

pub async fn list(app: &AppState, s: &AuthedSession) -> Result<Vec<CategoryDto>, ApiError>;
pub async fn create(app: &AppState, s: &AuthedSession, name: &str) -> Result<CategoryDto, ApiError>;
pub async fn rename(app: &AppState, s: &AuthedSession, id: Uuid, name: &str) -> Result<CategoryDto, ApiError>;
pub async fn delete(app: &AppState, s: &AuthedSession, id: Uuid) -> Result<(), ApiError>;
pub async fn messages(app: &AppState, s: &AuthedSession, id: Uuid, cursor: Option<&str>, limit: u32) -> Result<FiledPage, ApiError>;
```

## Algorithm

1. API-CAT-1: load user state. For each category and each connected mailbox where `labels` has an ID: `count_messages({ label })` (exact label count, T-602a). A failed count leaves that mailbox out of `per_mailbox`. `message_count` sums `per_mailbox`. Sort categories by name, case-insensitive.
2. API-CAT-2: `validate_category_name` (T-604). A case-insensitive clash: `409 category_exists`. Else push `Category { category_id: Rng::uuid_v4, name, labels: empty, created_at }` in one update, `totals.categories_created += 1`. Labels are created lazily on first file (S7). Return `201` with zero counts.
3. API-CAT-3: unknown ID: `404 not_found`. Clash with another category: `409 category_exists`. For each mailbox with a label: `rename_label`; a provider `Invalid("label_exists")` maps to `409 category_exists`; other provider failures `502`. Then save the name. Return `200` with counts.
4. API-CAT-4: unknown ID: `404`. For each mailbox with a label: `remove_label` (label definition only; messages stay, INV-5). Then one update removes the category and every `File` rule whose category is this one, and drops the category from `sender_stats[*].files` and `last_filed`. Return `204`.
5. API-CAT-5: `limit` 1 to 50 (default 20). Open the cursor if given (`400` on failure, or if its `category_id` differs). For each mailbox with a label not in `exhausted`: `list_messages({ label }, page, max = limit)`, `get_meta` per ID if needed. Merge newest first, take `limit`, record each mailbox's next page token; a mailbox whose page is used up and has no token goes into `exhausted`. Strings through `plain_text`. Mailbox failures go into `mailbox_errors`. `next_cursor` null when every mailbox is exhausted.

`[DEFAULT]` page simplicity: a mailbox's whole page is consumed before its next token is used, so a few older messages can appear on a later page than strict merging would put them. Good enough for a browsing list.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FL-05 AC1 | Categories are listed with message counts, and a category opens to its messages |
| FL-02 AC1 | A category created by name gets its label in a mailbox when first used |
| INV-5 | Deleting a category removes the label only; no message is deleted |

## Tests that must pass

- `fl_05_ac1_categories_with_counts` (service integration, two mailboxes)
- `fl_05_ac1_category_messages_across_mailboxes` (service integration, paging through 60 messages)
- `fl_02_ac1_create_then_first_file_creates_label` (service integration)
- `fl_02_ac1_create_duplicate_name_conflict` (service integration: `409 category_exists`, case-insensitive)
- `inv_5_delete_category_keeps_messages` (service integration: messages still exist, label gone, filing rules for it removed)
- `s9_filed_rename_category` (service integration: label renamed in every mailbox)
- `s9_filed_provider_error_per_mailbox` (service integration)
- `asvs_v8_2_2_other_users_category_not_found` (service integration)

## Edge cases and traps

- `category_id` is in the path; the cursor and limit are query parameters. Never accept a label name or address in a URL.
- Deleting a category must never call anything that deletes messages; only `remove_label`.
- Provider calls go before the user state update, never inside its closure.
- A category with no labels yet has counts of zero and no provider calls.

## Out of scope

- Filing suggestions and the keep prompt: T-607b.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
