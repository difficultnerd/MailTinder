//! T-904: `GeminiClassifier` contract and failure scenarios against
//! `fake-vertex` (S10 9.1, CL-03 AC2, BAKE-4).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::unused_async,
    clippy::items_after_statements
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use adapters_gcp::{
    GcpTokenSource, GeminiClassifier, GeminiConfig, StaticTokenSource, PROD_BASE_URL,
};
use async_trait::async_trait;
use domain::Classification;
use obs::Sensitive;
use ports::{
    Classifier, ClassifierError, ClassifierInput, EgressError, EgressRequest, EgressResponse,
    HttpEgress, OneClickOutcome,
};
use testkit::contract::classifier::classifier_contract;
use testkit::{FakeHttpEgress, FakeVertexHandle, Scenario};
use url::Url;

const PROJECT: &str = "demo-mailtinder";
/// The regional Vertex host `PROD_BASE_URL` must name.
const VERTEX_HOST: &str = "us-central1-aiplatform.googleapis.com";
/// The global endpoint host, which must never be used (S4 5.1).
const GLOBAL_HOST: &str = "aiplatform.googleapis.com";

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

fn token_source() -> Arc<dyn GcpTokenSource> {
    Arc::new(StaticTokenSource(Sensitive::new("test-token".to_owned())))
}

/// A classifier whose real egress is the testkit fake, routed to `addr`.
fn classifier(addr: SocketAddr) -> GeminiClassifier {
    let egress = Arc::new(FakeHttpEgress::new());
    egress.forward(VERTEX_HOST, addr);
    let cfg = GeminiConfig::new(
        PROJECT.to_owned(),
        testkit::fake_vertex::MODEL_VERSION.to_owned(),
    );
    GeminiClassifier::new(cfg, egress, token_source())
}

fn boxed(addr: SocketAddr) -> Arc<dyn Classifier> {
    Arc::new(classifier(addr))
}

async fn start() -> Result<FakeVertexHandle, String> {
    testkit::fake_vertex::start()
        .await
        .map_err(|error| error.to_string())
}

/// The shared `Classifier` contract suite runs against `GeminiClassifier`.
#[tokio::test]
async fn classifier_contract_gemini_fake_vertex() -> Result<(), String> {
    let fake = start().await?;
    let addr = fake.addr();
    classifier_contract(move || boxed(addr)).await
}

/// CL-03 AC2: a call slower than the 2-second timeout is a `Timeout`, proved on
/// the pausable tokio clock.
#[tokio::test(start_paused = true)]
async fn cl_03_ac2_gemini_timeout_recorded() -> Result<(), String> {
    struct SlowEgress;

    #[async_trait]
    impl HttpEgress for SlowEgress {
        async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
            Err(EgressError::NotPermitted)
        }

        async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(EgressResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: b"{}".to_vec(),
            })
        }
    }

    let cfg = GeminiConfig::new(
        PROJECT.to_owned(),
        testkit::fake_vertex::MODEL_VERSION.to_owned(),
    );
    let classifier = GeminiClassifier::new(cfg, Arc::new(SlowEgress), token_source());
    match classifier.classify(&sample()).await {
        Err(ClassifierError::Timeout) => Ok(()),
        other => Err(format!("expected Timeout, got {other:?}")),
    }
}

/// BAKE-4: every S10 9.1 scenario maps to the right error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bake_4_gemini_failure_scenarios() -> Result<(), String> {
    let fake = start().await?;
    let addr = fake.addr();
    let cases: Vec<(Scenario, Option<ClassifierError>)> = vec![
        (Scenario::Valid, None),
        (Scenario::Delay { ms: 3000 }, Some(ClassifierError::Timeout)),
        (Scenario::TooManyRequests, Some(ClassifierError::Http(429))),
        (Scenario::ServerError, Some(ClassifierError::Http(500))),
        (
            Scenario::ServiceUnavailable,
            Some(ClassifierError::Http(503)),
        ),
        (
            Scenario::ConnectionReset,
            Some(ClassifierError::Unavailable),
        ),
        (
            Scenario::MalformedJson,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::TruncatedBody,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::HtmlContentType,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::OversizedBody,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::ClassOutsideEnum,
            Some(ClassifierError::InvalidOutput),
        ),
        (Scenario::ExtraField, Some(ClassifierError::InvalidOutput)),
        (Scenario::Score101, Some(ClassifierError::InvalidOutput)),
        (Scenario::LogprobNan, Some(ClassifierError::InvalidOutput)),
        (Scenario::LogprobInf, Some(ClassifierError::InvalidOutput)),
        (
            Scenario::LogprobsDoNotCoverClass,
            Some(ClassifierError::InvalidOutput),
        ),
        (Scenario::MissingField, Some(ClassifierError::InvalidOutput)),
        (Scenario::Safety, Some(ClassifierError::InvalidOutput)),
        (
            Scenario::EmptyCandidates,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::DifferentModelVersion,
            Some(ClassifierError::InvalidOutput),
        ),
        (Scenario::HostileText, Some(ClassifierError::InvalidOutput)),
    ];

    for (scenario, expected) in cases {
        fake.set_scenario(scenario.clone());
        let result = classifier(addr).classify(&sample()).await;
        match expected {
            None => {
                let prediction: Classification = result
                    .map_err(|error| format!("{scenario:?}: expected an answer, got {error:?}"))?;
                if prediction.bulk_score > 100 {
                    return Err(format!("{scenario:?}: score out of range"));
                }
            }
            Some(error) => {
                if result != Err(error.clone()) {
                    return Err(format!("{scenario:?}: expected {error:?}, got {result:?}"));
                }
            }
        }
    }
    Ok(())
}

/// BAKE-4 (data-not-instructions): a hostile answer text fails strict parsing
/// and nothing else is read.
#[tokio::test]
async fn bake_4_gemini_instruction_text_is_data() -> Result<(), String> {
    let fake = start().await?;
    fake.set_scenario(Scenario::HostileText);
    match classifier(fake.addr()).classify(&sample()).await {
        Err(ClassifierError::InvalidOutput) => Ok(()),
        other => Err(format!("hostile text must fail parsing, got {other:?}")),
    }
}

/// The request goes to the `us-central1` regional host and path, never the
/// global endpoint.
#[tokio::test]
async fn gemini_request_uses_regional_endpoint() -> Result<(), String> {
    let fake = start().await?;
    let egress = Arc::new(FakeHttpEgress::new());
    egress.forward(VERTEX_HOST, fake.addr());
    let cfg = GeminiConfig::new(
        PROJECT.to_owned(),
        testkit::fake_vertex::MODEL_VERSION.to_owned(),
    );
    let classifier = GeminiClassifier::new(
        cfg,
        Arc::clone(&egress) as Arc<dyn HttpEgress>,
        token_source(),
    );
    classifier
        .classify(&sample())
        .await
        .map_err(|error| format!("expected an answer, got {error:?}"))?;

    let records = egress.records();
    let record = records
        .first()
        .ok_or_else(|| "the fake egress saw no request".to_owned())?;
    if record.host != VERTEX_HOST || record.host == GLOBAL_HOST {
        return Err(format!("wrong host: {}", record.host));
    }
    if !record.path.contains("/locations/us-central1/") {
        return Err(format!("wrong region path: {}", record.path));
    }

    let seen = fake.requests();
    let request = seen
        .first()
        .ok_or_else(|| "fake-vertex saw no request".to_owned())?;
    if !request.path.contains("/locations/us-central1/") {
        return Err(format!("fake-vertex saw the wrong path: {}", request.path));
    }
    if Url::parse(PROD_BASE_URL)
        .map_err(|e| e.to_string())?
        .host_str()
        != Some(VERTEX_HOST)
    {
        return Err("PROD_BASE_URL must name the regional host".to_owned());
    }
    Ok(())
}

/// The request carries the five-class enum through `responseSchema` and asks for
/// log probabilities.
#[tokio::test]
async fn gemini_request_has_enum_and_logprobs() -> Result<(), String> {
    let fake = start().await?;
    classifier(fake.addr())
        .classify(&sample())
        .await
        .map_err(|error| format!("expected an answer, got {error:?}"))?;
    let seen = fake.requests();
    let request = seen
        .first()
        .ok_or_else(|| "fake-vertex saw no request".to_owned())?;
    let body: serde_json::Value =
        serde_json::from_str(&request.body).map_err(|error| error.to_string())?;
    let generation = body
        .get("generationConfig")
        .ok_or_else(|| "no generationConfig".to_owned())?;
    if generation
        .get("responseLogprobs")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err("responseLogprobs must be true".to_owned());
    }
    let names: Vec<String> = generation
        .pointer("/responseSchema/properties/class/enum")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "no class enum".to_owned())?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    if names != testkit::fake_vertex::CLASS_ENUM {
        return Err(format!("wrong enum: {names:?}"));
    }
    Ok(())
}

/// A response naming a different model version than pinned is rejected.
#[tokio::test]
async fn gemini_model_version_mismatch_rejected() -> Result<(), String> {
    let fake = start().await?;
    fake.set_scenario(Scenario::DifferentModelVersion);
    match classifier(fake.addr()).classify(&sample()).await {
        Err(ClassifierError::InvalidOutput) => Ok(()),
        other => Err(format!("expected InvalidOutput, got {other:?}")),
    }
}
