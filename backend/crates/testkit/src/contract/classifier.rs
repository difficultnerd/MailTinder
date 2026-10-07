//! The `Classifier` contract suite (T-901): every implementation passes it.

use std::sync::Arc;

use domain::{HeaderFacts, HeaderRules, SenderKey};
use ports::{Classifier, ClassifierId, ClassifierInput};

use async_trait::async_trait;
use domain::Classification;
use ports::ClassifierError;

/// A small, label-free input.
fn sample() -> ClassifierInput {
    ClassifierInput {
        from_display: String::new(),
        from_domain: "example.com".to_owned(),
        list_id: Some("news.example.com".to_owned()),
        has_list_unsubscribe: true,
        has_list_unsubscribe_post: false,
        precedence: Some("bulk".to_owned()),
        auto_submitted: None,
        esp_header_names: Vec::new(),
        auth_summary: "pass".to_owned(),
        subject: String::new(),
        text: String::new(),
        input_version: "0",
    }
}

fn in_unit(v: f32) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

/// Runs the suite against implementations from `make`.
///
/// # Errors
///
/// A description of the first broken rule.
pub async fn classifier_contract(make: impl Fn() -> Arc<dyn Classifier>) -> Result<(), String> {
    let model = make();
    let id = model.id();
    if id.0.is_empty() || !id.0.contains('@') {
        return Err("id must look like name@version".to_owned());
    }
    if model.id() != id {
        return Err("id must be stable".to_owned());
    }
    let input = sample();
    let first = model.classify(&input).await.map_err(|e| e.to_string())?;
    if first.bulk_score > 100 {
        return Err("bulk_score above 100".to_owned());
    }
    if !first.confidence.map_or(true, in_unit) {
        return Err("confidence out of range".to_owned());
    }
    if let Some(p) = first.probabilities {
        if !p.iter().all(|v| in_unit(*v)) {
            return Err("probability out of range".to_owned());
        }
    }
    let second = model.classify(&input).await.map_err(|e| e.to_string())?;
    if first != second {
        return Err("same input must give the same answer".to_owned());
    }
    let (a, b) = tokio::join!(model.classify(&input), model.classify(&input));
    if a.map_err(|e| e.to_string())? != first || b.map_err(|e| e.to_string())? != first {
        return Err("concurrent calls must agree".to_owned());
    }
    if format!("{input:?}").contains("example.com") {
        return Err("Debug of the input must not print content".to_owned());
    }
    Ok(())
}

/// Wraps the static header rules as a `Classifier`, for the contract only.
pub struct HeaderRulesClassifier;

#[async_trait]
impl Classifier for HeaderRulesClassifier {
    fn id(&self) -> ClassifierId {
        ClassifierId(domain::HEADER_RULES_ID.to_owned())
    }

    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError> {
        let facts = HeaderFacts {
            list_unsubscribe_present: input.has_list_unsubscribe,
            list_id: input.list_id.clone(),
            precedence_bulk: input.precedence.is_some(),
            auto_submitted: input.auto_submitted.is_some(),
            from_authenticated: input.auth_summary == "pass",
            ..HeaderFacts::default()
        };
        let sender = SenderKey::from_address(&format!("sender@{}", input.from_domain));
        Ok(HeaderRules::classify(&facts, &sender))
    }
}
