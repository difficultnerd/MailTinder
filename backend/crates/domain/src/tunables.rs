//! `S2` tunables and other adjustable numbers. Services load overrides from
//! config; tests use `Default`.

use std::time::Duration;

/// Tunable numbers from the S2 glossary and other `[TUNABLE]` sources.
#[derive(Clone, Debug, PartialEq)]
pub struct Tunables {
    /// 5 minutes (`UNSUB_DELAY`).
    pub unsub_delay: Duration,
    /// 3.
    pub personal_block_threshold: u32,
    /// 90 days.
    pub personal_block_window: Duration,
    /// 90 days (`PB-01 AC3`).
    pub block_prompt_decline: Duration,
    /// 2 (`SKIP_MAX_RETURNS`, not tunable in `S2` but kept here).
    pub skip_max_returns: u8,
    /// 30 days.
    pub needs_attention_ttl: Duration,
    /// 1 hour after `due_at`.
    pub job_ttl: Duration,
    /// 30 days (`S3` terminal job).
    pub job_outcome_retention: Duration,
    /// 4 (`S3`).
    pub cloud_tasks_max_attempts: u32,
    /// 7 days.
    pub invite_ttl: Duration,
    /// 5 minutes.
    pub step_up_window: Duration,
    /// 300 (`FD-01 AC2`).
    pub preview_max_chars: usize,
    /// 5 (`FL-04 AC1`).
    pub keep_learning_threshold: u32,
    /// 3 (`FL-03 AC1`).
    pub filing_learned_threshold: u32,
    /// 20 (`GM-08 AC1`).
    pub boss_min_seen: u32,
    /// 5 (`GM-08 AC1`).
    pub boss_top_n: usize,
    /// 90 days (`GM-05 AC1`).
    pub mail_stopped_window: Duration,
    /// 50 (`GM-03 AC1`).
    pub round_swipes: u32,
}

impl Default for Tunables {
    fn default() -> Self {
        Self {
            unsub_delay: Duration::from_secs(5 * 60),
            personal_block_threshold: 3,
            personal_block_window: Duration::from_secs(90 * 24 * 60 * 60),
            block_prompt_decline: Duration::from_secs(90 * 24 * 60 * 60),
            skip_max_returns: 2,
            needs_attention_ttl: Duration::from_secs(30 * 24 * 60 * 60),
            job_ttl: Duration::from_secs(60 * 60),
            job_outcome_retention: Duration::from_secs(30 * 24 * 60 * 60),
            cloud_tasks_max_attempts: 4,
            invite_ttl: Duration::from_secs(7 * 24 * 60 * 60),
            step_up_window: Duration::from_secs(5 * 60),
            preview_max_chars: 300,
            keep_learning_threshold: 5,
            filing_learned_threshold: 3,
            boss_min_seen: 20,
            boss_top_n: 5,
            mail_stopped_window: Duration::from_secs(90 * 24 * 60 * 60),
            round_swipes: 50,
        }
    }
}
