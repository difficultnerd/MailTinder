//! Per-sender counter methods behind the block and keep-learning prompts
//! (T-104).

use time::OffsetDateTime;

use crate::ids::CategoryId;
use crate::sender::{SenderStats, REJECTS_KEPT};
use crate::tunables::Tunables;

/// What a reject should do about the block prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockPrompt {
    /// Ask "Block <name>?".
    Ask,
    /// Not enough counted personal rejects yet.
    NotYet,
    /// The user declined recently; do not ask.
    Declined,
    /// The reject was not authenticated; nothing was counted.
    NotCounted,
}

impl SenderStats {
    /// A keep increments the sender's keep count (SW-01 AC2).
    pub fn record_keep(&mut self) {
        self.keeps += 1;
    }

    /// Counts an authenticated reject (SR-01 AC6, PB-01 AC4) and says whether
    /// to ask "Block <name>?". `personal` is true when the rejected message's
    /// class is `Personal`.
    pub fn record_reject(
        &mut self,
        authenticated: bool,
        personal: bool,
        now: OffsetDateTime,
        t: &Tunables,
    ) -> BlockPrompt {
        if !authenticated {
            return BlockPrompt::NotCounted;
        }
        self.rejects_counted.push(now);
        if self.rejects_counted.len() > REJECTS_KEPT {
            let excess = self.rejects_counted.len() - REJECTS_KEPT;
            self.rejects_counted.drain(..excess);
        }
        if !personal {
            return BlockPrompt::NotYet;
        }
        if self
            .block_prompt_declined_until
            .is_some_and(|until| until > now)
        {
            return BlockPrompt::Declined;
        }
        if self.rejects_within(now, t.personal_block_window) >= t.personal_block_threshold {
            BlockPrompt::Ask
        } else {
            BlockPrompt::NotYet
        }
    }

    /// Remove the last counted reject equal to `at`, if any (for undo, T-105b).
    pub fn unrecord_last_reject(&mut self, at: OffsetDateTime) {
        if let Some(idx) = self.rejects_counted.iter().rposition(|r| *r == at) {
            self.rejects_counted.remove(idx);
        }
    }

    /// A file increments the category count and sets `last_filed`.
    pub fn record_file(&mut self, category: CategoryId) {
        *self.files.entry(category.0).or_insert(0) += 1;
        self.last_filed = Some(category);
    }

    /// Record a decline of the block prompt (PB-01 AC3).
    pub fn decline_block_prompt(&mut self, now: OffsetDateTime, t: &Tunables) {
        let decline =
            time::Duration::try_from(t.block_prompt_decline).unwrap_or(time::Duration::ZERO);
        self.block_prompt_declined_until = Some(now + decline);
    }

    /// Count counted rejects within `window` before `now`.
    pub fn rejects_within(&self, now: OffsetDateTime, window: std::time::Duration) -> u32 {
        let window = time::Duration::try_from(window).unwrap_or(time::Duration::ZERO);
        u32::try_from(
            self.rejects_counted
                .iter()
                .filter(|at| **at <= now && now - **at < window)
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    /// FL-04 AC1: kept at least `keep_learning_threshold` times, never
    /// rejected, and no `File` rule for the sender yet.
    pub fn keep_prompt_due(&self, has_file_rule: bool, t: &Tunables) -> bool {
        self.keeps >= t.keep_learning_threshold && self.rejects_counted.is_empty() && !has_file_rule
    }
}
