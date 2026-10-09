//! The `JevClassifier` adapter: TypeSafe's System One API (`jev-1.13.0`).
//!
//! One request carries both questions (a `Choice` over the five classes and a
//! `Score` for bulk). The answer is validated strictly; any deviation becomes a
//! [`ClassifierError`] so the card is unaffected (CL-03 AC2, CR-01 3).
#![allow(clippy::doc_markdown)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Classification, MessageClass};
use obs::Sensitive;
use ports::{
    Classifier, ClassifierError, ClassifierId, ClassifierInput, EgressError, EgressRequest,
    EgressResponse, HttpEgress, HttpMethod,
};
use serde::{Deserialize, Serialize};
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
/// The named question for the class `Choice`.
const CLASS_NAME: &str = "class";
/// The named question for the bulk `Score`.
const BULK_NAME: &str = "bulk";

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
/// two questions in fixed order.
fn request_body(input: &ClassifierInput) -> Result<Vec<u8>, serde_json::Error> {
    let state = ports::prompt::render_model_text(input);
    let request = JevRequest {
        model: JEV_MODEL,
        state: &state,
        questions: [
            JevQuestion::Choice {
                name: CLASS_NAME,
                prompt: ports::prompt::CLASS_QUESTION,
                options: ports::prompt::CLASS_OPTIONS.map(|(name, _)| name),
            },
            JevQuestion::Score {
                name: BULK_NAME,
                prompt: ports::prompt::BULK_QUESTION,
                min: 0,
                max: 100,
            },
        ],
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
    if parsed.model != JEV_MODEL {
        return Err(ClassifierError::InvalidOutput);
    }
    let Some(class) = MessageClass::ALL
        .iter()
        .copied()
        .find(|c| c.as_str() == parsed.answers.class.choice)
    else {
        return Err(ClassifierError::InvalidOutput);
    };
    let probabilities = &parsed.answers.class.probabilities;
    if probabilities.len() != 5 || !probabilities.iter().all(|v| in_unit(*v)) {
        return Err(ClassifierError::InvalidOutput);
    }
    let sum: f64 = probabilities.iter().sum();
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(ClassifierError::InvalidOutput);
    }
    let class_confidence = parsed.answers.class.confidence;
    let bulk_confidence = parsed.answers.bulk.confidence;
    let score = parsed.answers.bulk.score;
    if !in_unit(class_confidence) || !in_unit(bulk_confidence) || !(0.0..=100.0).contains(&score) {
        return Err(ClassifierError::InvalidOutput);
    }
    Ok(Classification {
        class,
        bulk_score: round_score(score),
        bulk_reason: "jev".to_owned(),
        confidence: Some(as_f32(class_confidence)),
        probabilities: Some(to_probabilities(probabilities)),
    })
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

/// `score` is already in `0.0..=100.0`; round to the nearest whole number.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_score(score: f64) -> u8 {
    score.round() as u8
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

// --- Wire format (an assumption; see the task's edge cases) ---

#[derive(Serialize)]
struct JevRequest<'a> {
    model: &'a str,
    state: &'a str,
    questions: [JevQuestion<'a>; 2],
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JevQuestion<'a> {
    Choice {
        name: &'a str,
        prompt: &'a str,
        options: [&'a str; 5],
    },
    Score {
        name: &'a str,
        prompt: &'a str,
        min: u8,
        max: u8,
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
    choice: String,
    probabilities: Vec<f64>,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreAnswer {
    score: f64,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct JevUsage {
    input_tokens: u64,
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
