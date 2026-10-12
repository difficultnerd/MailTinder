//! Swipe-to-label mapping (BAKE-6, S4 5.6, CR-01 1.6).

use crate::MessageClass;

/// The ground-truth label a swipe expresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Junk mail.
    Junk,
    /// Wanted mail.
    Wanted,
}

/// The four swipe directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeDirection {
    /// Left: junk.
    Left,
    /// Right: wanted.
    Right,
    /// Up: wanted.
    Up,
    /// Down: not a label.
    Down,
}

/// Left not undone: junk. Right or up not undone: wanted. Undone flips the
/// label. Down is not a label.
#[must_use]
pub fn label_for(direction: SwipeDirection, undone: bool) -> Option<Label> {
    match (direction, undone) {
        (SwipeDirection::Down, _) => None,
        (SwipeDirection::Left, false) | (SwipeDirection::Right | SwipeDirection::Up, true) => {
            Some(Label::Junk)
        }
        (SwipeDirection::Left, true) | (SwipeDirection::Right | SwipeDirection::Up, false) => {
            Some(Label::Wanted)
        }
    }
}

/// The label a classifier prediction implies: `list`, `bulk_no_header` and
/// `suspect` are junk; `notice` and `personal` are wanted.
#[must_use]
pub fn predicted_label(class: MessageClass) -> Label {
    match class {
        MessageClass::List | MessageClass::BulkNoHeader | MessageClass::Suspect => Label::Junk,
        MessageClass::Notice | MessageClass::Personal => Label::Wanted,
    }
}
