//! Undo: the record of one swipe, the plan that reverses it, and a LIFO stack
//! (T-105b).

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::class::SwipeAction;
use crate::ids::{JobId, MailboxId, MessageId, RuleId};
use crate::message::{LabelSet, MessageMeta};
use crate::sender::SenderKey;
use crate::swipe::{SwipeOutcome, SwipePlan};
use crate::SenderStats;

/// The record of one swipe; the payload T-604 seals into the undo token.
/// Personal data; Debug redacts the message ID and sender.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SwipeRecord {
    pub mailbox: MailboxId,
    pub message: MessageId,
    pub sender: SenderKey,
    pub action: SwipeAction,
    pub outcome: SwipeOutcome,
    /// Exact provider labels before the swipe.
    pub previous_labels: LabelSet,
    pub job_id: Option<JobId>,
    pub rule_id: Option<RuleId>,
    /// Set when the swipe added to `rejects_counted`.
    pub counted_reject_at: Option<OffsetDateTime>,
    pub at: OffsetDateTime,
}

impl fmt::Debug for SwipeRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwipeRecord")
            .field("mailbox", &self.mailbox)
            .field("message", &"MessageId([redacted])")
            .field("sender", &"SenderKey([redacted])")
            .field("action", &self.action)
            .field("outcome", &self.outcome)
            .field("previous_labels", &self.previous_labels)
            .field("job_id", &self.job_id)
            .field("rule_id", &self.rule_id)
            .field("counted_reject_at", &self.counted_reject_at)
            .field("at", &self.at)
            .finish()
    }
}

impl SwipeRecord {
    /// Build the record from a plan and the message it acted on.
    pub fn from_plan(
        plan: &SwipePlan,
        meta: &MessageMeta,
        action: SwipeAction,
        at: OffsetDateTime,
    ) -> SwipeRecord {
        // A counted reject pushes `at` as the last entry of `rejects_counted`.
        let counted_reject_at = plan
            .stats_after
            .rejects_counted
            .last()
            .is_some_and(|last| *last == at)
            .then_some(at);
        SwipeRecord {
            mailbox: meta.mailbox,
            message: meta.id.clone(),
            sender: meta.sender.clone(),
            action,
            outcome: plan.outcome,
            previous_labels: meta.labels.clone(),
            job_id: plan.unsubscribe.as_ref().map(|u| u.job_id),
            rule_id: plan.rule.as_ref().map(|r| r.rule_id),
            counted_reject_at,
            at,
        }
    }
}

/// The plan that reverses one swipe.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoPlan {
    /// Some for every swipe that changed the mailbox; exact previous set.
    pub restore_labels: Option<LabelSet>,
    pub cancel_job: Option<JobId>,
    pub remove_rule: Option<RuleId>,
    /// `SW-05 AC4a`.
    pub spam_report_not_recalled: bool,
}

/// Plan the reversal of a swipe.
pub fn plan_undo(record: &SwipeRecord) -> UndoPlan {
    let changed_mailbox = !matches!(record.outcome, SwipeOutcome::Kept | SwipeOutcome::Skipped);
    UndoPlan {
        restore_labels: changed_mailbox.then(|| record.previous_labels.clone()),
        cancel_job: record.job_id,
        remove_rule: record.rule_id,
        spam_report_not_recalled: record.outcome == SwipeOutcome::ReportedSpam,
    }
}

/// Reverse the stats a swipe applied. Never underflows.
pub fn reverse_stats(record: &SwipeRecord, stats: &mut SenderStats) {
    match record.action {
        SwipeAction::Keep => {
            stats.keeps = stats.keeps.saturating_sub(1);
        }
        SwipeAction::File { category } => {
            if let Some(count) = stats.files.get_mut(&category.0) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    stats.files.remove(&category.0);
                }
            }
            if stats.last_filed == Some(category) {
                stats.last_filed = None;
            }
        }
        SwipeAction::Reject => {
            if let Some(at) = record.counted_reject_at {
                stats.unrecord_last_reject(at);
            }
        }
        SwipeAction::Skip => {}
    }
}

/// The outcome of cancelling a queued unsubscribe job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobCancelOutcome {
    NoJob,
    Cancelled,
    AlreadySent,
    AlreadyFinishedNotSent,
}

/// The S7 API-SW-2 response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UndoResponse {
    pub restored: bool,
    pub unsubscribe_already_sent: bool,
}

/// Map a job-cancel outcome to the response. `restored` is always true here;
/// the caller only calls this after the labels were restored.
pub fn undo_response(cancel: JobCancelOutcome) -> UndoResponse {
    UndoResponse {
        restored: true,
        unsubscribe_already_sent: cancel == JobCancelOutcome::AlreadySent,
    }
}

/// A plain LIFO stack (SW-05 AC4: one swipe per tap, back through every swipe).
#[derive(Clone, Debug, Default)]
pub struct UndoStack<T> {
    items: Vec<T>,
}

impl<T> UndoStack<T> {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
