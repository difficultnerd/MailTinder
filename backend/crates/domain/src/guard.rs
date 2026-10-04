//! The header guard (T-103): clamps any classifier's answer to what the
//! headers prove, before it can drive the badge.

use crate::class::{Classification, MessageClass};
use crate::header_rules::{HeaderRules, UnsubscribeRoute, PERSONAL_HIGH_CONFIDENCE_MAX_SCORE};
use crate::message::HeaderFacts;

/// The guarded classification plus any notes about what was clamped.
#[derive(Clone, Debug, PartialEq)]
pub struct Guarded {
    /// What may be shown or acted on.
    pub classification: Classification,
    /// Empty when nothing was clamped.
    pub notes: Vec<GuardNote>,
}

/// Why the guard changed the candidate's class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardNote {
    /// Candidate said `list`; the headers do not allow it.
    ListWithoutCoveredHeader,
    /// Header rules said `personal` (high confidence); the candidate disagreed.
    PersonalOverridden,
}

/// Clamp `candidate` to what the headers prove.
///
/// `header_rules` is `HeaderRules::classify` for the same message; `candidate`
/// is the result to show (header rules themselves during the bake-off, or a
/// promoted model later). `candidate` `None` means the model failed, timed out
/// or returned invalid output: the header-rules result is used unchanged.
pub fn header_guard(
    facts: &HeaderFacts,
    header_rules: &Classification,
    candidate: Option<&Classification>,
) -> Guarded {
    let Some(candidate) = candidate else {
        return Guarded {
            classification: header_rules.clone(),
            notes: Vec::new(),
        };
    };

    let mut c = candidate.clone();
    let mut notes = Vec::new();
    let mut changed = false;

    // GUARD-1: no `list` without a DKIM-covered unsubscribe option.
    if matches!(
        HeaderRules::unsubscribe_route(facts),
        UnsubscribeRoute::None
    ) && c.class == MessageClass::List
    {
        c.class = if facts.list_unsubscribe_present {
            MessageClass::BulkNoHeader
        } else if header_rules.class != MessageClass::List {
            header_rules.class
        } else {
            MessageClass::BulkNoHeader
        };
        notes.push(GuardNote::ListWithoutCoveredHeader);
        changed = true;
    }

    // GUARD-2: high-confidence `personal` from header rules is never overridden
    // to `list` or `suspect` by a model alone.
    if header_rules.class == MessageClass::Personal
        && header_rules.bulk_score <= PERSONAL_HIGH_CONFIDENCE_MAX_SCORE
        && matches!(c.class, MessageClass::List | MessageClass::Suspect)
    {
        c.class = MessageClass::Personal;
        notes.push(GuardNote::PersonalOverridden);
        changed = true;
    }

    // When the class changed, the shown reason and score must match it.
    if changed {
        c.bulk_reason.clone_from(&header_rules.bulk_reason);
        c.bulk_score = header_rules.bulk_score;
    }

    Guarded {
        classification: c,
        notes,
    }
}
