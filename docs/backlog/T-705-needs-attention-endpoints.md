# T-705: Needs Attention endpoints

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | sonnet | about 250 lines of code plus tests | T-107, T-500 |

**Read only these spec sections:** S7 5.8 API-NA-1 to API-NA-3, S7 2 rows "Pagination" and "Opaque tokens", S7 2.1 "Sealed tokens", S7 1 principle 5, S7 4 rows `invalid_request` and `not_found` (`docs/specs/S7-api-contract.md`), `NeedsAttentionItem` schema and the `/needs-attention` paths in `docs/specs/S7-api-contract.openapi.yaml`, S2 NA-01 AC1, AC2 and UN-05 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`), S3 NeedsAttentionItem state machine (`docs/specs/S3-domain-model.md`), S5 `needs_attention/{id}` row (`docs/specs/S5-data-inventory.md`), ASVS register rows V8.2.2, V1.2.2 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

The `api` serves the Needs Attention list (newest first, paged, with the open count for the tab badge) and lets the user resolve or dismiss an item, which deletes it. The Flutter tab (T-1005) calls these routes.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/needs_attention.rs` | Three handlers and DTOs |
| Change | `backend/crates/api/src/routes/mod.rs` | Register the routes with auth level `user` |
| Create | `backend/crates/api/tests/needs_attention.rs` | Service integration tests |
| Change | `docs/specs/S7-api-contract.openapi.yaml` | Add `sign_in_required` to the `reason_code` enum (see trap) |

## Types and signatures

```rust
// api/src/routes/needs_attention.rs
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct ListQuery { pub cursor: Option<String>, pub limit: Option<u32> }   // default 20, max 50 (S7 2)

#[derive(Serialize)]
pub struct NeedsAttentionItemDto {
    pub item_id: Uuid, pub mailbox_id: Uuid, pub sender_display: String,
    pub reason_code: NeedsAttentionReason,   // serde snake_case, the S7 5.8 values plus sign_in_required
    pub link: Option<String>,                // https only, else null
    pub created_at: OffsetDateTime,
}
#[derive(Serialize)]
pub struct NeedsAttentionListDto { pub items: Vec<NeedsAttentionItemDto>, pub open_count: u64, pub next_cursor: Option<String> }

#[derive(Serialize, Deserialize)]
pub struct NaCursorPayload { pub after: String }   // the StoreCursor from T-201b, sealed before it leaves

pub async fn list(State(app): State<AppState>, session: AuthedSession, Query(q): Query<ListQuery>) -> Result<Json<NeedsAttentionListDto>, ApiError>;
pub async fn resolve(State(app): State<AppState>, session: AuthedSession, Path(item_id): Path<Uuid>) -> Result<StatusCode, ApiError>; // 204
pub async fn dismiss(State(app): State<AppState>, session: AuthedSession, Path(item_id): Path<Uuid>) -> Result<StatusCode, ApiError>; // 204
```

Uses `NeedsAttentionRepo::{by_user, count_for_user, get, delete}` and `NeedsAttentionRecord` (T-201b), `SealedTokens` with `TokenType::Cursor` (T-303), `safe_link` (T-704; if T-704 is not merged yet, copy its rules into a private function and leave a `TODO(T-704)` that T-704 removes).

## Algorithm

`GET /api/v1/needs-attention`:

1. `limit = q.limit.unwrap_or(20)`; outside 1 to 50 gives `400 invalid_request`.
2. `cursor`: if present, `SealedTokens::open(TokenType::Cursor, user, session_record_id, ...)` into `NaCursorPayload`; any failure gives `400 invalid_request`.
3. `store.needs_attention().by_user(user, PageRequest { limit, after })` (created_at descending, NA-01 AC1).
4. For each record: open `sender_display` with Aad field `NA_SENDER_DISPLAY` and the link with `NA_LINK` (scope = item ID). Pass the decrypted link through `safe_link`; failure gives `null` (defence in depth; V1.2.2). Skip records whose `expires_at <= now` (the sweeper may not have run yet; NA-01 AC2).
5. `open_count = count_for_user(user)`.
6. `next_cursor`: seal `NaCursorPayload` from `page.next` as `TokenType::Cursor`, expiry now plus 12 hours; `null` at the end.
7. Response `200` with `Cache-Control: no-store` (T-500 middleware adds it).

`POST /api/v1/needs-attention/{item_id}/resolve` and `/dismiss`:

1. `get(item_id)`. Missing, or `record.user_id != session.user_id`: `404 not_found`. For another user's item also write the security event `authz_denied` (route template only).
2. `delete(item_id, Precondition::Matches(version))`. `PreconditionFailed` (concurrent delete) counts as success.
3. Return `204`. Resolve and dismiss behave the same on the server (both delete, S3); the difference is only what the user meant.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| NA-01 AC1 | Lists every open item, newest first, with sender, reason and link |
| NA-01 AC2 | Items are deleted on resolve or dismiss, and expired items are not listed |
| UN-05 AC1 | An item shows the sender, the link and the reason it stopped |
| V8.2.2 | Another user's item ID returns 404 |
| V1.2.2 | Only https links leave the server |

## Tests that must pass

- `na_01_ac1_lists_open_items_newest_first` (service integration)
- `na_01_ac1_item_has_sender_reason_link` (service integration)
- `na_01_ac1_open_count_matches_items` (service integration)
- `na_01_ac1_pages_with_sealed_cursor` (service integration: 25 items, limit 20, two pages, no duplicates)
- `na_01_ac2_resolve_deletes_item` (service integration)
- `na_01_ac2_dismiss_deletes_item` (service integration)
- `na_01_ac2_expired_item_not_listed` (service integration, virtual clock past `expires_at`)
- `un_05_ac1_item_fields_returned` (service integration: an item raised by `raise_item` round-trips)
- `asvs_v8_2_2_other_users_item_returns_404` (service integration: resolve, dismiss as user B on user A's item; item still exists)
- `asvs_v1_2_2_non_https_link_never_returned` (service integration: a record written with an `http:` link by a test helper returns `link: null`)
- `api_na_1_tampered_cursor_400` and `api_na_1_undo_token_as_cursor_400` (service integration)

## Edge cases and traps

- S7 5.8 and the OpenAPI enum lack a reason for the "Sign in again" item (S2 UN-01 AC6, S9 6). Add `sign_in_required` to the OpenAPI enum in this PR, matching T-701; it is a reported spec gap.
- Return `404`, never `403`, for another user's item (S7 1 principle 5).
- No item ID, sender or link goes in a URL query string or a log line; the item ID in the path is a random UUID.
- `limit` and `cursor` are query parameters on a GET; reject unknown query parameters with `400`.
- Do not cache decrypted sender names between requests.

## Out of scope

- Raising items: T-701, T-704, T-706, T-707. TTL sweeping: T-706.
- The Flutter tab: T-1005.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The OpenAPI file validates after the enum change.
