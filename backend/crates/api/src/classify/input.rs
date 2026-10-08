//! The complete model input allowlist; no generic header pass-through.
use domain::redact::{
    from_domain, is_english, redact, text_tokens_bucket, truncate_words, word_count, InputFacts,
    INPUT_VERSION,
};
use domain::MessageMeta;
use ports::ClassifierInput;

#[must_use]
pub fn build_input(meta: &MessageMeta, stripped_text: &str) -> (ClassifierInput, InputFacts) {
    let text = redact(stripped_text);
    let facts = InputFacts {
        text_tokens_bucket: text_tokens_bucket(word_count(&text)),
        lang_is_english: is_english(&text),
    };
    let input = ClassifierInput {
        from_display: redact(&meta.from_display).chars().take(100).collect(),
        from_domain: from_domain(&meta.from_address),
        list_id: meta
            .facts
            .list_id
            .as_deref()
            .map(|id| redact(id).chars().take(200).collect()),
        has_list_unsubscribe: meta.facts.list_unsubscribe_present,
        has_list_unsubscribe_post: meta
            .facts
            .list_unsubscribe
            .as_ref()
            .is_some_and(|options| options.one_click_https.is_some()),
        precedence: meta.facts.precedence_bulk.then(|| "bulk".to_owned()),
        auto_submitted: meta
            .facts
            .auto_submitted
            .then(|| "auto-generated".to_owned()),
        esp_header_names: meta.facts.esp_hint.iter().cloned().collect(),
        auth_summary: format!(
            "from_authenticated={}",
            if meta.facts.from_authenticated {
                "pass"
            } else {
                "fail"
            }
        ),
        subject: redact(&meta.subject).chars().take(300).collect(),
        text: truncate_words(&text),
        input_version: INPUT_VERSION,
    };
    (input, facts)
}
