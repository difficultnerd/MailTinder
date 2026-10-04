//! Message classes, classifications and swipe actions.

use serde::{Deserialize, Serialize};

use crate::ids::CategoryId;

/// The class of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageClass {
    List,
    BulkNoHeader,
    Notice,
    Personal,
    Suspect,
}

impl MessageClass {
    pub const ALL: [MessageClass; 5] = [
        MessageClass::List,
        MessageClass::BulkNoHeader,
        MessageClass::Notice,
        MessageClass::Personal,
        MessageClass::Suspect,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MessageClass::List => "list",
            MessageClass::BulkNoHeader => "bulk_no_header",
            MessageClass::Notice => "notice",
            MessageClass::Personal => "personal",
            MessageClass::Suspect => "suspect",
        }
    }
}

/// The result of classifying a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub class: MessageClass,
    /// 0..=100.
    pub bulk_score: u8,
    /// Fixed text, at most 200 chars, never sender data.
    pub bulk_reason: String,
    pub confidence: Option<f32>,
    /// In `MessageClass::ALL` order.
    pub probabilities: Option<[f32; 5]>,
}

/// A swipe gesture's effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum SwipeAction {
    Keep,
    Skip,
    Reject,
    File { category: CategoryId },
}
