# T-602c: Feed endpoint

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 450 lines of code plus tests | T-103, T-108, T-303, T-402, T-601a, T-602a, T-602b |

Split from index row T-602 (Feed endpoint). This is the endpoint itself.

**Read only these spec sections:** S7 section 5.4 (API-FEED-1, `Card`, `suggestion`), section 2.1 (sealed tokens) and section 4 (`mailbox_errors` codes) in `docs/specs/S7-api-contract.md`; `FeedPage`, `Card`, `MailboxError` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 FD-01 to FD-04, SW-02 AC2, GM-08 AC1 and AC2, XC-01; S3 "Card visibility"; S10 section 5 (canary tokens) and 7.3. Nothing else is needed.

## Goal

`POST /api/v1/feed/next` returns the next page of cards across all the user's mailboxes, newest first, new mail before backlog, with skipped cards returning, per-mailbox errors as data, and a sealed classification token per card. It advances the stored Feed position in the user state file.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/feed.rs` | Handler, request and response DTOs |
| Create | `backend/crates/api/src/services/feed.rs` | Paging, merging, card building |
| Create | `backend/crates/api/src/text.rs` | `plain_text` sanitiser |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount the route; rate limit 30 per minute per user (S7 section 6) |
| Change | `backend/crates/api/Cargo.toml` | Add `futures = "0.3"` (bounded concurrency) |
| Create | `backend/crates/api/tests/feed.rs` | Service integration tests on `FakeMailbox` and the corpus |

## Types and signatures

```rust
// Used from earlier tasks (keep merged names): AppState, ApiError, AuthedSession (T-500, T-501),
// UserStateStore, UserState, MailboxPosition, SkipState (T-602b), MessageQuery, list_messages,
// count_messages, web_url (T-602a), HeaderRules + header_guard (T-102, T-103),
// SealedTokens::seal / ::open with TokenType::{Cursor, Classification} (T-303),
// domain::feed::{choose_skip_return, is_boss, Tunables} (T-108; is_boss(sender_key, all, &Tunables)), TokenService::mailbox_ctx (T-503).

// backend/crates/api/src/routes/feed.rs
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct FeedRequest { pub cursor: Option<String>, #[serde(default = "default_limit")] pub limit: u32, #[serde(default)] pub refresh: bool }
pub const FEED_LIMIT_DEFAULT: u32 = 20;   // S7 section 2
pub const FEED_LIMIT_MAX: u32 = 50;

#[derive(Serialize)]
pub struct FeedPage { pub cards: Vec<CardDto>, pub next_cursor: Option<String>, pub phase: Phase,
    pub phase_changed: bool, pub mailbox_errors: Vec<MailboxErrorDto>, pub rule_actions_applied: u32 }
#[derive(Serialize, Clone, Copy, PartialEq, Eq)] #[serde(rename_all = "snake_case")]
pub enum Phase { New, Backlog }

#[derive(Serialize)]
pub struct CardDto {
    pub mailbox_id: Uuid, pub message_id: String, pub received_at: OffsetDateTime,
    pub sender_name: String, pub sender_address: String, pub subject: String, pub preview: String,
    pub bulk_score: u8, pub bulk_reason: String, pub class: &'static str, pub has_one_click: bool,
    pub suggestion: Option<SuggestionDto>,      // None here; T-607b fills it
    pub keep_prompt: Option<CategoryRefDto>,    // None here; T-607b fills it
    pub skip_count: u8, pub boss: Option<BossDto>, pub provider_web_url: String,
    pub classification_token: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub classifier_id: Option<String>, // admins only
}
#[derive(Serialize)] pub struct BossDto { pub remaining: u64 }
#[derive(Serialize)] pub struct MailboxErrorDto { pub mailbox_id: Uuid, pub code: &'static str }

// Sealed payloads
#[derive(Serialize, Deserialize)]
pub struct CursorPayload { pub positions: BTreeMap<Uuid, MailboxPosition>, pub phase: Phase }
#[derive(Serialize, Deserialize)]
pub struct ClassificationPayload { pub mailbox_id: Uuid, pub message_id: String,
    pub header_rules: Classification, pub classifier_id: String /* "header_rules@1" */ }
pub const SEALED_TTL_HOURS: i64 = 12;   // [DEFAULT] the session's absolute lifetime (S7 3.2)

// backend/crates/api/src/text.rs
/// Removes every C0 and C1 control character (tab and newline become spaces), bidi controls (U+061C, U+200E, U+200F,
/// U+202A to U+202E, U+2066 to U+2069), zero-width characters (U+200B to U+200D, U+FEFF),
/// collapses runs of whitespace to one space, trims, then truncates to `max_chars` characters.
pub fn plain_text(s: &str, max_chars: usize) -> String;
pub const PREVIEW_MAX_CHARS: usize = 300;   // FD-01 AC2 [TUNABLE]

// backend/crates/api/src/services/feed.rs
pub async fn next_page(app: &AppState, session: &AuthedSession, req: FeedRequest) -> Result<FeedPage, ApiError>;
/// Hook for T-609; returns the IDs the rules acted on (excluded from cards). This task ships a no-op.
pub async fn apply_rules(app: &AppState, session: &AuthedSession, state: &UserState,
    metas: &[MessageMeta]) -> Result<RuleApplication, ApiError>;
pub struct RuleApplication { pub acted_on: BTreeSet<(Uuid, String)>, pub applied: u32 }
pub const PROVIDER_CONCURRENCY: usize = 8;   // [DEFAULT] matches the S4 5.7 model cap; keeps Gmail quota safe
```

## Algorithm

1. Validate: `limit` 1 to 50, else `400 invalid_request` with `fields: ["/limit"]`.
2. `UserStateStore::load`. Starting positions:
   - `cursor` present: `SealedTokens::open(TokenType::Cursor, ...)`. Failure: `400 invalid_request`. Use its positions and phase.
   - `cursor` null: use `state.positions` (FD-03 AC3).
3. Session start: for each connected mailbox whose stored `session_record_id` differs from the session's, or when `refresh` is true: set `new_floor = newest_seen`, `new_ceiling = None`, `new_done = false`, `session_record_id = session's` (FD-03 AC1, AC5). If the session changed, reset `skips` (SW-02 AC2: "until a new session"). First ever use (`newest_seen` None): set `new_done = true` so the user starts in backlog `[DEFAULT]` (there is no "last session" to be newer than).
4. Mailboxes: `mailboxes().by_user`. Status `needs_sign_in` adds a `sign_in_required` error and is skipped. For the rest get a context; a token failure sets status `needs_sign_in` and adds the error (FD-02 AC3).
5. Phase: `new` if any mailbox has `new_done == false`, else `backlog`.
6. Fetch per mailbox, at most `PROVIDER_CONCURRENCY` calls in flight:
   - New: `list_messages({ in_inbox, after: new_floor, before: new_ceiling + 1s }, max = limit)`.
   - Backlog: `list_messages({ in_inbox, before: backlog_ceiling + 1s }, max = limit)` (`None` ceiling means no bound).
   - Drop IDs in `boundary_ids`. A `MailError` adds the mapped code (`Unauthorized` gives `sign_in_required` and sets status; `RateLimited` or `Transient` gives `provider_unavailable`; others `provider_error`) and that mailbox contributes nothing.
7. If the phase is `new` and no mailbox returned anything, mark every mailbox `new_done`, switch to `backlog`, set `phase_changed = true`, and run step 6 again for backlog once.
8. Merge all candidates, sort by `internal_date` descending then by mailbox ID and message ID (stable), keep the first `limit` (FD-02 AC1).
9. `apply_rules` on the kept metas (no-op until T-609); drop any it acted on; add its `applied` to `rule_actions_applied`.
10. Skips: subtract the number of cards on this page from each `skips.queue[i].after_cards`; entries reaching 0 are fetched with `get_meta` and appended at the end of the page if still in the inbox (`NotFound` or no inbox label: drop silently, FD-04). T-108's `choose_skip_return` picked `after_cards` at swipe time.
11. Build each card (bounded concurrency):
    - `get_meta` if the page held IDs only; `NotFound` or no longer in the inbox: drop silently (FD-04 AC1).
    - `get_preview` (T-402 already stripped HTML); `plain_text(preview, 300)` (FD-01 AC2).
    - Classify with `HeaderRules` then `header_guard`; `has_one_click = class == List && facts.list_unsubscribe.one_click_https.is_some()`.
    - `plain_text` every string field to its schema length (256, 320, 998, 200).
    - Seal `ClassificationPayload` as `TokenType::Classification`, expiry now plus 12 hours.
    - Boss (GM-08): `is_boss(&sender_key, &state.sender_stats, &tunables)` (T-108 signature; at least 20 seen and in the top 5 by `seen`, not `boss_defeated`); if so `count_messages({ in_inbox, from: sender })`; a count failure leaves `boss` null.
    - `skip_count` from `skips.counts`; `provider_web_url` from `web_url(address, id)`; `classifier_id` = `"header_rules@1"` only when `session.is_admin`.
12. Compute new positions per mailbox from the cards taken from it: new phase sets `new_ceiling` to its oldest card time, backlog sets `backlog_ceiling`; `boundary_ids` = IDs at that exact second; `newest_seen = max(newest_seen, newest card)`. A mailbox that returned fewer than `limit` and had all of them taken gets `new_done = true` in the new phase.
13. One `UserStateStore::update`: store the request's starting positions (step 2 and 3, so the last page is shown again on resume rather than lost), the updated skip queue, and `sender_stats[sender].seen += 1`, `display`, `last_seen` for each card.
14. `next_cursor` = sealed `CursorPayload` with the step 12 positions; `null` when every mailbox is `new_done` and the backlog fetch returned nothing.
15. Every mailbox failed: still `200`, no cards, every mailbox listed (S7 5.4).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-01 AC1 | Each card has sender name and address, subject, preview, mailbox ID and badge fields |
| FD-01 AC2 | Preview is plain text of at most 300 characters with HTML stripped |
| FD-01 AC3 | Building cards writes no message content to the server store or logs |
| FD-02 AC1 | Cards from several mailboxes are interleaved by received time, newest first |
| FD-02 AC2 | Each card carries its `mailbox_id` |
| FD-02 AC3 | One failing mailbox appears in `mailbox_errors` and the others still load |
| FD-03 AC1 | Mail newer than the last session comes before older mail |
| FD-03 AC2 | After new mail runs out the Feed continues with older mail and reports `phase_changed` once |
| FD-03 AC3 | A null cursor resumes from the stored position |
| FD-03 AC4 | No daily cap: pages keep coming until the inbox is exhausted |
| FD-03 AC5 | `refresh: true` checks every mailbox for new mail |
| FD-04 AC1 | A message trashed or moved elsewhere since listing is skipped silently |
| SW-02 AC2 | A skipped card returns later, at most `SKIP_MAX_RETURNS` times, and not again until a new session |
| GM-08 AC1 | A sender with at least 20 cards seen and in the top 5 is a boss |
| GM-08 AC2 | A boss card carries `boss.remaining` from one count query |
| XC-01 | No body, subject, address or URL from the corpus appears in captured logs |

## Tests that must pass

- `fd_01_ac1_card_has_required_fields` (service integration)
- `fd_01_ac2_preview_plain_and_capped` (service integration, hostile corpus case with a 10,000 character body)
- `fd_01_ac2_bidi_and_control_characters_stripped` (unit, `api::text`)
- `fd_01_ac3_no_card_content_in_store_or_logs` (service integration with canary corpus: scan the store and captured `tracing` output)
- `fd_02_ac1_interleaved_newest_first` (service integration, two mailboxes)
- `fd_02_ac2_card_names_mailbox` (service integration)
- `fd_02_ac3_failing_mailbox_listed_others_load` (service integration; also the all-fail case returns `200` with no cards)
- `fd_03_ac1_new_mail_before_backlog` (service integration)
- `fd_03_ac2_phase_changed_once` (service integration)
- `fd_03_ac3_null_cursor_resumes_position` (service integration)
- `fd_03_ac4_no_daily_cap` (service integration: 500 messages page through to the end)
- `fd_03_ac5_refresh_finds_new_mail` (service integration)
- `fd_04_ac1_moved_message_skipped_silently` (service integration)
- `sw_02_ac2_skipped_card_returns_at_most_twice` (service integration with a seeded `Rng`)
- `sw_02_ac2_skips_reset_on_new_session` (service integration)
- `gm_08_ac1_boss_threshold` (service integration)
- `gm_08_ac2_boss_remaining_from_one_count` (service integration)
- `xc_01_feed_logs_hold_no_corpus_values` (service integration log scan)
- `asvs_v9_2_2_feed_rejects_undo_token_as_cursor` (service integration: an undo token sent as `cursor` gives `400`)
- `s9_feed_empty_no_mail` (service integration: empty mailboxes give no cards and `next_cursor` null)

## Edge cases and traps

- `GET` must never advance anything; this route is `POST` and CSRF-checked (T-500 middleware).
- Do not trust or reuse a class from anywhere but `HeaderRules` plus the guard in this request.
- Gmail `before:` is exclusive at second precision; use `ceiling + 1s` and `boundary_ids` to avoid both gaps and duplicates.
- Sort ties by mailbox and message ID so pages are deterministic in tests.
- Strip characters before truncating, and truncate on `char` boundaries (never byte slicing).
- `classifier_id` must be absent, not `null`, for non-admins.
- No card field, address, message ID or provider error text goes into logs or `ApiError` bodies.
- The closure passed to `UserStateStore::update` must not call providers (T-602b trap).
- Never write to `localStorage` on the app side; not this task, but do not add a cache header other than `no-store`.

## Out of scope

- Filing suggestion and keep prompt values: T-607b (this task returns `null` for both).
- Applying sort rules and appending job outcomes to History: T-609.
- Delivery check: T-707. Model predictions in the classification token: T-901.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A response from the integration test validates against the OpenAPI `FeedPage` schema.
