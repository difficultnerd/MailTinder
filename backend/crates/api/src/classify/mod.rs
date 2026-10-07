//! The classification pipeline (T-901): header rules, the header guard, and the
//! bake-off models behind the [`ports::Classifier`] trait.
//!
//! Every card's classification goes through one place: [`classify_page`] runs
//! `HeaderRules::classify` and `header_guard` for the badge, then — when the
//! per-request gate is open for a model — that model for every card on the page
//! in parallel, sealed into the card's `classification_token` so the later swipe
//! records it without calling a model again.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::too_many_lines
)]

pub mod input;
pub mod payload;

use std::sync::Arc;
use std::time::Duration;

use domain::{header_guard, Classification, HeaderRules, MessageMeta, Provider, HEADER_RULES_ID};
use futures::stream::{self, StreamExt};
use ports::{
    Classifier, ClassifierError, ClassifierId, ClassifierInput, ModelPrediction, TextTokensBucket,
};
use time::OffsetDateTime;

use crate::routes::feed::ClassificationPayload;

pub use input::{build_input, INPUT_VERSION, QUESTION_VERSION};
pub use payload::{age_bucket, eval_header_facts, prediction_err, prediction_ok, BakeoffPayload};

/// `[TUNABLE]` the per-model call timeout (S4 5.7, CR-01 T-new-3).
pub const MODEL_TIMEOUT: Duration = Duration::from_secs(2);
/// `[TUNABLE]` calls in flight per model per Feed request (S4 5.7).
pub const MODEL_CONCURRENCY: usize = 8;
/// `[DEFAULT]` T-908a's price-table key.
pub const PRICE_VERSION: &str = "1";

/// The bake-off models. `None` until the matching task wires the real adapter
/// (T-904, T-905).
#[derive(Clone, Default)]
pub struct ClassifierSet {
    pub gemini: Option<Arc<dyn Classifier>>,
    pub jev: Option<Arc<dyn Classifier>>,
}

/// The per-request bake-off gate: consent (T-902) AND the model's switch
/// (T-906b), computed per request. Closed by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BakeoffGate {
    pub gemini: bool,
    pub jev: bool,
}

impl BakeoffGate {
    /// True when at least one model is due to run.
    #[must_use]
    pub fn is_open(self) -> bool {
        self.gemini || self.jev
    }
}

/// One card handed to the pipeline.
pub struct CardToClassify<'a> {
    pub meta: &'a MessageMeta,
    pub stripped_text: &'a str,
    /// The mailbox's provider, for the eval record (T-906a).
    pub provider: Provider,
}

/// A card after classification: the shown badge and the payload to seal.
pub struct ClassifiedCard {
    pub badge: Classification,
    pub payload: ClassificationPayload,
}

/// Header rules and guard for every card; then, if the gate is open for a
/// model, that model for every card in parallel. Never returns an error
/// because of a model.
pub async fn classify_page(
    set: &ClassifierSet,
    gate: BakeoffGate,
    now: OffsetDateTime,
    cards: &[CardToClassify<'_>],
) -> Vec<ClassifiedCard> {
    // Step 1: the badge is always the guarded header-rules answer. During the
    // bake-off the guard's candidate is header rules itself (CL-01 AC3).
    let prepared: Vec<(Classification, Classification)> = cards
        .iter()
        .map(|c| {
            let rules = HeaderRules::classify(&c.meta.facts, &c.meta.sender);
            let badge = header_guard(&c.meta.facts, &rules, Some(&rules)).classification;
            (rules, badge)
        })
        .collect();

    // Step 2: with the gate closed for both models there is no bake-off.
    if !gate.is_open() {
        return cards
            .iter()
            .zip(prepared)
            .map(|(c, (rules, badge))| ClassifiedCard {
                badge,
                payload: ClassificationPayload {
                    mailbox_id: c.meta.mailbox.0,
                    message_id: c.meta.id.as_str().to_owned(),
                    header_rules: rules,
                    classifier_id: HEADER_RULES_ID.to_owned(),
                    issued_at: now,
                    bakeoff: None,
                },
            })
            .collect();
    }

    // Step 3: one shared input per card, so both models see byte-identical
    // input (BAKE-2).
    let inputs: Vec<Arc<ClassifierInput>> = cards
        .iter()
        .map(|c| Arc::new(build_input(c.meta, c.stripped_text)))
        .collect();

    // Step 4: the two model streams run at the same time.
    let (gemini, jev) = tokio::join!(
        run_model(set.gemini.as_ref(), gate.gemini, &inputs),
        run_model(set.jev.as_ref(), gate.jev, &inputs),
    );

    // Steps 6 and 8: build the sealed bake-off payload per card.
    cards
        .iter()
        .zip(prepared)
        .enumerate()
        .map(|(i, (c, (rules, badge)))| ClassifiedCard {
            badge,
            payload: ClassificationPayload {
                mailbox_id: c.meta.mailbox.0,
                message_id: c.meta.id.as_str().to_owned(),
                header_rules: rules,
                classifier_id: HEADER_RULES_ID.to_owned(),
                issued_at: now,
                bakeoff: Some(BakeoffPayload {
                    gemini: gemini[i].clone(),
                    jev: jev[i].clone(),
                    header_facts: eval_header_facts(&c.meta.facts),
                    provider: c.provider,
                    age_bucket: age_bucket(c.meta.internal_date, now),
                    text_tokens_bucket: TextTokensBucket::Under100,
                    lang_is_english: false,
                    input_version: inputs[i].input_version.to_owned(),
                    question_version: QUESTION_VERSION.to_owned(),
                    price_version: PRICE_VERSION.to_owned(),
                }),
            },
        })
        .collect()
}

/// Runs one model over every card, bounded by [`MODEL_CONCURRENCY`]. A closed
/// gate or a missing model yields `None` for every card.
async fn run_model(
    model: Option<&Arc<dyn Classifier>>,
    open: bool,
    inputs: &[Arc<ClassifierInput>],
) -> Vec<Option<ModelPrediction>> {
    let Some(model) = model.filter(|_| open) else {
        return vec![None; inputs.len()];
    };
    let id = model.id();
    let results: Vec<(usize, ModelPrediction)> = stream::iter(inputs.iter().cloned().enumerate())
        .map(|(i, input)| {
            let model = Arc::clone(model);
            let id = id.clone();
            async move { (i, call(model.as_ref(), &id, &input).await) }
        })
        .buffer_unordered(MODEL_CONCURRENCY)
        .collect()
        .await;
    let mut out: Vec<Option<ModelPrediction>> = vec![None; inputs.len()];
    for (i, prediction) in results {
        out[i] = Some(prediction);
    }
    out
}

/// One model call: pausable clock, timeout, validation and error mapping.
async fn call(
    model: &dyn Classifier,
    id: &ClassifierId,
    input: &ClassifierInput,
) -> ModelPrediction {
    let start = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(MODEL_TIMEOUT, model.classify(input)).await;
    let latency_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
    match outcome {
        Ok(Ok(classification)) if valid(&classification) => {
            prediction_ok(id, &classification, latency_ms, None)
        }
        Ok(Ok(_)) => prediction_err(id, &ClassifierError::InvalidOutput, latency_ms),
        Ok(Err(e)) => prediction_err(id, &e, latency_ms),
        Err(_) => prediction_err(id, &ClassifierError::Timeout, latency_ms),
    }
}

/// A model answer is used only when its score, confidence and probabilities are
/// in range; anything else is `InvalidOutput` (CL-03 AC2).
fn valid(c: &Classification) -> bool {
    let confidence_ok = c.confidence.map_or(true, in_unit);
    let probabilities_ok = match c.probabilities {
        Some(p) => p.iter().all(|v| in_unit(*v)),
        None => true,
    };
    c.bulk_score <= 100 && confidence_ok && probabilities_ok
}

/// Finite and within `0` to `1` inclusive.
fn in_unit(v: f32) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}
