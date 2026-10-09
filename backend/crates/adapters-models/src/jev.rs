//! The `JevClassifier` adapter: TypeSafe's System One API (`jev-1.13.0`).
//!
//! One request carries both questions (a `Choice` over the five classes and a
//! `Score` for bulk). The answer is validated strictly; any deviation becomes a
//! [`ClassifierError`] so the card is unaffected (CL-03 AC2, CR-01 3).
//!
//! ## Confirmed wire format
//!
//! Confirmed against TypeSafe's API reference, <https://docs.typesafe.ai/api.md>
//! (sections "Evaluation endpoint", "Question types" and "Answer types"), and
//! the JavaScript SDK interface pages it links, on 2026-10-09. The task's
//! earlier shape was an assumption; this is the confirmed contract:
//!
//! ```json
//! // request
//! {
//!   "model": "jev-1.13.0",
//!   "state": "<render_model_text(input)>",
//!   "questions": {
//!     "class": {
//!       "type": "choice",
//!       "instructions": "<CLASS_QUESTION>",
//!       "criteria": { "list": "…", "bulk_no_header": "…", "notice": "…", "personal": "…", "suspect": "…" }
//!     },
//!     "bulk": {
//!       "type": "score",
//!       "instructions": "<BULK_QUESTION>",
//!       "criteria": [ "0 (certainly one-to-one)", "100 (certainly bulk)" ]
//!     }
//!   }
//! }
//! // response
//! {
//!   "model": "jev-1.13.0",
//!   "answers": {
//!     "class": { "type": "choice", "choice": "list",
//!                "probabilities": { "list": 0.7, "bulk_no_header": 0.1, "notice": 0.1, "personal": 0.05, "suspect": 0.05 },
//!                "confidence": 0.7 },
//!     "bulk":  { "type": "score", "score": 0.42,
//!                "legend": { "0": "0 (certainly one-to-one)", "1": "100 (certainly bulk)" },
//!                "probabilities": { "0": 0.58, "1": 0.42 },
//!                "confidence": 0.9 }
//!   },
//!   "usage": { "input_tokens": 5, "output_tokens": 2 }
//! }
//! ```
//!
//! Three consequences of the confirmed shape, recorded here because they are
//! not obvious from the task's earlier assumption:
//!
//! - `questions` and `answers` are **maps keyed by the question id**, not
//!   arrays. Each question carries `instructions` (not `prompt`); a Choice
//!   carries `criteria` as an option-to-description map and a Score carries
//!   `criteria` as an ordered array of level descriptions.
//! - A Choice answer returns its `probabilities` as an **option-to-probability
//!   map**, so we reorder by the fixed [`ports::prompt::CLASS_OPTIONS`] order
//!   and reject a missing or extra option (task edge case).
//! - A Score answer's `score` is the probability-weighted level index and the
//!   API caps a Score at ten levels, so a 101-level 0-to-100 rubric is not
//!   expressible. We send two levels anchored at 0 and 100; the confirmed
//!   `score` is then a fraction in `0..=1` we scale to `0..=100`.
#![allow(clippy::doc_markdown)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Classification, MessageClass};
use obs::Sensitive;
use ports::{
    Classifier, ClassifierError, ClassifierId, ClassifierInput, EgressError, EgressRequest,
    EgressResponse, HttpEgress, HttpMethod,
};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

/// The pinned model. Never `jev-latest`: its answers change without notice
/// (CR-01 1.1).
pub const JEV_MODEL: &str = "jev-1.13.0";
/// The `ClassifierId` this adapter reports.
pub const JEV_ID: &str = "jev@1.13.0";
/// The production base URL. The only base URL a release build can reach.
pub const PROD_BASE_URL: &str = "https://api.typesafe.ai";
/// Refuse a response body larger than this.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// `[DEFAULT]` how far the class probabilities may sum from 1.
pub const PROBABILITY_SUM_TOLERANCE: f64 = 1e-3;

/// The System One path on the vendor host (in the `api` egress allowlist).
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
/// The `[TUNABLE]` per-call timeout (CR-01 T-new-3), enforced by the egress.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// The `bulk` `Score` rubric. TypeSafe returns the probability-weighted level
/// index, so two anchors give a fraction in `0..=1`; the API allows at most ten
/// levels, so this is how a 0-to-100 question is asked.
const BULK_LEVELS: [&str; 2] = ["0 (certainly one-to-one)", "100 (certainly bulk)"];
/// The bulk score's range: the two-anchor rubric's fraction times this.
const BULK_SCORE_SCALE: f64 = 100.0;

/// Where the classifier calls the vendor. `base_url` is overridable only in
/// test builds (see [`JevConfig::with_base_url`]).
pub struct JevConfig {
    pub base_url: Url,
}

impl JevConfig {
    /// The production configuration: the pinned [`PROD_BASE_URL`].
    ///
    /// # Errors
    ///
    /// Returns a [`url::ParseError`] only if `PROD_BASE_URL` stops parsing.
    pub fn production() -> Result<Self, url::ParseError> {
        Ok(Self {
            base_url: Url::parse(PROD_BASE_URL)?,
        })
    }

    /// Test-only base URL override. Compiled only under `cfg(test)` or the
    /// `testkit` feature; a release build has no way to reach another host.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn with_base_url(base_url: Url) -> Self {
        Self { base_url }
    }
}

/// The Jev classifier: one request, two questions, strict validation.
pub struct JevClassifier {
    cfg: JevConfig,
    egress: Arc<dyn HttpEgress>,
    api_key: Sensitive<String>,
}

impl JevClassifier {
    #[must_use]
    pub fn new(cfg: JevConfig, egress: Arc<dyn HttpEgress>, api_key: Sensitive<String>) -> Self {
        Self {
            cfg,
            egress,
            api_key,
        }
    }
}

#[async_trait]
impl Classifier for JevClassifier {
    fn id(&self) -> ClassifierId {
        ClassifierId(JEV_ID.to_owned())
    }

    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError> {
        let body = request_body(input).map_err(|_| ClassifierError::InvalidOutput)?;
        let url = self
            .cfg
            .base_url
            .join(SYSTEM_ONE_PATH)
            .map_err(|_| ClassifierError::Unavailable)?;
        let request = EgressRequest {
            method: HttpMethod::Post,
            url,
            headers: vec![
                (
                    "Authorization".to_owned(),
                    Sensitive::new(format!("Bearer {}", self.api_key.expose())),
                ),
                (
                    "Content-Type".to_owned(),
                    Sensitive::new("application/json".to_owned()),
                ),
            ],
            body: Some(body),
            timeout: REQUEST_TIMEOUT,
        };
        let response = self
            .egress
            .call(request)
            .await
            .map_err(|error| map_egress(&error))?;
        interpret(&response)
    }
}

/// A `Timeout` on the wire is a classifier timeout; anything else is an outage.
fn map_egress(error: &EgressError) -> ClassifierError {
    if *error == EgressError::Timeout {
        ClassifierError::Timeout
    } else {
        ClassifierError::Unavailable
    }
}

/// Build the request body: the pinned model, the shared rendered state, and the
/// two named questions. `questions` is a map keyed by the question id, each
/// Choice option keeps the fixed `CLASS_OPTIONS` order (CR-01 1.1).
fn request_body(input: &ClassifierInput) -> Result<Vec<u8>, serde_json::Error> {
    let state = ports::prompt::render_model_text(input);
    let request = JevRequest {
        model: JEV_MODEL,
        state: &state,
        questions: JevQuestions {
            class: JevQuestion::Choice {
                instructions: ports::prompt::CLASS_QUESTION,
                criteria: OrderedMap {
                    entries: ports::prompt::CLASS_OPTIONS.as_slice(),
                },
            },
            bulk: JevQuestion::Score {
                instructions: ports::prompt::BULK_QUESTION,
                criteria: BULK_LEVELS,
            },
        },
    };
    serde_json::to_vec(&request)
}

/// Validate the response and turn it into a [`Classification`]. First failure
/// wins (CR-01 3, S4 5.1).
fn interpret(response: &EgressResponse) -> Result<Classification, ClassifierError> {
    if response.status != 200 {
        return Err(ClassifierError::Http(response.status));
    }
    if !content_type_is_json(&response.headers) || response.body.len() > MAX_RESPONSE_BYTES {
        return Err(ClassifierError::InvalidOutput);
    }
    let parsed: JevResponse =
        serde_json::from_slice(&response.body).map_err(|_| ClassifierError::InvalidOutput)?;
    if parsed.model != JEV_MODEL
        || parsed.answers.class.kind != "choice"
        || parsed.answers.bulk.kind != "score"
    {
        return Err(ClassifierError::InvalidOutput);
    }
    let Some(class) = MessageClass::ALL
        .iter()
        .copied()
        .find(|c| c.as_str() == parsed.answers.class.choice)
    else {
        return Err(ClassifierError::InvalidOutput);
    };
    let Some(probabilities) = ordered_probabilities(&parsed.answers.class.probabilities) else {
        return Err(ClassifierError::InvalidOutput);
    };
    let sum: f64 = probabilities.iter().sum();
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(ClassifierError::InvalidOutput);
    }
    let class_confidence = parsed.answers.class.confidence;
    let bulk_confidence = parsed.answers.bulk.confidence;
    let score = parsed.answers.bulk.score;
    if !in_unit(class_confidence) || !in_unit(bulk_confidence) || !in_unit(score) {
        return Err(ClassifierError::InvalidOutput);
    }
    Ok(Classification {
        class,
        bulk_score: bulk_score(score),
        bulk_reason: "jev".to_owned(),
        confidence: Some(as_f32(class_confidence)),
        probabilities: Some(to_probabilities(&probabilities)),
    })
}

/// The five class probabilities reordered into [`ports::prompt::CLASS_OPTIONS`]
/// order, or `None` if any option is missing, any extra option is present, or
/// any probability is not finite and in `0..=1`.
fn ordered_probabilities(values: &BTreeMap<String, f64>) -> Option<[f64; 5]> {
    if values.len() != ports::prompt::CLASS_OPTIONS.len() {
        return None;
    }
    let mut out = [0.0_f64; 5];
    for (slot, (name, _)) in out.iter_mut().zip(ports::prompt::CLASS_OPTIONS.iter()) {
        let value = values.get(*name)?;
        if !in_unit(*value) {
            return None;
        }
        *slot = *value;
    }
    Some(out)
}

/// Finite and within `0` to `1` inclusive.
fn in_unit(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// A content type that is `application/json` (a `charset` suffix is allowed).
fn content_type_is_json(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
    })
}

/// `score` is already finite and in `0..=1` (the two-anchor rubric's fraction);
/// scale it to `0..=100`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn bulk_score(score: f64) -> u8 {
    (score * BULK_SCORE_SCALE).round() as u8
}

/// `value` is already finite and in `0.0..=1.0`.
#[allow(clippy::cast_possible_truncation)]
fn as_f32(value: f64) -> f32 {
    value as f32
}

/// The five probabilities, in the option order we sent.
#[allow(clippy::cast_possible_truncation)]
fn to_probabilities(values: &[f64]) -> [f32; 5] {
    let mut out = [0.0_f32; 5];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = *value as f32;
    }
    out
}

// --- Confirmed wire format (see the module doc) ---

/// Serialises `entries` as a JSON object in the order given (never sorted), so
/// the Choice criteria keep the fixed `CLASS_OPTIONS` order (CR-01 1.1).
struct OrderedMap<'a> {
    entries: &'a [(&'a str, &'a str)],
}

impl Serialize for OrderedMap<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (key, value) in self.entries {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct JevRequest<'a> {
    model: &'a str,
    state: &'a str,
    questions: JevQuestions<'a>,
}

#[derive(Serialize)]
struct JevQuestions<'a> {
    class: JevQuestion<'a>,
    bulk: JevQuestion<'a>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JevQuestion<'a> {
    Choice {
        instructions: &'a str,
        criteria: OrderedMap<'a>,
    },
    Score {
        instructions: &'a str,
        criteria: [&'a str; 2],
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JevResponse {
    model: String,
    answers: JevAnswers,
    #[serde(default)]
    #[allow(dead_code)]
    usage: Option<JevUsage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JevAnswers {
    class: ChoiceAnswer,
    bulk: ScoreAnswer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    /// Probabilities keyed by option name (reordered into `CLASS_OPTIONS` order).
    probabilities: BTreeMap<String, f64>,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreAnswer {
    #[serde(rename = "type")]
    kind: String,
    /// The probability-weighted level index over `BULK_LEVELS`.
    score: f64,
    /// Rubric descriptions keyed by level (not read; present in the wire shape).
    #[allow(dead_code)]
    legend: BTreeMap<String, String>,
    /// Probabilities keyed by level (not read; present in the wire shape).
    #[allow(dead_code)]
    probabilities: BTreeMap<String, f64>,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JevUsage {
    #[serde(rename = "input_tokens")]
    #[allow(dead_code)]
    input: u64,
    #[serde(rename = "output_tokens")]
    #[allow(dead_code)]
    output: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input() -> ClassifierInput {
        ClassifierInput {
            from_display: "News".to_owned(),
            from_domain: "example.com".to_owned(),
            list_id: Some("news.example.com".to_owned()),
            has_list_unsubscribe: true,
            has_list_unsubscribe_post: false,
            precedence: Some("bulk".to_owned()),
            auto_submitted: None,
            esp_header_names: vec!["List-Id".to_owned()],
            auth_summary: "pass".to_owned(),
            subject: "Weekly digest".to_owned(),
            text: "Hello there".to_owned(),
            input_version: "1",
        }
    }

    /// The `state` we send is exactly what Gemini is given as user content.
    #[test]
    fn jev_state_equals_gemini_content() -> Result<(), serde_json::Error> {
        let input = sample_input();
        let body = request_body(&input)?;
        let value: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(
            value["state"],
            serde_json::Value::String(ports::prompt::render_model_text(&input))
        );
        Ok(())
    }

    /// Documented `cfg` check: outside test builds and the `testkit` feature,
    /// `with_base_url` does not exist, so `production` is the only constructor.
    #[test]
    fn jev_base_url_not_overridable_in_release() -> Result<(), url::ParseError> {
        let config = JevConfig::production()?;
        assert_eq!(config.base_url, Url::parse(PROD_BASE_URL)?);
        assert_eq!(config.base_url.host_str(), Some("api.typesafe.ai"));
        Ok(())
    }
}
