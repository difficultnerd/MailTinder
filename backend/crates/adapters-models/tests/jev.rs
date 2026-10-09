//! Contract suite and failure-scenario tests for the Jev classifier (T-905).
//!
//! CL-03 AC2 (errors and timeouts are failures; the card is unaffected) and
//! BAKE-4 (every S10 9.1 Jev scenario maps to the right error).
#![allow(clippy::too_many_lines, clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use adapters_models::jev::{JevClassifier, JevConfig};
use async_trait::async_trait;
use obs::Sensitive;
use ports::{
    Classifier, ClassifierError, ClassifierInput, EgressError, EgressRequest, EgressResponse,
    HttpEgress, OneClickOutcome,
};
use testkit::contract::classifier::classifier_contract;
use testkit::fake_jev::{FakeJev, FakeJevHandle, Scenario, TEST_KEY};
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXPECTED_OPTIONS: [&str; 5] = ["list", "bulk_no_header", "notice", "personal", "suspect"];

fn sample_input() -> ClassifierInput {
    ClassifierInput {
        from_display: "News".to_owned(),
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
        input_version: "1",
    }
}

/// A classifier pointed at `fake-jev`: the base URL override exists only in
/// test builds (`with_base_url`), and this egress forwards to the fake.
fn jev(fake: &FakeJevHandle, key: &str) -> Result<Arc<dyn Classifier>, url::ParseError> {
    let config = JevConfig::with_base_url(fake.base_url()?);
    let egress = Arc::new(ForwardEgress::new(fake.addr()));
    Ok(Arc::new(JevClassifier::new(
        config,
        egress,
        Sensitive::new(key.to_owned()),
    )))
}

/// A pass-through egress to `fake-jev` that distinguishes a client timeout.
struct ForwardEgress {
    addr: SocketAddr,
}

impl ForwardEgress {
    fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }
}

#[async_trait]
impl HttpEgress for ForwardEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(req.timeout)
            .build()
            .map_err(|_| EgressError::Connect)?;
        let target = format!("http://{}{}", self.addr, req.url.path());
        let mut builder = client.post(&target);
        for (name, value) in &req.headers {
            builder = builder.header(name, value.expose());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                EgressError::Timeout
            } else {
                EgressError::Connect
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|_| EgressError::Connect)?
            .to_vec();
        Ok(EgressResponse {
            status,
            headers,
            body,
        })
    }
}

/// An egress that records the request and returns one fixed error.
struct RecordingEgress {
    error: EgressError,
    seen: Mutex<Option<EgressRequest>>,
}

impl RecordingEgress {
    fn new(error: EgressError) -> Self {
        Self {
            error,
            seen: Mutex::new(None),
        }
    }

    fn take(&self) -> Option<EgressRequest> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

#[async_trait]
impl HttpEgress for RecordingEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(req);
        Err(self.error.clone())
    }
}

/// The `Classifier` contract suite, run against `JevClassifier`/`fake-jev`.
#[tokio::test]
async fn classifier_contract_jev_fake_jev() -> TestResult {
    let fake = FakeJev::start().await?;
    let classifier = jev(&fake, TEST_KEY)?;
    classifier_contract(move || Arc::clone(&classifier))
        .await
        .map_err(std::io::Error::other)?;
    Ok(())
}

/// CL-03 AC2: a 2-second egress timeout is a recorded `Timeout`, and the
/// request carries the pinned host, path and 2-second deadline.
#[tokio::test(start_paused = true)]
async fn cl_03_ac2_jev_timeout_recorded() -> TestResult {
    let egress = Arc::new(RecordingEgress::new(EgressError::Timeout));
    let config = JevConfig::production()?;
    let classifier = JevClassifier::new(
        config,
        Arc::clone(&egress) as Arc<dyn HttpEgress>,
        Sensitive::new("key-not-used".to_owned()),
    );
    let result = classifier.classify(&sample_input()).await;
    assert_eq!(result, Err(ClassifierError::Timeout));

    let request = egress.take().ok_or("no request recorded")?;
    assert_eq!(request.timeout, Duration::from_secs(2));
    assert_eq!(request.url.as_str(), "https://api.typesafe.ai/v1/systemone");
    assert_eq!(request.method, ports::HttpMethod::Post);
    Ok(())
}

/// BAKE-4: every S10 9.1 Jev scenario maps to the right error.
#[tokio::test]
async fn bake_4_jev_failure_scenarios() -> TestResult {
    let fake = FakeJev::start().await?;
    let cases: &[(Scenario, Option<ClassifierError>)] = &[
        (Scenario::Valid, None),
        (Scenario::Delay, Some(ClassifierError::Timeout)),
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
            Scenario::WrongContentType,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::OversizedBody,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::ChoiceOutsideFive,
            Some(ClassifierError::InvalidOutput),
        ),
        (Scenario::UnknownField, Some(ClassifierError::InvalidOutput)),
        (Scenario::Score101, Some(ClassifierError::InvalidOutput)),
        (
            Scenario::ProbabilityNan,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::ProbabilityInf,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::FourProbabilities,
            Some(ClassifierError::InvalidOutput),
        ),
        (
            Scenario::ProbabilitiesSumLow,
            Some(ClassifierError::InvalidOutput),
        ),
        (Scenario::MissingField, Some(ClassifierError::InvalidOutput)),
        (Scenario::ModelLatest, Some(ClassifierError::InvalidOutput)),
        (
            Scenario::InstructionText,
            Some(ClassifierError::InvalidOutput),
        ),
    ];
    for (scenario, expected) in cases {
        fake.set_scenario(*scenario);
        fake.set_delay_ms(3_000);
        let classifier = jev(&fake, TEST_KEY)?;
        let result = classifier.classify(&sample_input()).await;
        match expected {
            None => assert!(
                result.is_ok(),
                "scenario {scenario:?} should pass: {result:?}"
            ),
            Some(error) => assert_eq!(
                result,
                Err(error.clone()),
                "scenario {scenario:?} mapped wrong"
            ),
        }
    }
    Ok(())
}

/// BAKE-4: a hostile answer is parsed only as typed fields, so an instruction
/// string in `choice` is rejected and nothing else happens.
#[tokio::test]
async fn bake_4_jev_instruction_text_is_data() -> TestResult {
    let fake = FakeJev::start().await?;
    fake.set_scenario(Scenario::InstructionText);
    let classifier = jev(&fake, TEST_KEY)?;
    let result = classifier.classify(&sample_input()).await;
    assert_eq!(result, Err(ClassifierError::InvalidOutput));

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let body = requests[0].json().ok_or("request body was not JSON")?;
    assert_eq!(
        body["state"],
        ports::prompt::render_model_text(&sample_input())
    );
    Ok(())
}

/// The request pins the model, the fixed option order and the two questions.
#[tokio::test]
async fn jev_request_pins_model_and_option_order() -> TestResult {
    let fake = FakeJev::start().await?;
    let classifier = jev(&fake, TEST_KEY)?;
    assert!(classifier.classify(&sample_input()).await.is_ok());

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/systemone");
    assert_eq!(request.content_type.as_deref(), Some("application/json"));
    assert_eq!(request.authorization, Some(format!("Bearer {TEST_KEY}")));

    let body = request.json().ok_or("request body was not JSON")?;
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(
        body["state"],
        ports::prompt::render_model_text(&sample_input())
    );
    let questions = body["questions"]
        .as_object()
        .ok_or("questions not an object")?;
    assert_eq!(questions.len(), 2);

    let class = &questions["class"];
    assert_eq!(class["type"], "choice");
    assert_eq!(class["instructions"], ports::prompt::CLASS_QUESTION);
    let criteria = class["criteria"]
        .as_object()
        .ok_or("class criteria not an object")?;
    assert_eq!(criteria.len(), EXPECTED_OPTIONS.len());
    for name in EXPECTED_OPTIONS {
        assert!(criteria.contains_key(name), "missing option {name}");
    }
    assert_eq!(
        criteria["list"],
        ports::prompt::CLASS_OPTIONS[0].1,
        "the criteria map must carry each option's rubric description"
    );

    let bulk = &questions["bulk"];
    assert_eq!(bulk["type"], "score");
    assert_eq!(bulk["instructions"], ports::prompt::BULK_QUESTION);
    assert_eq!(bulk["criteria"].as_array().map(Vec::len), Some(2));

    // A JSON object loses key order when parsed, so read the criteria order
    // from the serialised bytes: the five options must appear in the fixed
    // CLASS_OPTIONS order (the fake also rejects a request that does not).
    let raw = String::from_utf8_lossy(&request.body);
    let mut cursor = 0;
    for name in EXPECTED_OPTIONS {
        let needle = format!("\"{name}\":");
        let at = raw[cursor..]
            .find(&needle)
            .ok_or_else(|| format!("option {name} not in the fixed order"))?;
        cursor += at + needle.len();
    }
    Ok(())
}

/// The API key never reaches a log line.
#[tokio::test]
async fn jev_key_never_logged() -> TestResult {
    let fake = FakeJev::start().await?;
    let canary = "CANARY-JEV-KEY-9f31";
    let (sink, _guard) = obs::capture("adapters-models", obs::arc(obs::FixedClock::default()));
    let classifier = jev(&fake, canary)?;
    let _ = classifier.classify(&sample_input()).await;
    let text = sink.text();
    let leaks = obs::scan_for_leaks(&text, &[canary.to_owned()]);
    assert!(leaks.is_empty(), "log leaked the key: {leaks:?}");
    Ok(())
}
