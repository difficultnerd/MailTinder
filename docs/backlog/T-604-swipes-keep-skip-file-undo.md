# T-604: Swipes: keep, skip, file and their undo

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 450 lines of code plus tests | T-105a, T-105b, T-403, T-602c |

**Read only these spec sections:** S7 section 5.5 (API-SW-1 and API-SW-2, outcome table, idempotency) and section 2.1 (sealed tokens) in `docs/specs/S7-api-contract.md`; `SwipeRequest` and `SwipeResult` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 SW-01, SW-02, SW-04 AC2, SW-05 AC1 and AC4, FL-02 AC1, FL-03 AC2, CL-03 AC4; S3 "Swipe and undo" and INV-5, INV-6. Nothing else is needed.

## Goal

`POST /api/v1/swipes` handles `keep`, `skip` and `file`, and `POST /api/v1/swipes/undo` reverses them exactly. This task builds the shared swipe pipeline (validation, idempotency, token checks, fresh re-read, undo token) that T-605 and T-606 extend for `reject`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/swipes.rs` | Both handlers, request and response DTOs |
| Create | `backend/crates/api/src/services/swipe.rs` | Pipeline and keep, skip, file execution |
| Create | `backend/crates/api/src/services/undo.rs` | Undo pipeline and keep, skip, file reversal |
| Create | `backend/crates/api/src/services/categories.rs` | `resolve_or_create_category`, `ensure_label_for` (T-607a extends) |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount routes; rate limit 60 per minute, burst 10, per user (S7 section 6) |
| Create | `backend/crates/api/tests/swipes_keep_skip_file.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks: SwipeInput, SwipePlan, plan_swipe, derive_swipe_ids, SwipeOutcome (T-105a);
// SwipeRecord, plan_undo, reverse_stats, UndoResponse (T-105b); SealedTokens, TokenType, http_status_for (T-303);
// UserStateStore, RecentSwipe, HistoryEntry (T-602b); ClassificationPayload, plain_text (T-602c);
// HeaderRules + header_guard (T-102, T-103); choose_skip_return (T-108).

#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct SwipeRequest { pub mailbox_id: Uuid, pub message_id: String, pub action: ActionDto,
    pub category_id: Option<Uuid>, pub new_category_name: Option<String>, pub classification_token: String }
#[derive(Deserialize, Clone, Copy)] #[serde(rename_all = "snake_case")]
pub enum ActionDto { Keep, Skip, Reject, File }

#[derive(Serialize, Deserialize, Clone)]
pub struct SwipeResultDto {
    pub outcome: SwipeOutcome,
    pub unsubscribe_due_at: Option<OffsetDateTime>,
    pub filed_category: Option<CategoryRefDto>,
    pub undo_token: String,
    pub prompts: Vec<PromptDto>,                     // empty here; T-605 fills block_person
    pub achievements_unlocked: Vec<AchievementDto>,  // empty here; T-802 fills
    pub boss_defeated: bool,                          // false here; T-605 sets
}

/// The sealed undo payload: T-105b's record plus what the api needs to reverse its own writes.
#[derive(Serialize, Deserialize, Clone)]
pub struct UndoPayload { pub record: SwipeRecord, pub swipe_id: Uuid,
    pub history_entry: Option<Uuid>, pub na_item: Option<Uuid>, pub skip_key: Option<String> }

pub const NS_SWIPE: Uuid = Uuid::from_u128(0x6d74_7377_0000_4000_8000_000000000001); // [DEFAULT] fixed namespace
pub fn swipe_id(user: &UserId, idempotency_key: Uuid) -> Uuid; // Uuid::new_v5(&NS_SWIPE, user bytes ++ key bytes)

pub async fn swipe(app: &AppState, session: &AuthedSession, idempotency_key: Uuid, req: SwipeRequest)
    -> Result<SwipeResultDto, ApiError>;
pub async fn undo(app: &AppState, session: &AuthedSession, token: &str) -> Result<UndoResponse, ApiError>;

// services/categories.rs
pub async fn resolve_or_create_category(app: &AppState, user: &UserId, id: Option<Uuid>, name: Option<&str>,
    swipe_id: Uuid) -> Result<Category, ApiError>;
pub async fn ensure_label_for(app: &AppState, user: &UserId, ctx: &MailboxCtx, category: &Category)
    -> Result<String, ApiError>;  // label ID, created lazily and saved to Category.labels
pub fn validate_category_name(raw: &str) -> Result<String, ApiError>; // trimmed, 1..=100 chars, no '/' first or last
```

## Algorithm

`swipe`:

1. `Idempotency-Key` header must be a UUID, else `400 invalid_request` with `fields: ["Idempotency-Key"]`. `file` needs exactly one of `category_id` and `new_category_name`; other actions need neither. Validate the name.
2. `swipe_id = swipe_id(user, key)`. Load user state; if `recent_swipes` has `swipe_id`, rebuild the stored result, re-seal its undo payload and return it unchanged (a retry after a success; no provider call).
3. Mailbox must belong to the user, else `404 not_found`.
4. Classification token: the clear type must be `classification`, else `400` (V9.2.2). `open`: success must name the same mailbox and message, else `400 invalid_request`. Any other open failure (earlier session, expiry) is not an error: continue and mark "no eval" (S7 5.5).
5. Re-read: `get_meta`. `NotFound`, or the message lacks the inbox label: `409 message_changed`. Classify again with `HeaderRules` plus `header_guard`; the token's class is never used for the action (CL-03 AC4).
6. `plan_swipe(SwipeInput { action, meta, badge, stats, ids: derive_swipe_ids(user, key), source_swipe: key, now, tunables })`.
7. `reject`: hand over to `services::reject::execute` (T-605). Until T-605 is merged this arm returns `500 internal_error`.
8. `keep` (SW-01): no provider call. `skip` (SW-02): no provider call; `choose_skip_return(skips_this_session, cards_left, rng draw, tunables)`; `None` means the cap is reached and the card is not queued.
9. `file` (SW-04 AC2, FL-02 AC1): `resolve_or_create_category` (an existing name, case-insensitive, is reused, as S9 section 4 says), `ensure_label_for`, then `set_labels(add {label}, remove {INBOX})`. A provider error maps to `502 provider_error` or `503 provider_unavailable`; nothing is recorded.
10. One `UserStateStore::update`: `sender_stats = plan.stats_after`; skip queue and `skips.counts` for skips; `totals.triaged += 1` for keep and file, `totals.cleared += 1` for file; for file, push History `{ entry_id: swipe_id, action: filed, outcome: done }`; push `RecentSwipe`.
11. Seal `UndoPayload` (`TokenType::Undo`, expiry = the session's absolute expiry). Return `200`.

`undo` (SW-05 AC1, AC4):

1. `open(TokenType::Undo)`; any failure is `410 undo_expired` (`http_status_for`).
2. `plan_undo(record)`. `reject` records go to `services::undo::undo_reject` (T-606); until then `500`.
3. File: `restore_labels(exact previous_labels)`. Failure: `502 provider_error`; the token stays valid (nothing else changed).
4. One update: `reverse_stats`; remove the skip queue entry and decrement `skips.counts`; remove the History entry `history_entry`; `totals` back; drop the `RecentSwipe`. A category created by this swipe stays (the label exists in the mailbox) `[DEFAULT]`.
5. Return `{ restored: true, unsubscribe_already_sent: false }`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-01 AC1 | Keep leaves the message unchanged in the mailbox |
| SW-01 AC2 | Keep is counted in the sender's keep count |
| SW-02 AC1 | Skip changes nothing in the mailbox |
| SW-02 AC2 | Skip queues a return at most `SKIP_MAX_RETURNS` times |
| SW-04 AC2 | File applies the label and removes the message from the inbox |
| FL-02 AC1 | A new category name creates the label if absent and applies it |
| FL-03 AC2 | Nothing is filed without a `file` swipe request |
| SW-05 AC1 | Undo restores the exact previous labels and the card can return |
| SW-05 AC4 | Undo works back through every swipe of the session, one per call |
| CL-03 AC4 | The swipe accepts only a classification token for that message and re-runs the header rules |
| INV-5 | No swipe path deletes a message |
| INV-6 | Every keep, skip and file in the session can be undone |

## Tests that must pass

- `sw_01_ac1_keep_leaves_labels_unchanged` (service integration)
- `sw_01_ac2_keep_counted_for_sender` (service integration)
- `sw_02_ac1_skip_changes_nothing` (service integration)
- `sw_02_ac2_third_skip_not_queued` (service integration)
- `sw_04_ac2_file_applies_label_and_leaves_inbox` (service integration)
- `fl_02_ac1_new_category_creates_label` (service integration; second swipe with the same name in other case reuses it)
- `fl_03_ac2_no_filing_without_file_request` (service integration: keep and skip never call `set_labels`)
- `sw_05_ac1_undo_file_restores_exact_labels` (service integration; label set compared exactly)
- `sw_05_ac1_undo_restore_failure_keeps_token_valid` (service integration: first undo `502`, second succeeds)
- `sw_05_ac4_undo_walks_back_three_swipes` (service integration)
- `cl_03_ac4_token_for_other_message_refused` (service integration: `400`)
- `cl_03_ac4_expired_token_swipe_proceeds` (service integration)
- `asvs_v9_2_2_undo_token_as_classification_refused` (service integration)
- `asvs_v2_3_4_retried_swipe_counts_once` (service integration: same `Idempotency-Key` twice gives the same response and one keep)
- `asvs_v8_2_2_swipe_on_other_users_mailbox_not_found` (service integration)
- `fd_04_ac1_swipe_on_moved_message_conflict` (service integration: `409 message_changed`)
- `inv_5_swipe_paths_never_delete` (service integration: `fake-google` records no permanent delete over every action)
- `inv_6_every_session_swipe_undoable` (property: any sequence of keep, skip and file swipes followed by the same number of undos restores labels and stats)

## Edge cases and traps

- Check `recent_swipes` before the re-read: after a successful file the message is no longer in the inbox, so a retry would wrongly get `409`.
- Provider change first, then the state update; never the other way round, and never a provider call inside the `update` closure.
- The undo token is bound to `session_record_id`, not the cookie; rotation must not break undo.
- The previous label set comes from the fresh `get_meta` in this request, not from the client or the token.
- Never put the message ID, address or label names in logs or error bodies.
- Gmail treats `/` as nesting: refuse names starting or ending with `/`.

## Out of scope

- Reject and its undo: T-605, T-606. Writing `classifier_eval`: T-906a. Achievements: T-802. Filing suggestions: T-607b.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Responses validate against the OpenAPI `SwipeResult` schema.
