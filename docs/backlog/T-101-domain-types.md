# T-101: Domain identifiers, mailbox and message types

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 400 lines of code plus tests | T-001 |

**Read only these spec sections:** S3 "Entities" (both tables), "Message classes", the `sender_key` normalisation paragraph, "Invariants" (`docs/specs/S3-domain-model.md`); S2 "Glossary of fixed values" and AU-03 AC7, SR-01 AC1a (`docs/specs/S2-v1-acceptance-criteria.md`); S5 "Logs and telemetry" (what may never be logged). `docs/backlog/CONVENTIONS.md` "Core domain types" and "Rust conventions". Nothing else is needed.

## Goal

The `domain` crate gets the shared vocabulary every later task uses: typed IDs, `Provider`, the mailbox identity, `SenderKey` with relay unwrapping, `LabelSet`, `HeaderFacts`, `UnsubscribeOptions`, `MailtoTarget`, `MessageMeta`, `MessageClass`, `Classification`, `SwipeAction`, `SenderStats` and the S2 tunables. Types that carry personal data print `[redacted]` in `Debug` so a stray `{:?}` cannot leak them. No I/O, no provider types (INV-7).

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/domain/src/lib.rs` | Module declarations and re-exports of every public item below |
| Create | `backend/crates/domain/src/ids.rs` | `UserId`, `MailboxId`, `MessageId`, `JobId`, `RuleId`, `CategoryId`, `ClassifierId` |
| Create | `backend/crates/domain/src/mailbox.rs` | `Provider`, `ProviderSubjectId`, `MailboxIdentity`, `MailboxStatus`, `Mailbox` |
| Create | `backend/crates/domain/src/sender.rs` | `SenderKey`, relay unwrapping, `SenderStats` |
| Create | `backend/crates/domain/src/message.rs` | `LabelSet`, `HeaderFacts`, `UnsubscribeOptions`, `MailtoTarget`, `MessageMeta` |
| Create | `backend/crates/domain/src/class.rs` | `MessageClass`, `Classification`, `SwipeAction` |
| Create | `backend/crates/domain/src/tunables.rs` | `Tunables` with S2 defaults |
| Create | `backend/crates/domain/src/error.rs` | `DomainError` |

## Types and signatures

```rust
// ids.rs. All derive Clone, Copy (Uuid ones), PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize
// with #[serde(transparent)]. Uuid IDs also derive Debug and implement Display (opaque, not personal).
pub struct UserId(pub Uuid);
pub struct MailboxId(pub Uuid);
pub struct JobId(pub Uuid);
pub struct RuleId(pub Uuid);
pub struct CategoryId(pub Uuid);
pub struct MessageId(String);            // provider message ID; Debug prints MessageId([redacted]); no Display
impl MessageId {
    pub fn new(raw: impl Into<String>) -> Result<Self, DomainError>; // 1..=256 chars, no control chars
    pub fn as_str(&self) -> &str;
}
pub struct ClassifierId(String);         // "header_rules@1", "gemini@flash-lite", "jev@1.13.0"; Debug and Display allowed
impl ClassifierId { pub fn new(raw: &str) -> Result<Self, DomainError>; pub fn as_str(&self) -> &str; }
pub const HEADER_RULES_ID: &str = "header_rules@1";

// mailbox.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider { Gmail }               // Graph in v2; never match with `_`
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSubjectId(String);     // Google `sub`; Debug redacted
impl ProviderSubjectId { pub fn new(raw: impl Into<String>) -> Result<Self, DomainError>; pub fn as_str(&self) -> &str; } // 1..=255, no control chars
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MailboxIdentity { pub provider: Provider, pub subject: ProviderSubjectId } // AU-03 AC7: never an email
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxStatus { Connected, NeedsSignIn, ConsentBlocked }  // transitions: T-107; ConsentBlocked is v2 only
#[derive(Clone, Debug, PartialEq)]
pub struct Mailbox {
    pub id: MailboxId, pub user: UserId, pub identity: MailboxIdentity,
    pub status: MailboxStatus, pub linked_at: OffsetDateTime, pub is_primary: bool,
}
impl Mailbox { pub fn owned_by(&self, user: &UserId) -> bool; }   // INV-3

// sender.rs
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SenderKey(String);             // Debug prints SenderKey([redacted]); no Display
impl SenderKey {
    pub fn from_address(address: &str) -> SenderKey; // trim, lower-case, unwrap known relays (SR-01 AC1a)
    pub fn as_str(&self) -> &str;
    pub fn domain(&self) -> &str;                    // after the last '@', or "" when none
    pub fn is_noreply(&self) -> bool;                // local part matches the NOREPLY rule below
    pub fn is_empty(&self) -> bool;
}
pub const RELAY_DOMAINS: [&str; 1] = ["icloud.com"];  // [DEFAULT] iCloud Hide My Email (S3), extendable

// Shape agreed with T-602b, which stores it in the user state file. Fields in this order.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SenderStats {
    pub display: String,                          // last seen display name, plain text (C2: Debug is derived; the
                                                  // privacy rule does not match `display`; never log a SenderStats)
    pub seen: u32,                                // cards shown (GM-08)
    pub keeps: u32,
    pub rejects_counted: Vec<OffsetDateTime>,     // authenticated rejects only, oldest first, keep the last 20
    pub files: BTreeMap<Uuid, u32>,               // CategoryId -> times filed
    pub last_filed: Option<CategoryId>,
    pub last_seen: Option<OffsetDateTime>,
    pub block_prompt_declined_until: Option<OffsetDateTime>,
    pub boss_defeated: bool,
}
pub const REJECTS_KEPT: usize = 20;

// message.rs
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelSet(BTreeSet<String>);    // exact provider label IDs, opaque to the domain
impl LabelSet {
    pub fn new() -> Self;
    pub fn from_ids<I: IntoIterator<Item = String>>(ids: I) -> Self;
    pub fn contains(&self, id: &str) -> bool;
    pub fn insert(&mut self, id: String) -> bool;
    pub fn remove(&mut self, id: &str) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = &String>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn difference(&self, other: &LabelSet) -> LabelSet;
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailtoTarget { to: String, subject: Option<String>, body: Option<String> }  // Debug redacted
impl MailtoTarget {
    /// Refuses CR or LF in any field, an empty `to`, `to` without exactly one '@', or `to` over 320 chars.
    /// Full RFC 6068 parsing and the cc/bcc refusal live in T-404; this is the last line of defence.
    pub fn new(to: &str, subject: Option<&str>, body: Option<&str>) -> Result<Self, DomainError>;
    pub fn to(&self) -> &str; pub fn subject(&self) -> Option<&str>; pub fn body(&self) -> Option<&str>;
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeOptions {           // only options covered by a passing DKIM signature (T-406); Debug shows presence only
    pub one_click_https: Option<Url>,     // https URI with List-Unsubscribe-Post: List-Unsubscribe=One-Click, both covered
    pub https: Option<Url>,               // web link (https only; a plain http link is dropped, S6 6), covered
    pub mailto: Option<MailtoTarget>,     // covered
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct HeaderFacts {                  // Debug prints booleans and is_some() only
    pub list_unsubscribe: Option<UnsubscribeOptions>, // DKIM-covered options only (T-406); None when none survive
    pub list_unsubscribe_present: bool,   // NEW: a List-Unsubscribe header exists at all, covered or not
    pub list_id: Option<String>,          // normalised by the adapter (T-401 step 6)
    pub feedback_id: Option<String>,
    pub precedence_bulk: bool,
    pub auto_submitted: bool,
    pub from_authenticated: bool,         // DKIM-aligned From or provider auth pass
    pub esp_hint: Option<String>,         // a key from T-401's ESP_HINTS, badge reason only
    pub is_reply_or_thread: bool,
    pub reply_to_mismatch: bool,          // NEW: Reply-To domain differs from From domain
    pub display_name_spoof: bool,         // NEW: display name contains an address or domain other than the From domain
}

#[derive(Clone, PartialEq)]
pub struct MessageMeta {                  // Debug: mailbox, internal_date, labels, facts; redacts the rest
    pub mailbox: MailboxId, pub id: MessageId, pub internal_date: OffsetDateTime,
    pub from_display: String, pub from_address: String, pub sender: SenderKey,
    pub subject: String, pub labels: LabelSet, pub facts: HeaderFacts,
}

// class.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageClass { List, BulkNoHeader, Notice, Personal, Suspect }
impl MessageClass { pub const ALL: [MessageClass; 5]; pub fn as_str(self) -> &'static str; }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub class: MessageClass,
    pub bulk_score: u8,                   // 0..=100
    pub bulk_reason: String,              // fixed text, at most 200 chars, never sender data
    pub confidence: Option<f32>,
    pub probabilities: Option<[f32; 5]>,  // in MessageClass::ALL order
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum SwipeAction { Keep, Skip, Reject, File { category: CategoryId } }

// tunables.rs: S2 glossary and other [TUNABLE] numbers. Services load overrides from config; tests use Default.
#[derive(Clone, Debug, PartialEq)]
pub struct Tunables {
    pub unsub_delay: Duration,              // 5 minutes (UNSUB_DELAY)
    pub personal_block_threshold: u32,      // 3
    pub personal_block_window: Duration,    // 90 days
    pub block_prompt_decline: Duration,     // 90 days (PB-01 AC3)
    pub skip_max_returns: u8,               // 2 (SKIP_MAX_RETURNS, not tunable in S2 but kept here)
    pub needs_attention_ttl: Duration,      // 30 days
    pub job_ttl: Duration,                  // 1 hour after due_at
    pub job_outcome_retention: Duration,    // 30 days (S3 terminal job)
    pub cloud_tasks_max_attempts: u32,      // 4 (S3)
    pub invite_ttl: Duration,               // 7 days
    pub step_up_window: Duration,           // 5 minutes
    pub preview_max_chars: usize,           // 300 (FD-01 AC2)
    pub keep_learning_threshold: u32,       // 5 (FL-04 AC1)
    pub filing_learned_threshold: u32,      // 3 (FL-03 AC1)
    pub boss_min_seen: u32,                 // 20 (GM-08 AC1)
    pub boss_top_n: usize,                  // 5 (GM-08 AC1)
    pub mail_stopped_window: Duration,      // 90 days (GM-05 AC1)
    pub round_swipes: u32,                  // 50 (GM-03 AC1)
}
impl Default for Tunables { /* the values above */ }

// error.rs
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("invalid identifier")] InvalidId,
    #[error("invalid mailto target")] InvalidMailto,
    #[error("transition not allowed")] TransitionNotAllowed,
    #[error("invalid value")] InvalidValue,
}
```

## Algorithm

1. **IDs.** Uuid newtypes are plain. `MessageId::new` accepts 1 to 256 characters with no `char::is_control`; anything else is `InvalidId`. Implement `Debug` for `MessageId` by hand as `MessageId([redacted])`; do not implement `Display` (S5: message IDs never go in logs).
2. **`SenderKey::from_address(address)`:**
   1. Trim whitespace and one pair of surrounding angle brackets, lower-case (ASCII and Unicode `to_lowercase`).
   2. Relay unwrapping `[DEFAULT]` (S3 names iCloud Hide My Email; the exact format is not in any spec): if the domain is in `RELAY_DOMAINS` and the local part contains `_at_`, split the local part at the **last** `_at_` into `orig_local` and `rest`. `rest` must look like `<domain with dots as underscores>_<suffix>` where `suffix` is the last `_`-separated segment and is 4 or more ASCII alphanumerics. Drop the suffix, turn the remaining underscores into dots, and require the result to contain a dot and only `[a-z0-9.-]`. If every check passes, the key is `orig_local@that_domain`; otherwise keep the relay address unchanged. Example (synthetic): `news_at_shop_example_com_k3j9x2@icloud.com` gives `news@shop.example.com`.
   3. An empty or `@`-less input gives a key equal to the trimmed lower-cased text (T-401 passes `""` for an unparsable From; T-102 treats an empty key as suspect).
3. **`is_noreply`:** the local part, with `-`, `_` and `.` removed, is `noreply`, `donotreply` or starts with `noreply` `[DEFAULT]` (common automated senders; badge reason input only).
4. **Debug by hand** for `SenderKey`, `ProviderSubjectId`, `MailtoTarget`, `UnsubscribeOptions` (`one_click: bool, https: bool, mailto: bool`), `HeaderFacts` (every bool, and `list_id.is_some()`, `feedback_id.is_some()`, `esp_hint`, `list_unsubscribe` as presence) and `MessageMeta` (redact `id`, `from_display`, `from_address`, `sender`, `subject`). Use `f.debug_struct(...)` with the literal `"[redacted]"`.
5. **`MailtoTarget::new`** as documented; trim nothing silently (a value with CR or LF is refused, not cleaned).
6. **`Mailbox::owned_by`** compares `user` (INV-3: a mailbox has exactly one `UserId`, a non-optional field).
7. **`MailboxIdentity`** equality is provider plus subject only. There is no email field anywhere in it (AU-03 AC7).
8. **`Tunables::default()`** uses `time::Duration::minutes(5)`, `Duration::days(90)` and so on. Comment each value with its S2 or S3 source.
9. Re-export everything from `lib.rs` (`pub use ids::*;` and so on) so callers write `domain::SenderKey`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-03 AC7 | A mailbox identity is provider plus provider subject ID, never an email address |
| INV-3 | A mailbox has exactly one owning user and `owned_by` refuses every other user |
| SR-01 AC1a | A sender address from a known forwarding relay is unwrapped to the original sender |
| XC-01 | `Debug` of sender, subject, message ID and unsubscribe targets prints `[redacted]` |

## Tests that must pass

- `au_03_ac7_identity_equal_by_provider_and_subject` (unit)
- `au_03_ac7_identity_has_no_email_field` (unit: build a `MailboxIdentity` with a struct literal of only `provider` and `subject`; it compiles only if no email field exists)
- `inv_3_mailbox_owned_by_one_user_only` (unit)
- `sr_01_ac1a_icloud_relay_unwrapped_to_original_sender` (unit)
- `sr_01_ac1a_relay_with_bad_suffix_kept_as_is` (unit)
- `sr_01_ac1a_non_relay_domain_untouched` (unit)
- `sr_01_ac1a_sender_key_lower_cased_and_trimmed` (property: for any generated `local@domain` built from `[a-zA-Z0-9._-]`, the key is the lower-cased input and `from_address(key) == key`)
- `xc_01_debug_redacts_sender_subject_and_ids` (unit: format a `MessageMeta` with canary values `CANARY-T101-*` and assert no canary appears)
- `xc_01_debug_of_unsubscribe_options_shows_presence_only` (unit)
- `message_id_rejects_empty_long_and_control` (unit)
- `mailto_target_rejects_cr_lf_and_bad_address` (unit)
- `serde_round_trip_ids_actions_and_classes` (unit: `SwipeAction::File` serialises as `{"action":"file","category":"..."}`, `MessageClass::BulkNoHeader` as `"bulk_no_header"`)
- `tunables_default_matches_s2_glossary` (unit)

## Edge cases and traps

- `HeaderFacts`, `UnsubscribeOptions` and `MessageMeta` must not `#[derive(Debug)]`; T-003's Semgrep rule fails a derive on a struct with `subject` or `url`-like fields, and S5 bans them from logs. Implement `Debug` by hand.
- `HeaderFacts` derives `Default` so adapters and tests can write `HeaderFacts { precedence_bulk: true, ..HeaderFacts::default() }`. The three NEW fields are additions to CONVENTIONS; T-401 must fill them (report this in the pull request).
- Never `match` a `Provider` with `_`; there is one variant now and v2 adds one, which must force every match to be reviewed (XC-02).
- No `std::time::SystemTime` or `OffsetDateTime::now_utc()` anywhere in `domain`; every function takes `now` as an argument.
- No `unwrap` or `expect`; Clippy denies them in tests too. Tests return `Result<(), Box<dyn std::error::Error>>`.
- Relay unwrapping is a trust decision: only `RELAY_DOMAINS` are unwrapped, and a malformed relay address is kept as is rather than guessed.
- Use only example domains in tests (`example.com`, `example.org`, `.test`); `icloud.com` appears only as the relay domain constant.
- `Classification` holds `f32`, so it cannot derive `Eq`; compare with `PartialEq`.
- `SenderStats` and the `Tunables` field names are shared with T-602b and later tasks; keep them exactly.
- `serde` derives use `#[serde(rename_all = "snake_case")]` to match S7 wire values.

## Out of scope

- Header rules and badge text: T-102. Guard: T-103. Rule matching and the counters on `SenderStats`: T-104.
- `MailboxStatus` transitions and the invite and Needs Attention types: T-107.
- HTML stripping and `domain::text`: T-402. DKIM checks: T-406.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `domain` still has only the five allowed dependencies (T-001's INV-7 test passes).
