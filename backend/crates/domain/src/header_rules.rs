//! The `HeaderRules` classifier: a pure, deterministic mapping from
//! `HeaderFacts` plus the `SenderKey` to a `Classification`, and the one
//! unsubscribe route a reject may use.
//!
//! Everything here is a pure function of its arguments: same input, same
//! output, no clock, no randomness (T-102 algorithm step 6).

use std::fmt;

use url::Url;

use crate::class::{Classification, MessageClass};
use crate::message::{HeaderFacts, MailtoTarget};
use crate::sender::SenderKey;

/// The stateless header-rules classifier. Version `domain::HEADER_RULES_ID`.
pub struct HeaderRules;

/// The single unsubscribe route a reject may use, chosen from the DKIM-covered
/// options only (`facts.list_unsubscribe`, S6 6).
#[derive(Clone, PartialEq, Eq)]
pub enum UnsubscribeRoute {
    /// One-click (RFC 8058): a job, method `one_click` (UN-02).
    OneClick(Url),
    /// Mailto: a job, method `mailto` (UN-03).
    Mailto(MailtoTarget),
    /// No job: Needs Attention "Open unsubscribe page"; the link is present
    /// only when the scheme is https (UN-04 AC6).
    ManualLink(Option<Url>),
    /// No DKIM-covered option.
    None,
}

impl fmt::Debug for UnsubscribeRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            UnsubscribeRoute::OneClick(_) => "OneClick",
            UnsubscribeRoute::Mailto(_) => "Mailto",
            UnsubscribeRoute::ManualLink(_) => "ManualLink",
            UnsubscribeRoute::None => "None",
        };
        f.write_str(name)
    }
}

/// Used by T-103 (GUARD-2): header rules say `personal` with high confidence
/// at or below this score.
pub const PERSONAL_HIGH_CONFIDENCE_MAX_SCORE: u8 = 10;

/// S7 Card `bulk_reason` length cap.
pub const BULK_REASON_MAX_CHARS: usize = 200;

/// Keys from T-401's `ESP_HINTS` mapped to display names. An unknown key is
/// never echoed; it becomes the generic phrase.
pub const ESP_NAMES: [(&str, &str); 8] = [
    ("mailchimp", "Mailchimp"),
    ("sendgrid", "SendGrid"),
    ("mailgun", "Mailgun"),
    ("amazon_ses", "Amazon SES"),
    ("salesforce", "Salesforce"),
    ("hubspot", "HubSpot"),
    ("constant_contact", "Constant Contact"),
    ("klaviyo", "Klaviyo"),
];

// Score weights. `[DEFAULT]`: spike E2 sets the thresholds; until it has run
// these additive weights stand in and are `[TUNABLE]` by editing this one
// table (T-102 algorithm step 2).
const SCORE_LIST_UNSUBSCRIBE_PRESENT: u8 = 30;
const SCORE_ROUTE_PRESENT: u8 = 15;
const SCORE_LIST_ID: u8 = 15;
const SCORE_PRECEDENCE_BULK: u8 = 20;
const SCORE_FEEDBACK_ID: u8 = 10;
const SCORE_ESP_HINT: u8 = 10;
const SCORE_AUTO_SUBMITTED: u8 = 5;
const SCORE_REPLY_OR_THREAD_SUBTRACT: u8 = 30;

impl HeaderRules {
    /// Classify a message from its header facts and sender key.
    pub fn classify(facts: &HeaderFacts, sender: &SenderKey) -> Classification {
        let score = Self::score(facts);
        let class = Self::class(facts, sender);
        let reason = Self::reason(facts, class);
        Classification {
            class,
            bulk_score: score,
            bulk_reason: reason,
            confidence: Some(f32::from(score) / 100.0),
            probabilities: None,
        }
    }

    /// The one unsubscribe route a reject may use, from the DKIM-covered
    /// options only (S6 6). Never reads `list_unsubscribe_present`.
    pub fn unsubscribe_route(facts: &HeaderFacts) -> UnsubscribeRoute {
        let Some(options) = &facts.list_unsubscribe else {
            return UnsubscribeRoute::None;
        };
        if let Some(url) = &options.one_click_https {
            if url.scheme() == "https" {
                return UnsubscribeRoute::OneClick(url.clone());
            }
        }
        if let Some(target) = &options.mailto {
            return UnsubscribeRoute::Mailto(target.clone());
        }
        if let Some(url) = &options.https {
            if url.scheme() == "https" {
                return UnsubscribeRoute::ManualLink(Some(url.clone()));
            }
            return UnsubscribeRoute::ManualLink(None);
        }
        UnsubscribeRoute::None
    }

    /// The bulk score, 0..=100, from additive weights clamped to the range.
    pub fn score(facts: &HeaderFacts) -> u8 {
        let mut score: i32 = 0;
        if facts.list_unsubscribe_present {
            score += i32::from(SCORE_LIST_UNSUBSCRIBE_PRESENT);
        }
        if !matches!(Self::unsubscribe_route(facts), UnsubscribeRoute::None) {
            score += i32::from(SCORE_ROUTE_PRESENT);
        }
        if facts.list_id.is_some() {
            score += i32::from(SCORE_LIST_ID);
        }
        if facts.precedence_bulk {
            score += i32::from(SCORE_PRECEDENCE_BULK);
        }
        if facts.feedback_id.is_some() {
            score += i32::from(SCORE_FEEDBACK_ID);
        }
        if facts.esp_hint.is_some() {
            score += i32::from(SCORE_ESP_HINT);
        }
        if facts.auto_submitted {
            score += i32::from(SCORE_AUTO_SUBMITTED);
        }
        if facts.is_reply_or_thread {
            score -= i32::from(SCORE_REPLY_OR_THREAD_SUBTRACT);
        }
        u8::try_from(score.clamp(0, 100)).unwrap_or(0)
    }

    /// The class, first rule that holds wins (T-102 algorithm step 3).
    fn class(facts: &HeaderFacts, sender: &SenderKey) -> MessageClass {
        if sender.is_empty()
            || (!facts.from_authenticated && (facts.reply_to_mismatch || facts.display_name_spoof))
        {
            return MessageClass::Suspect;
        }
        if !matches!(Self::unsubscribe_route(facts), UnsubscribeRoute::None) {
            return MessageClass::List;
        }
        if facts.list_unsubscribe_present || facts.precedence_bulk || facts.list_id.is_some() {
            return MessageClass::BulkNoHeader;
        }
        if facts.auto_submitted
            || sender.is_noreply()
            || facts.esp_hint.is_some()
            || facts.feedback_id.is_some()
        {
            return MessageClass::Notice;
        }
        MessageClass::Personal
    }

    /// The plain-words reason, fixed fragments only, joined with ", ",
    /// first letter capitalised, at most `BULK_REASON_MAX_CHARS`.
    fn reason(facts: &HeaderFacts, class: MessageClass) -> String {
        let mut fragments: Vec<String> = Vec::new();
        let class_fragment = match class {
            MessageClass::List => "Mailing list",
            MessageClass::BulkNoHeader => "Looks like bulk mail with no unsubscribe header",
            MessageClass::Notice => "Automated notice",
            MessageClass::Personal => "Looks personal",
            MessageClass::Suspect => "Looks suspicious, sender not verified",
        };
        fragments.push(class_fragment.to_owned());

        if let Some(hint) = &facts.esp_hint {
            let name = ESP_NAMES
                .iter()
                .find(|(key, _)| key == hint)
                .map(|(_, name)| *name);
            match name {
                Some(name) => fragments.push(format!("sent through {name}")),
                None => fragments.push("sent through a bulk mail service".to_owned()),
            }
        }

        match Self::unsubscribe_route(facts) {
            UnsubscribeRoute::OneClick(_) => {
                fragments.push("has one-click unsubscribe".to_owned());
            }
            UnsubscribeRoute::Mailto(_) => fragments.push("unsubscribes by email".to_owned()),
            UnsubscribeRoute::ManualLink(_) => {
                fragments.push("unsubscribe needs you to visit their page".to_owned());
            }
            UnsubscribeRoute::None => {}
        }

        if class == MessageClass::List && facts.auto_submitted && !facts.precedence_bulk {
            // `[DEFAULT]`: header rules cannot tell marketing from
            // transactional; this is the only hint they have.
            fragments.push("may be account updates".to_owned());
        }

        let mut reason = fragments.join(", ");
        if let Some(first) = reason.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        reason.truncate(BULK_REASON_MAX_CHARS);
        reason
    }
}
