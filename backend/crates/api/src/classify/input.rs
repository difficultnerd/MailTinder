//! Building the model input (T-901 stub; T-903 replaces the body).

use domain::MessageMeta;
use ports::ClassifierInput;

/// The input format version. T-903 bumps it.
pub const INPUT_VERSION: &str = "0";
/// The question version. T-903 replaces it with its own constant.
pub const QUESTION_VERSION: &str = "0";

/// Until T-903 lands: subject and text are empty strings, `input_version` "0".
pub fn build_input(meta: &MessageMeta, _stripped_text: &str) -> ClassifierInput {
    let facts = &meta.facts;
    let from_domain = meta
        .from_address
        .rsplit_once('@')
        .map(|(_, d)| d.trim_matches('>').to_lowercase())
        .unwrap_or_default();
    ClassifierInput {
        from_display: String::new(),
        from_domain,
        list_id: facts.list_id.clone(),
        has_list_unsubscribe: facts.list_unsubscribe_present,
        has_list_unsubscribe_post: facts
            .list_unsubscribe
            .as_ref()
            .is_some_and(|o| o.one_click_https.is_some()),
        precedence: facts.precedence_bulk.then(|| "bulk".to_owned()),
        auto_submitted: facts.auto_submitted.then(|| "auto-generated".to_owned()),
        esp_header_names: facts.esp_hint.iter().cloned().collect(),
        auth_summary: if facts.from_authenticated {
            "pass"
        } else {
            "none"
        }
        .to_owned(),
        subject: String::new(),
        text: String::new(),
        input_version: INPUT_VERSION,
    }
}
