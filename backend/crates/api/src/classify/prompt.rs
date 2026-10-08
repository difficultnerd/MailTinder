//! One byte-identical rendering and fixed questions for both model adapters.
use ports::ClassifierInput;

pub const QUESTION_VERSION: &str = "1";
pub const CLASS_OPTIONS: [(&str, &str); 5] = [
    (
        "list",
        "a mailing list or newsletter the person subscribed to",
    ),
    (
        "bulk_no_header",
        "bulk or marketing mail with no proper unsubscribe header",
    ),
    (
        "notice",
        "an account, billing or security notice from a service",
    ),
    ("personal", "a one-to-one message written by a person"),
    ("suspect", "spam or phishing"),
];
pub const CLASS_QUESTION: &str = "Which kind of email is this?";
pub const BULK_QUESTION: &str = "How likely is it that this email was sent in bulk to many people, from 0 (certainly one-to-one) to 100 (certainly bulk)?";

fn value(text: &str) -> &str {
    if text.is_empty() {
        "none"
    } else {
        text
    }
}

/// Never log or persist the result; wrap it in `Sensitive` at the egress boundary.
#[must_use]
pub fn render_model_text(input: &ClassifierInput) -> String {
    format!(
        "from_name: {}\nfrom_domain: {}\nlist_id: {}\nlist_unsubscribe: {}\nlist_unsubscribe_post: {}\nprecedence: {}\nauto_submitted: {}\nesp: {}\nauthentication: {}\nsubject: {}\ntext: {}",
        value(&input.from_display), value(&input.from_domain),
        value(input.list_id.as_deref().unwrap_or("")),
        if input.has_list_unsubscribe { "present" } else { "absent" },
        if input.has_list_unsubscribe_post { "one-click" } else { "absent" },
        value(input.precedence.as_deref().unwrap_or("")),
        value(input.auto_submitted.as_deref().unwrap_or("")),
        value(&input.esp_header_names.join(",")), value(&input.auth_summary),
        value(&input.subject), value(&input.text),
    )
}
