//! The swipe planner (T-105a): turns one swipe into a pure `SwipePlan`.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::class::{Classification, MessageClass, SwipeAction};
use crate::header_rules::{HeaderRules, UnsubscribeRoute};
use crate::ids::{CategoryId, JobId, RuleId, UserId};
use crate::message::{MailtoTarget, MessageMeta};
use crate::rules::SortRule;
use crate::sender::SenderStats;
use crate::sender_stats::BlockPrompt;
use crate::tunables::Tunables;
use crate::unsubscribe_job::JobMethod;

/// Everything the planner needs. `badge` is display-only and MUST NOT
/// influence the plan (GUARD-3).
pub struct SwipeInput<'a> {
    pub action: SwipeAction,
    /// Freshly re-read from the provider (S7 5.5).
    pub meta: &'a MessageMeta,
    /// What the card showed; display only.
    pub badge: &'a Classification,
    /// Before this swipe.
    pub stats: &'a SenderStats,
    pub ids: SwipeIds,
    /// Idempotency-Key.
    pub source_swipe: Uuid,
    pub now: OffsetDateTime,
    pub tunables: &'a Tunables,
}

/// The IDs a swipe may create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipeIds {
    pub job_id: JobId,
    pub rule_id: RuleId,
}

/// The mailbox change a swipe plans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailboxChange {
    None,
    Trash,
    ReportSpamAndTrash,
    ApplyCategory { category: CategoryId },
}

/// A queued unsubscribe job. Personal data; Debug prints the method only.
#[derive(Clone, PartialEq, Eq)]
pub struct UnsubscribePlan {
    pub job_id: JobId,
    pub method: JobMethod,
    pub target: UnsubscribeTarget,
    pub due_at: OffsetDateTime,
}

impl fmt::Debug for UnsubscribePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnsubscribePlan")
            .field("job_id", &self.job_id)
            .field("method", &self.method)
            .field("target", &self.target)
            .field("due_at", &self.due_at)
            .finish()
    }
}

/// The target of an unsubscribe job. Personal data; Debug prints the variant.
#[derive(Clone, PartialEq, Eq)]
pub enum UnsubscribeTarget {
    OneClick(Url),
    Mailto(MailtoTarget),
}

impl fmt::Debug for UnsubscribeTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsubscribeTarget::OneClick(_) => f.write_str("OneClick([redacted])"),
            UnsubscribeTarget::Mailto(_) => f.write_str("Mailto([redacted])"),
        }
    }
}

/// A Needs Attention "open unsubscribe page" item. Personal data; Debug prints
/// `link.is_some()` only.
#[derive(Clone, PartialEq, Eq)]
pub struct ManualUnsubscribePlan {
    pub link: Option<Url>,
}

impl fmt::Debug for ManualUnsubscribePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManualUnsubscribePlan")
            .field("link", &self.link.is_some())
            .finish()
    }
}

/// The S7 API-SW-1 outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeOutcome {
    Kept,
    Skipped,
    TrashedUnsubscribeQueued,
    TrashedUnsubscribeManual,
    TrashedListNoUnsubscribe,
    Trashed,
    ReportedSpam,
    Filed,
}

/// The full plan for one swipe.
#[derive(Clone, Debug, PartialEq)]
pub struct SwipePlan {
    /// The `HeaderRules` class from the facts (the action class).
    pub class: MessageClass,
    pub change: MailboxChange,
    pub unsubscribe: Option<UnsubscribePlan>,
    pub manual_unsubscribe: Option<ManualUnsubscribePlan>,
    pub rule: Option<SortRule>,
    /// Stats with this swipe applied.
    pub stats_after: SenderStats,
    /// PB-01 AC1.
    pub block_prompt: bool,
    pub outcome: SwipeOutcome,
}

/// Plan one swipe. Pure: no clock (uses `input.now`), no randomness, no I/O.
#[allow(clippy::too_many_lines)]
pub fn plan_swipe(input: &SwipeInput<'_>) -> SwipePlan {
    let class = HeaderRules::classify(&input.meta.facts, &input.meta.sender).class;
    let route = HeaderRules::unsubscribe_route(&input.meta.facts);
    let mut stats_after = input.stats.clone();
    let mut block_prompt = false;
    let mut unsubscribe = None;
    let mut manual_unsubscribe = None;
    let mut rule = None;

    let (change, outcome) = match input.action {
        SwipeAction::Keep => {
            stats_after.record_keep();
            (MailboxChange::None, SwipeOutcome::Kept)
        }
        SwipeAction::Skip => (MailboxChange::None, SwipeOutcome::Skipped),
        SwipeAction::File { category } => {
            stats_after.record_file(category);
            (
                MailboxChange::ApplyCategory { category },
                SwipeOutcome::Filed,
            )
        }
        SwipeAction::Reject => match class {
            MessageClass::List => {
                let (unsub, manual, outcome) = match route {
                    UnsubscribeRoute::OneClick(url) => (
                        Some(UnsubscribePlan {
                            job_id: input.ids.job_id,
                            method: JobMethod::OneClick,
                            target: UnsubscribeTarget::OneClick(url),
                            due_at: input.now + input.tunables.unsub_delay,
                        }),
                        None,
                        SwipeOutcome::TrashedUnsubscribeQueued,
                    ),
                    UnsubscribeRoute::Mailto(target) => (
                        Some(UnsubscribePlan {
                            job_id: input.ids.job_id,
                            method: JobMethod::Mailto,
                            target: UnsubscribeTarget::Mailto(target),
                            due_at: input.now + input.tunables.unsub_delay,
                        }),
                        None,
                        SwipeOutcome::TrashedUnsubscribeQueued,
                    ),
                    UnsubscribeRoute::ManualLink(link) => (
                        None,
                        Some(ManualUnsubscribePlan { link }),
                        SwipeOutcome::TrashedUnsubscribeManual,
                    ),
                    UnsubscribeRoute::None => (None, None, SwipeOutcome::Trashed),
                };
                unsubscribe = unsub;
                manual_unsubscribe = manual;
                rule = SortRule::reject_list_for(
                    input.meta,
                    input.ids.rule_id,
                    input.now,
                    Some(input.source_swipe),
                );
                stats_after.record_reject(
                    input.meta.facts.from_authenticated,
                    false,
                    input.now,
                    input.tunables,
                );
                (MailboxChange::Trash, outcome)
            }
            MessageClass::BulkNoHeader => {
                rule = SortRule::reject_list_for(
                    input.meta,
                    input.ids.rule_id,
                    input.now,
                    Some(input.source_swipe),
                );
                stats_after.record_reject(
                    input.meta.facts.from_authenticated,
                    false,
                    input.now,
                    input.tunables,
                );
                (MailboxChange::Trash, SwipeOutcome::TrashedListNoUnsubscribe)
            }
            MessageClass::Notice => {
                stats_after.record_reject(
                    input.meta.facts.from_authenticated,
                    false,
                    input.now,
                    input.tunables,
                );
                (MailboxChange::Trash, SwipeOutcome::Trashed)
            }
            MessageClass::Personal => {
                block_prompt = stats_after.record_reject(
                    input.meta.facts.from_authenticated,
                    true,
                    input.now,
                    input.tunables,
                ) == BlockPrompt::Ask;
                (MailboxChange::Trash, SwipeOutcome::Trashed)
            }
            MessageClass::Suspect => (
                MailboxChange::ReportSpamAndTrash,
                SwipeOutcome::ReportedSpam,
            ),
        },
    };

    SwipePlan {
        class,
        change,
        unsubscribe,
        manual_unsubscribe,
        rule,
        stats_after,
        block_prompt,
        outcome,
    }
}

/// Derive the job and rule IDs for a swipe from the user and Idempotency-Key
/// (S7 5.5): UUID v5 of the user ID and the key, with distinct names.
pub fn derive_swipe_ids(user: &UserId, idempotency_key: Uuid) -> SwipeIds {
    let job_id = JobId(Uuid::new_v5(
        &user.0,
        format!("job:{idempotency_key}").as_bytes(),
    ));
    let rule_id = RuleId(Uuid::new_v5(
        &user.0,
        format!("rule:{idempotency_key}").as_bytes(),
    ));
    SwipeIds { job_id, rule_id }
}
