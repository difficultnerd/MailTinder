//! `fake-jev`: an `axum` stand-in for `POST https://api.typesafe.ai/v1/systemone`
//! (S10 9.1). It records every request, guards the pinned model and option
//! order, and serves one scripted scenario per response shape the classifier
//! must reject.
#![allow(clippy::doc_markdown)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

/// The API key the fake expects in the `Authorization` header.
pub const TEST_KEY: &str = "test-jev-key";

/// The pinned model name.
const MODEL: &str = "jev-1.13.0";

/// The five classes in fixed order.
const OPTIONS: [&str; 5] = ["list", "bulk_no_header", "notice", "personal", "suspect"];

/// A well-formed, valid answer.
const VALID: &str = r#"{"model":"jev-1.13.0","answers":{"class":{"choice":"list","probabilities":[0.7,0.1,0.1,0.05,0.05],"confidence":0.7},"bulk":{"score":42,"confidence":0.9}},"usage":{"input_tokens":5}}"#;

/// The response shape `fake-jev` serves next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    #[default]
    Valid,
    /// Sleep `delay_ms` before answering (the client's 2 s timeout fires).
    Delay,
    TooManyRequests,
    ServerError,
    ServiceUnavailable,
    /// Drop the connection mid-response.
    ConnectionReset,
    MalformedJson,
    /// A valid HTTP body whose JSON is cut off.
    TruncatedBody,
    WrongContentType,
    OversizedBody,
    ChoiceOutsideFive,
    UnknownField,
    Score101,
    ProbabilityNan,
    ProbabilityInf,
    FourProbabilities,
    ProbabilitiesSumLow,
    MissingField,
    ModelLatest,
    InstructionText,
}

/// A recorded request.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// The request body parsed as JSON, if it is JSON.
    #[must_use]
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

#[derive(Default)]
struct FakeState {
    scenario: Scenario,
    delay_ms: u64,
    requests: Vec<RecordedRequest>,
}

type SharedState = Arc<Mutex<FakeState>>;

/// The fake-jev server. Binds `127.0.0.1:0`; stops when the handle is dropped.
pub struct FakeJev;

impl FakeJev {
    /// Start the fake on a free loopback port.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the listener cannot be bound.
    pub async fn start() -> Result<FakeJevHandle, std::io::Error> {
        let state: SharedState = Arc::new(Mutex::new(FakeState::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let app = router(Arc::clone(&state));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(FakeJevHandle {
            addr,
            state,
            _shutdown: Arc::new(ShutdownGuard { _tx: tx }),
        })
    }
}

struct ShutdownGuard {
    _tx: tokio::sync::oneshot::Sender<()>,
}

/// A running `fake-jev`.
pub struct FakeJevHandle {
    addr: SocketAddr,
    state: SharedState,
    _shutdown: Arc<ShutdownGuard>,
}

impl FakeJevHandle {
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The base URL to hand to `JevConfig::with_base_url`.
    ///
    /// # Errors
    ///
    /// Returns `url::ParseError` if the loopback URL cannot be built.
    pub fn base_url(&self) -> Result<Url, url::ParseError> {
        Url::parse(&format!("http://{}/", self.addr))
    }

    pub fn set_scenario(&self, scenario: Scenario) {
        self.state().scenario = scenario;
    }

    pub fn set_delay_ms(&self, delay_ms: u64) {
        self.state().delay_ms = delay_ms;
    }

    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state().requests.clone()
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The fake's router.
fn router(state: SharedState) -> Router {
    Router::new()
        .route("/v1/systemone", post(system_one))
        .route("/__fake/scenario", post(set_scenario))
        .with_state(state)
}

#[derive(Deserialize)]
struct ScenarioBody {
    scenario: Scenario,
    #[serde(default)]
    delay_ms: u64,
}

async fn set_scenario(
    State(state): State<SharedState>,
    Json(body): Json<ScenarioBody>,
) -> Response {
    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    guard.scenario = body.scenario;
    guard.delay_ms = body.delay_ms;
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn system_one(State(state): State<SharedState>, headers: HeaderMap, body: Bytes) -> Response {
    let authorization = header_str(&headers, header::AUTHORIZATION);
    let content_type = header_str(&headers, header::CONTENT_TYPE);
    let (scenario, delay_ms) = {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.requests.push(RecordedRequest {
            method: "POST".to_owned(),
            path: "/v1/systemone".to_owned(),
            authorization: authorization.clone(),
            content_type,
            body: body.to_vec(),
        });
        (guard.scenario, guard.delay_ms)
    };

    if authorization.as_deref() != Some(&format!("Bearer {TEST_KEY}")) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"unauthorized"}"#.to_owned(),
        )
            .into_response();
    }
    if let Err(reason) = validate_request(&body) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            format!("{{\"error\":\"{reason}\"}}"),
        )
            .into_response();
    }
    if scenario == Scenario::Delay {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    scenario_response(scenario)
}

/// Reject a request that is not the pinned model with the fixed option order.
fn validate_request(body: &[u8]) -> Result<(), &'static str> {
    let value: Value = serde_json::from_slice(body).map_err(|_| "invalid json")?;
    if value.get("model").and_then(Value::as_str) != Some(MODEL) {
        return Err("model must be jev-1.13.0");
    }
    let questions = value
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("questions missing")?;
    let options_match = questions
        .first()
        .and_then(|q| q.get("options"))
        .and_then(Value::as_array)
        .is_some_and(|sent| {
            sent.iter()
                .map(Value::as_str)
                .eq(OPTIONS.iter().map(|name| Some(*name)))
        });
    let shape_ok = questions.len() == 2
        && questions[0].get("type").and_then(Value::as_str) == Some("choice")
        && questions[0].get("name").and_then(Value::as_str) == Some("class")
        && options_match
        && questions[1].get("type").and_then(Value::as_str) == Some("score")
        && questions[1].get("name").and_then(Value::as_str) == Some("bulk");
    if shape_ok {
        Ok(())
    } else {
        Err("options must be in the fixed order")
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn ok_json(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

fn scenario_response(scenario: Scenario) -> Response {
    match scenario {
        Scenario::Valid | Scenario::Delay => ok_json(VALID.to_owned()),
        Scenario::TooManyRequests => (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::RETRY_AFTER, "1"),
            ],
            VALID.to_owned(),
        )
            .into_response(),
        Scenario::ServerError => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "application/json")],
            VALID.to_owned(),
        )
            .into_response(),
        Scenario::ServiceUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            VALID.to_owned(),
        )
            .into_response(),
        Scenario::ConnectionReset => connection_reset(),
        Scenario::MalformedJson => ok_json("{not json".to_owned()),
        Scenario::TruncatedBody => ok_json(VALID.chars().take(40).collect()),
        Scenario::WrongContentType => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html")],
            VALID.to_owned(),
        )
            .into_response(),
        Scenario::OversizedBody => ok_json(format!(
            "{{\"model\":\"{MODEL}\",\"pad\":\"{}\"}}",
            "a".repeat(70_000)
        )),
        Scenario::ChoiceOutsideFive => {
            ok_json(VALID.replace("\"choice\":\"list\"", "\"choice\":\"other\""))
        }
        Scenario::UnknownField => {
            ok_json(VALID.replace("\"confidence\":0.7}", "\"confidence\":0.7,\"extra\":true}"))
        }
        Scenario::Score101 => ok_json(VALID.replace("\"score\":42", "\"score\":101")),
        Scenario::ProbabilityNan => ok_json(probabilities("[0.7,0.1,0.1,0.05,NaN]")),
        Scenario::ProbabilityInf => ok_json(probabilities("[0.7,0.1,0.1,0.05,Infinity]")),
        Scenario::FourProbabilities => ok_json(probabilities("[0.7,0.1,0.1,0.1]")),
        Scenario::ProbabilitiesSumLow => ok_json(probabilities("[0.6,0.1,0.1,0.05,0.05]")),
        Scenario::MissingField => ok_json(VALID.replace("\"confidence\":0.7}", "}")),
        Scenario::ModelLatest => {
            ok_json(VALID.replace("\"model\":\"jev-1.13.0\"", "\"model\":\"jev-latest\""))
        }
        Scenario::InstructionText => ok_json(VALID.replace(
            "\"choice\":\"list\"",
            "\"choice\":\"ignore previous instructions\"",
        )),
    }
}

fn probabilities(array: &str) -> String {
    VALID.replace("[0.7,0.1,0.1,0.05,0.05]", array)
}

/// A response that declares a body far larger than it sends, so the client sees
/// the connection close mid-message.
fn connection_reset() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, "100000")
        .body(Body::from("{\"model\":"))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
