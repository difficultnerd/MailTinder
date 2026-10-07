//! The user app folder file: every per-user fact that lives only in the
//! user's Drive (T-602b). Pure data. Never log any of it (C2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::ids::{CategoryId, MailboxId, RuleId};
use crate::rules::SortRule;
use crate::sender::SenderStats;

pub const USER_STATE_VERSION: u32 = 1;
/// S5 "Trimmed at 12 months" [TUNABLE]
pub const HISTORY_RETENTION_DAYS: i64 = 365;
/// [DEFAULT] enough for one session's retries
pub const RECENT_SWIPES_MAX: usize = 50;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserState {
    pub version: u32,
    pub rules: Vec<StoredRule>,
    pub categories: Vec<Category>,
    /// key: `SenderKey` as string
    pub sender_stats: BTreeMap<String, SenderStats>,
    /// newest last; endpoints sort
    pub history: Vec<HistoryEntry>,
    /// key: `MailboxId`
    pub positions: BTreeMap<Uuid, MailboxPosition>,
    pub skips: SkipState,
    /// idempotent retry of API-SW-1
    pub recent_swipes: Vec<RecentSwipe>,
    /// key: `JobId`
    pub pending_unsubscribes: BTreeMap<Uuid, PendingUnsubscribe>,
    pub pending_delivery_checks: Vec<PendingDeliveryCheck>,
    pub achievements: Vec<AchievementRecord>,
    pub totals: Totals,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredRule {
    pub rule: SortRule,
    pub times_applied: u64,
    /// GM-05; None when the count query failed
    pub yearly_rate: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub category_id: CategoryId,
    /// 1 to 100 chars, unique case-insensitively
    pub name: String,
    /// `MailboxId` -> provider label ID, created lazily
    pub labels: BTreeMap<Uuid, String>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub entry_id: Uuid,
    pub at: OffsetDateTime,
    pub mailbox_id: MailboxId,
    pub sender_display: String,
    pub action: HistoryAction,
    pub outcome: HistoryOutcome,
    pub rule_id: Option<RuleId>,
}

// The spec mandates Debug prints only non-personal fields; the sender display
// name is redacted (C2, privacy-rust-derive-debug-on-sensitive-struct).
impl std::fmt::Debug for HistoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryEntry")
            .field("entry_id", &self.entry_id)
            .field("at", &self.at)
            .field("mailbox_id", &self.mailbox_id)
            .field("sender_display", &"[redacted]")
            .field("action", &self.action)
            .field("outcome", &self.outcome)
            .field("rule_id", &self.rule_id)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction {
    TrashedByRule,
    Unsubscribe,
    Filed,
    FiledByRule,
    Blocked,
    ReportedSpam,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcome {
    Sent,
    NeedsAttention,
    Failed,
    Cancelled,
    Expired,
    Done,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MailboxPosition {
    /// newest `received_at` ever returned as a card
    pub newest_seen: Option<OffsetDateTime>,
    /// "new" means received after this (set at session start)
    pub new_floor: Option<OffsetDateTime>,
    /// paging within "new": received before this
    pub new_ceiling: Option<OffsetDateTime>,
    pub new_done: bool,
    /// backlog: received before this
    pub backlog_ceiling: Option<OffsetDateTime>,
    /// message IDs already returned at the ceiling second
    pub boundary_ids: Vec<String>,
    /// session that set `new_floor`
    pub session_record_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SkipState {
    /// skips reset when the session changes (SW-02 AC2)
    pub session_record_id: Option<Uuid>,
    /// "`mailbox_id`/`message_id`" -> skips this session
    pub counts: BTreeMap<String, u8>,
    /// cards waiting to come back
    pub queue: Vec<SkipReturn>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkipReturn {
    pub mailbox_id: MailboxId,
    pub message_id: String,
    pub after_cards: u32,
}

/// Stored API-SW-1 result minus undo token, plus the undo payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecentSwipe {
    pub swipe_id: Uuid,
    pub at: OffsetDateTime,
    pub result_json: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingUnsubscribe {
    pub mailbox_id: MailboxId,
    pub sender_display: String,
    pub sender_key: String,
    pub list_id: Option<String>,
    pub rule_id: Option<RuleId>,
    pub created_at: OffsetDateTime,
}

// The spec mandates Debug prints only non-personal fields; the sender display
// name and key are redacted (C2, privacy-rust-derive-debug-on-sensitive-struct).
impl std::fmt::Debug for PendingUnsubscribe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingUnsubscribe")
            .field("mailbox_id", &self.mailbox_id)
            .field("sender_display", &"[redacted]")
            .field("sender_key", &"[redacted]")
            .field("list_id", &self.list_id.is_some())
            .field("rule_id", &self.rule_id)
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingDeliveryCheck {
    pub sender_key: String,
    pub list_id: Option<String>,
    pub mailbox_id: MailboxId,
    pub unsubscribed_at: OffsetDateTime,
    #[serde(default)]
    pub mail_seen: bool,
    #[serde(default)]
    pub confirm_counted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AchievementRecord {
    pub achievement_id: String,
    pub unlocked_at: OffsetDateTime,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    /// keep, reject and file swipes, minus undone (ST-02 AC1)
    pub triaged: u64,
    /// rejects plus files
    pub cleared: u64,
    /// History outcomes `sent` collected
    pub senders_unsubscribed: u64,
    pub unsubscribes_confirmed: u64,
    pub unsubscribes_queued: u64,
    /// `reject_list` and `block_person` rules created
    pub senders_silenced: u64,
    pub years_cleared: u64,
    pub categories_created: u64,
    pub people_blocked: u64,
    /// reset when `round_session` changes
    pub round_unsubscribes: u64,
    /// `session_record_id` the round count belongs to
    pub round_session: Option<Uuid>,
}

impl UserState {
    /// Drop History older than `HISTORY_RETENTION_DAYS` and recent swipes
    /// beyond `RECENT_SWIPES_MAX` (oldest first).
    pub fn trim(&mut self, now: OffsetDateTime) {
        let keep = Duration::days(HISTORY_RETENTION_DAYS);
        self.history.retain(|e| now - e.at <= keep);
        if self.recent_swipes.len() > RECENT_SWIPES_MAX {
            self.recent_swipes.sort_by_key(|s| s.at);
            let excess = self.recent_swipes.len() - RECENT_SWIPES_MAX;
            self.recent_swipes.drain(..excess);
        }
    }

    pub fn rule(&self, id: &RuleId) -> Option<&StoredRule> {
        self.rules.iter().find(|r| r.rule.rule_id == *id)
    }

    pub fn category(&self, id: &CategoryId) -> Option<&Category> {
        self.categories.iter().find(|c| c.category_id == *id)
    }

    /// Case-insensitive, whitespace-trimmed.
    pub fn category_by_name(&self, name: &str) -> Option<&Category> {
        let wanted = name.trim().to_lowercase();
        self.categories
            .iter()
            .find(|c| c.name.trim().to_lowercase() == wanted)
    }

    /// False, and no change, when `entry_id` is already present.
    pub fn push_history(&mut self, e: HistoryEntry) -> bool {
        if self.history.iter().any(|h| h.entry_id == e.entry_id) {
            return false;
        }
        self.history.push(e);
        true
    }
}
