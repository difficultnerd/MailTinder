//! Sort rules: reject, block and filing matching (T-104).

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ids::{CategoryId, RuleId};
use crate::message::MessageMeta;
use crate::sender::SenderKey;

/// What a rule does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    RejectList,
    BlockPerson,
    File,
}

/// The matcher of a rule. Personal data; Debug redacts the sender.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMatch {
    pub sender: SenderKey,
    pub list_id: Option<String>,
    pub feedback_id: Option<String>,
}

impl fmt::Debug for RuleMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuleMatch")
            .field("sender", &"SenderKey([redacted])")
            .field("list_id", &self.list_id.is_some())
            .field("feedback_id", &self.feedback_id.is_some())
            .finish()
    }
}

/// A stored sort rule. Personal data; Debug redacts the matcher.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SortRule {
    pub rule_id: RuleId,
    pub kind: RuleKind,
    #[serde(rename = "match")]
    pub matcher: RuleMatch,
    /// Some only for `File`.
    pub category: Option<CategoryId>,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    /// The swipe's Idempotency-Key, when a swipe created it.
    pub source_swipe: Option<Uuid>,
}

impl fmt::Debug for SortRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SortRule")
            .field("rule_id", &self.rule_id)
            .field("kind", &self.kind)
            .field("matcher", &self.matcher)
            .field("category", &self.category)
            .field("enabled", &self.enabled)
            .field("created_at", &self.created_at)
            .field("source_swipe", &self.source_swipe)
            .finish()
    }
}

/// The action a matched rule prescribes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleAction {
    Trash {
        rule_id: RuleId,
    },
    File {
        rule_id: RuleId,
        category: CategoryId,
    },
}

impl SortRule {
    /// Whether this enabled rule matches the message.
    pub fn matches(&self, meta: &MessageMeta) -> bool {
        if !self.enabled {
            return false;
        }
        if meta.sender != self.matcher.sender {
            return false;
        }
        match self.kind {
            RuleKind::RejectList => {
                if let Some(id) = &self.matcher.list_id {
                    meta.facts.list_id.as_deref() == Some(id.as_str())
                } else {
                    if !meta.facts.list_unsubscribe_present {
                        return false;
                    }
                    if let Some(f) = &self.matcher.feedback_id {
                        meta.facts.feedback_id.as_deref() == Some(f.as_str())
                    } else {
                        true
                    }
                }
            }
            RuleKind::BlockPerson | RuleKind::File => true,
        }
    }

    /// The action this rule prescribes; `None` only for a `File` rule missing
    /// its category.
    pub fn action(&self) -> Option<RuleAction> {
        match self.kind {
            RuleKind::RejectList | RuleKind::BlockPerson => Some(RuleAction::Trash {
                rule_id: self.rule_id,
            }),
            RuleKind::File => self.category.map(|category| RuleAction::File {
                rule_id: self.rule_id,
                category,
            }),
        }
    }

    /// Build a `RejectList` rule for a message, or `None` when the message is
    /// not authenticated (SR-01 AC6).
    pub fn reject_list_for(
        meta: &MessageMeta,
        rule_id: RuleId,
        now: OffsetDateTime,
        source: Option<Uuid>,
    ) -> Option<SortRule> {
        if !meta.facts.from_authenticated {
            return None;
        }
        let list_id = meta.facts.list_id.clone();
        let feedback_id = if list_id.is_none() {
            meta.facts.feedback_id.clone()
        } else {
            None
        };
        Some(SortRule {
            rule_id,
            kind: RuleKind::RejectList,
            matcher: RuleMatch {
                sender: meta.sender.clone(),
                list_id,
                feedback_id,
            },
            category: None,
            enabled: true,
            created_at: now,
            source_swipe: source,
        })
    }

    /// Build a `BlockPerson` rule for a sender.
    pub fn block_person_for(
        sender: &SenderKey,
        rule_id: RuleId,
        now: OffsetDateTime,
        source: Option<Uuid>,
    ) -> SortRule {
        SortRule {
            rule_id,
            kind: RuleKind::BlockPerson,
            matcher: RuleMatch {
                sender: sender.clone(),
                list_id: None,
                feedback_id: None,
            },
            category: None,
            enabled: true,
            created_at: now,
            source_swipe: source,
        }
    }

    /// Build a `File` rule for a sender and category.
    pub fn file_for(
        sender: &SenderKey,
        category: CategoryId,
        rule_id: RuleId,
        now: OffsetDateTime,
    ) -> SortRule {
        SortRule {
            rule_id,
            kind: RuleKind::File,
            matcher: RuleMatch {
                sender: sender.clone(),
                list_id: None,
                feedback_id: None,
            },
            category: Some(category),
            enabled: true,
            created_at: now,
            source_swipe: None,
        }
    }
}

/// The first enabled matching rule, preferring trash (`RejectList` and
/// `BlockPerson`) over `File`; ties by older `created_at`, then `rule_id`.
pub fn first_match<'a>(rules: &'a [SortRule], meta: &MessageMeta) -> Option<&'a SortRule> {
    rules.iter().filter(|r| r.matches(meta)).min_by(|a, b| {
        let a_trash = matches!(a.kind, RuleKind::RejectList | RuleKind::BlockPerson);
        let b_trash = matches!(b.kind, RuleKind::RejectList | RuleKind::BlockPerson);
        b_trash
            .cmp(&a_trash)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    })
}
