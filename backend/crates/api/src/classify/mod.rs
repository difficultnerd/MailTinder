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
pub mod prompt;

use std::sync::Arc;
use std::time::Duration;

use domain::redact::approx_tokens;
use domain::{header_guard, Classification, HeaderRules, MessageMeta, Provider, HEADER_RULES_ID};
use futures::stream::{self, StreamExt};
use ports::{Classifier, ClassifierError, ClassifierId, ClassifierInput, ModelPrediction};
use time::OffsetDateTime;

use crate::routes::feed::ClassificationPayload;

/// The per-request bake-off gate: consent (T-902) AND the model's switch
/// (T-906b), computed per request. Reusing T-902's type keeps one gate shape
/// (its doc comment says T-901 imports it rather than defining a second type).
pub use crate::experiments::BakeoffGate;
pub use input::build_input;
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
    // input (BAKE-2). T-903's `build_input` also returns the coarse buckets
    // for the sealed eval payload.
    let built: Vec<(Arc<ClassifierInput>, domain::redact::InputFacts)> = cards
        .iter()
        .map(|c| {
            let (value, facts) = build_input(c.meta, c.stripped_text);
            (Arc::new(value), facts)
        })
        .collect();
    let inputs: Vec<Arc<ClassifierInput>> =
        built.iter().map(|(value, _)| Arc::clone(value)).collect();

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
        .map(|(i, (c, (rules, badge)))| {
            let (value, facts) = &built[i];
            ClassifiedCard {
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
                        text_tokens_bucket: facts.text_tokens_bucket,
                        lang_is_english: facts.lang_is_english,
                        input_version: value.input_version.to_owned(),
                        question_version: prompt::QUESTION_VERSION.to_owned(),
                        price_version: PRICE_VERSION.to_owned(),
                    }),
                },
            }
        })
        .collect()
}

/// Runs one model over every card, bounded by [`MODEL_CONCURRENCY`]. A closed
/// gate or a missing model yields `None` for every card.
///
/// The whole run shares **one** deadline of [`MODEL_TIMEOUT`] from the start,
/// not one timeout per wave: with the concurrency cap a large page (up to the
/// Feed cap) can otherwise wait `ceil(cards / MODEL_CONCURRENCY)` timeouts and
/// tie up worker capacity (trap 1; S4 5.7). A card not reached before the
/// deadline is recorded as a timeout and no model is called for it.
async fn run_model(
    model: Option<&Arc<dyn Classifier>>,
    open: bool,
    inputs: &[Arc<ClassifierInput>],
) -> Vec<Option<ModelPrediction>> {
    let Some(model) = model.filter(|_| open) else {
        return vec![None; inputs.len()];
    };
    let id = model.id();
    let deadline = tokio::time::Instant::now() + MODEL_TIMEOUT;
    let results: Vec<(usize, ModelPrediction)> = stream::iter(inputs.iter().cloned().enumerate())
        .map(|(i, input)| {
            let model = Arc::clone(model);
            let id = id.clone();
            async move { (i, call(model.as_ref(), &id, &input, deadline).await) }
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

/// One model call: pausable clock, the page deadline, validation and error
/// mapping. Past the deadline the model is not called at all.
async fn call(
    model: &dyn Classifier,
    id: &ClassifierId,
    input: &ClassifierInput,
    deadline: tokio::time::Instant,
) -> ModelPrediction {
    let start = tokio::time::Instant::now();
    if start >= deadline {
        return prediction_err(id, &ClassifierError::Timeout, elapsed_ms(start));
    }
    let outcome = tokio::time::timeout_at(deadline, model.classify(input)).await;
    let latency_ms = elapsed_ms(start);
    match outcome {
        Ok(Ok(classification)) if valid(&classification) => {
            let tokens = approx_tokens(&prompt::render_model_text(input));
            prediction_ok(id, &classification, latency_ms, Some(tokens))
        }
        Ok(Ok(_)) => prediction_err(id, &ClassifierError::InvalidOutput, latency_ms),
        Ok(Err(e)) => prediction_err(id, &e, latency_ms),
        Err(_) => prediction_err(id, &ClassifierError::Timeout, latency_ms),
    }
}

/// Elapsed milliseconds since `start`, saturating at `u32::MAX`.
fn elapsed_ms(start: tokio::time::Instant) -> u32 {
    u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX)
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
