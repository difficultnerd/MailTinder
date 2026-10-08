//! The sealed bake-off payload and its conversions (T-901).

use domain::{HeaderFacts, Provider};
use ports::{
    AgeBucket, ClassifierError, ClassifierId, EvalHeaderFacts, ModelErrorCode, ModelPrediction,
    TextTokensBucket,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use domain::Classification;

/// What the swipe needs to write the `classifier_eval` record later (T-906a):
/// both predictions and the coarse, non-identifying context. Sealed inside the
/// card's `classification_token`; never part of the card DTO.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeoffPayload {
    /// `None`: not called (switched off). `Some` with `error_code`: called and failed.
    pub gemini: Option<ModelPrediction>,
    pub jev: Option<ModelPrediction>,
    pub header_facts: EvalHeaderFacts,
    pub provider: Provider,
    pub age_bucket: AgeBucket,
    pub text_tokens_bucket: TextTokensBucket,
    pub lang_is_english: bool,
    pub input_version: String,
    pub question_version: String,
    pub price_version: String,
}

/// `<7d`, `7d-90d`, `90d-1y` (365 d), `1y-5y`, `>5y`; the lower bound is inclusive.
pub fn age_bucket(received_at: OffsetDateTime, now: OffsetDateTime) -> AgeBucket {
    let age = now - received_at;
    if age < Duration::days(7) {
        AgeBucket::Under7d
    } else if age < Duration::days(90) {
        AgeBucket::D7To90d
    } else if age < Duration::days(365) {
        AgeBucket::D90To1y
    } else if age < Duration::days(5 * 365) {
        AgeBucket::Y1To5y
    } else {
        AgeBucket::Over5y
    }
}

/// Booleans only, for the eval record.
pub fn eval_header_facts(facts: &HeaderFacts) -> EvalHeaderFacts {
    let options = facts.list_unsubscribe.as_ref();
    EvalHeaderFacts {
        has_list_unsubscribe: facts.list_unsubscribe_present,
        dkim_covers_list_unsubscribe: options.is_some(),
        has_list_unsubscribe_post: options.is_some_and(|o| o.one_click_https.is_some()),
        has_list_id: facts.list_id.is_some(),
        has_feedback_id: facts.feedback_id.is_some(),
        precedence_bulk: facts.precedence_bulk,
        auto_submitted: facts.auto_submitted,
        from_authenticated: facts.from_authenticated,
    }
}

/// A successful model answer.
pub fn prediction_ok(
    id: &ClassifierId,
    c: &Classification,
    latency_ms: u32,
    input_tokens: Option<u32>,
) -> ModelPrediction {
    ModelPrediction {
        class: Some(c.class),
        score: Some(c.bulk_score),
        confidence: c.confidence,
        probabilities: c.probabilities,
        model_version: id.0.clone(),
        latency_ms,
        input_tokens,
        error_code: None,
    }
}

/// A failed model call, one to one with [`ClassifierError`].
pub fn prediction_err(id: &ClassifierId, e: &ClassifierError, latency_ms: u32) -> ModelPrediction {
    let code = match e {
        ClassifierError::Timeout => ModelErrorCode::Timeout,
        ClassifierError::Http(_) => ModelErrorCode::Http,
        ClassifierError::InvalidOutput => ModelErrorCode::InvalidOutput,
        ClassifierError::Disabled => ModelErrorCode::Disabled,
        ClassifierError::Unavailable => ModelErrorCode::Unavailable,
    };
    ModelPrediction {
        class: None,
        score: None,
        confidence: None,
        probabilities: None,
        model_version: id.0.clone(),
        latency_ms,
        input_tokens: None,
        error_code: Some(code),
    }
}

impl std::fmt::Debug for BakeoffPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BakeoffPayload { .. }")
    }
}
