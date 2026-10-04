# T-602b: User state file: schema, sealed load and If-Match update

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 350 lines of code plus tests | T-101, T-104, T-108, T-302, T-405, T-503 |

Split from index row T-602 (Feed endpoint). Every M6 and M8 endpoint reads or writes the user's app folder file; this task defines what is in it and the one safe way to change it.

**Read only these spec sections:** S3 "Where each entity lives" (User app folder bullet) and the "User app folder file" entity table in `docs/specs/S3-domain-model.md`; S5 "User app folder" section; S6 section 5 (`data_key` row); S10 section 6.3 row "App folder writes (option B)"; S2 SR-01 AC5, FD-03 AC3, GM-06 AC2. Nothing else is needed.

## Goal

A typed `UserState` (rules, categories, sender stats, History, Feed positions, pending unsubscribes, pending delivery checks, achievements, totals) and a `UserStateStore` in `api` that loads it from the primary mailbox's Drive app folder, decrypts it under the user's `data_key`, and applies changes with ETag `If-Match` and retry, so two requests never overwrite each other.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/user_state.rs` | `UserState` and its parts (pure data, serde) |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod user_state;` |
| Create | `backend/crates/api/src/services/user_state_store.rs` | Load, decrypt, update with retry |
| Create | `backend/crates/api/tests/user_state_store.rs` | Service integration tests |

## Types and signatures

```rust
// backend/crates/domain/src/user_state.rs
pub const USER_STATE_VERSION: u32 = 1;
pub const HISTORY_RETENTION_DAYS: i64 = 365;   // S5 "Trimmed at 12 months" [TUNABLE]
pub const RECENT_SWIPES_MAX: usize = 50;       // [DEFAULT] enough for one session's retries

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserState {
    pub version: u32,
    pub rules: Vec<StoredRule>,
    pub categories: Vec<Category>,
    pub sender_stats: BTreeMap<String, SenderStats>,      // key: SenderKey as string
    pub history: Vec<HistoryEntry>,                        // newest last; endpoints sort
    pub positions: BTreeMap<Uuid, MailboxPosition>,        // key: MailboxId
    pub skips: SkipState,
    pub recent_swipes: Vec<RecentSwipe>,                   // idempotent retry of API-SW-1
    pub pending_unsubscribes: BTreeMap<Uuid, PendingUnsubscribe>, // key: JobId
    pub pending_delivery_checks: Vec<PendingDeliveryCheck>,
    pub achievements: Vec<AchievementRecord>,
    pub totals: Totals,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredRule {
    pub rule: SortRule,            // from T-104: id, kind, match (sender, list_id, feedback_id), category, enabled, created_at, source_swipe
    pub times_applied: u64,
    pub yearly_rate: Option<u32>,  // GM-05; None when the count query failed
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub category_id: CategoryId,
    pub name: String,                         // 1 to 100 chars, unique case-insensitively
    pub labels: BTreeMap<Uuid, String>,       // MailboxId -> provider label ID, created lazily
    pub created_at: OffsetDateTime,
}

// SenderStats is owned by T-101 (domain). Import it; do not redefine it here.
// This task needs these fields on it. Any that T-101 lacks are added to T-101's struct in this
// pull request, each with #[serde(default)]:
//   display: String, seen: u32, keeps: u32, rejects_counted: Vec<OffsetDateTime> (authenticated
//   rejects only, keep last 20), files: BTreeMap<Uuid, u32> (CategoryId -> count),
//   last_filed: Option<CategoryId>, last_seen: Option<OffsetDateTime>,
//   block_prompt_declined_until: Option<OffsetDateTime>, boss_defeated: bool
use crate::SenderStats; // T-101 path; use the merged path

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub entry_id: Uuid,
    pub at: OffsetDateTime,
    pub mailbox_id: MailboxId,
    pub sender_display: String,
    pub action: HistoryAction,
    pub outcome: HistoryOutcome,
    pub rule_id: Option<RuleId>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction { TrashedByRule, Unsubscribe, Filed, FiledByRule, Blocked, ReportedSpam }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcome { Sent, NeedsAttention, Failed, Cancelled, Expired, Done }   // S7 API-HIST-1

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MailboxPosition {
    pub newest_seen: Option<OffsetDateTime>,   // newest received_at ever returned as a card
    pub new_floor: Option<OffsetDateTime>,     // "new" means received after this (set at session start)
    pub new_ceiling: Option<OffsetDateTime>,   // paging within "new": received before this
    pub new_done: bool,
    pub backlog_ceiling: Option<OffsetDateTime>, // backlog: received before this
    pub boundary_ids: Vec<String>,             // message IDs already returned at the ceiling second
    pub session_record_id: Option<Uuid>,       // session that set new_floor
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SkipState {
    pub session_record_id: Option<Uuid>,       // skips reset when the session changes (SW-02 AC2)
    pub counts: BTreeMap<String, u8>,          // "mailbox_id/message_id" -> skips this session
    pub queue: Vec<SkipReturn>,                // cards waiting to come back
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkipReturn { pub mailbox_id: MailboxId, pub message_id: String, pub after_cards: u32 }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecentSwipe { pub swipe_id: Uuid, pub at: OffsetDateTime, pub result_json: String } // stored API-SW-1 result minus undo token, plus the undo payload

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingUnsubscribe {
    pub mailbox_id: MailboxId, pub sender_display: String, pub sender_key: String,
    pub list_id: Option<String>, pub rule_id: Option<RuleId>, pub created_at: OffsetDateTime,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingDeliveryCheck { pub sender_key: String, pub list_id: Option<String>, pub mailbox_id: MailboxId, pub unsubscribed_at: OffsetDateTime }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AchievementRecord { pub achievement_id: String, pub unlocked_at: OffsetDateTime }
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    pub triaged: u64,                 // keep, reject and file swipes, minus undone (ST-02 AC1)
    pub cleared: u64,                 // rejects plus files (T-109 "cleared")
    pub senders_unsubscribed: u64,    // History outcomes `sent` collected (T-609)
    pub unsubscribes_confirmed: u64,  // T-707
    // T-109 AchievementProgress inputs, all time unless noted
    pub unsubscribes_queued: u64,
    pub senders_silenced: u64,        // reject_list and block_person rules created
    pub years_cleared: u64,
    pub categories_created: u64,
    pub people_blocked: u64,
    pub round_unsubscribes: u64,      // reset when round_session changes (T-109 Algorithm 4)
    pub round_session: Option<Uuid>,  // session_record_id the round count belongs to
}

impl UserState {
    pub fn trim(&mut self, now: OffsetDateTime);            // drops History older than HISTORY_RETENTION_DAYS, recent swipes beyond RECENT_SWIPES_MAX
    pub fn rule(&self, id: &RuleId) -> Option<&StoredRule>;
    pub fn category(&self, id: &CategoryId) -> Option<&Category>;
    pub fn category_by_name(&self, name: &str) -> Option<&Category>; // case-insensitive, trimmed
    pub fn push_history(&mut self, e: HistoryEntry) -> bool;        // false if entry_id already present
}

// backend/crates/api/src/services/user_state_store.rs
pub const UPDATE_ATTEMPTS: u32 = 3;  // [DEFAULT] three tries covers two racing tabs

pub struct Loaded { pub state: UserState, pub etag: Option<ETag> }

pub struct UserStateStore { /* Arc<AppState> parts: AppFolderStore, KeyService, TokenService, ServerStore, Clock */ }
impl UserStateStore {
    pub async fn load(&self, user: &UserId) -> Result<Loaded, ApiError>;
    /// Re-runs `f` on a fresh copy after every ETag conflict. `f` must be pure: no provider or store calls.
    pub async fn update<R, F>(&self, user: &UserId, f: F) -> Result<R, ApiError>
        where F: Fn(&mut UserState) -> R + Send, R: Send;
}
```

If T-104 or T-108 merged different names for `SortRule` or the Feed cursor, keep `StoredRule` wrapping their `SortRule`, and keep `MailboxPosition` here (T-108's pure functions take it as input; adapt their signatures in this pull request only if they need a field listed above).

## Algorithm

`load(user)`:

1. Find the user's primary mailbox (`mailboxes().by_user`, `is_primary`). None: `500 internal_error` (a user always has one).
2. `TokenService::mailbox_ctx(primary)`. A token failure maps to `409 sign_in_required` with `mailbox_id`.
3. `AppFolderStore::read(ctx)`. `None`: return `UserState::default()` with `version = USER_STATE_VERSION` and `etag = None` (the user deleted the file, or first use).
4. Load the user's `wrapped_data_key`; `KeyService::open(user, wrapped, Aad { user, scope: "app_folder", field: "user_state" }, bytes)`. The AAD names the user, not the mailbox, so the file can move between Drives as bytes (T-601b).
5. `serde_json::from_slice`. A decrypt or parse failure is `500 internal_error` and a security event `app_folder_unreadable`; never overwrite the file in this case.
6. A `version` greater than `USER_STATE_VERSION`: `500 internal_error` (never downgrade).

`update(user, f)`:

1. Loop up to `UPDATE_ATTEMPTS` times:
   1. `load`.
   2. Run `f(&mut state)`; then `state.trim(clock.now())`.
   3. Serialise, seal with the same AAD, `AppFolderStore::write(ctx, bytes, etag.as_ref())`.
   4. `Ok` returns `f`'s result. `AppFolderError::Conflict` goes round the loop again. Any other error maps to `503 provider_unavailable` (rate limit or transient) or `502 provider_error`.
2. After the last conflict: `503 provider_unavailable` with `retry_after_seconds = 1`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SR-01 AC5 | Rules live in the encrypted app folder file, not in any server store |
| FD-03 AC3 | Feed positions are stored in the app folder file and survive a new session |
| GM-06 AC2 | Only the achievement ID and unlock date are stored, in the app folder |
| INV-1 | No user state field reaches Firestore; the server store holds no subject, body or snippet |
| INV-4 | A History entry appended through `update` is never lost to a concurrent write |

## Tests that must pass

- `sr_01_ac5_rules_written_only_to_app_folder` (service integration: after an update with a rule, the in-memory `ServerStore` holds no rule data and the Drive fake holds ciphertext that does not contain the sender string)
- `fd_03_ac3_positions_survive_new_session` (service integration)
- `gm_06_ac2_achievement_record_has_two_fields` (unit, `domain`: serialised JSON of `AchievementRecord` has exactly `achievement_id` and `unlocked_at`)
- `inv_1_user_state_never_in_server_store` (service integration: scan the in-memory store after updates for any `UserState` field value)
- `inv_4_concurrent_updates_keep_both_history_entries` (service integration: two `update` calls race through the Drive fake's ETag conflict; both entries present)
- `inv_4_update_gives_up_after_three_conflicts` (service integration: fake always conflicts; `503 provider_unavailable`)
- `user_state_missing_file_starts_empty` (service integration)
- `user_state_undecryptable_file_not_overwritten` (service integration: corrupt bytes; `500`, no write recorded)
- `user_state_history_trimmed_at_twelve_months` (unit, `domain`, virtual time values)
- `user_state_push_history_is_idempotent` (unit, `domain`)
- `user_state_round_trip` (property, `domain`: any generated `UserState` serialises and parses back equal)

## Edge cases and traps

- The closure passed to `update` can run up to three times. Never call a provider, the store or the scheduler inside it; do side effects before or after.
- Pass the ETag from the same `load` to `write`. Writing with `if_match = None` when a file exists is a lost update.
- `deny_unknown_fields` on `UserState` means a newer file fails to parse; that is intended (step 6 guard), but every new field added later needs `#[serde(default)]`.
- Never log the state, a sender key or a display name. `UserState` must not derive a `Debug` that a logger could pick up in production paths; if `Debug` is derived for tests, never pass it to `tracing`.
- Use `Clock` for `now`; no `SystemTime::now()`.
- One file per user, in the primary mailbox's Drive only. Never read or write other mailboxes' Drives here.

## Out of scope

- Moving the file on disconnect: T-601b.
- Deleting the file on account deletion: T-803.
- Rebuilding categories from existing labels after the user deletes the file: not in v1 (`[DEFAULT]` start empty).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The pull request lists `UserState` fields so later tasks can cite them.
