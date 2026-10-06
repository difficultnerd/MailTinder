use api::{
    app_state, build_router,
    config::ApiConfig,
    error::ApiError,
    http::json::ApiJson,
    http::request_id::RequestId,
    limits::{self, LimitSubject},
};
use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use obs::Sensitive;
use serde::Deserialize;
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> Result<(api::state::AppState, testkit::Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        "https://mailtinder.test".into(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes))
}
fn router() -> Result<Router, Box<dyn std::error::Error>> {
    Ok(build_router(fixture()?.0))
}
async fn call(
    router: Router,
    method: Method,
    uri: &str,
    body: &'static str,
    content_type: Option<&str>,
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        req = req.header("content-type", ct);
    }
    Ok(router.oneshot(req.body(Body::from(body))?).await?)
}
async fn call_headers(
    router: Router,
    method: Method,
    uri: &str,
    body: &'static str,
    headers: &[(&str, &str)],
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    Ok(router.oneshot(req.body(Body::from(body))?).await?)
}
async fn problem(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 16384).await?,
    )?)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Echo {
    value: String,
}
async fn echo(ApiJson(body): ApiJson<Echo>) -> Response {
    api::http::json::json_ok(StatusCode::OK, &serde_json::json!({"value":body.value}))
}
async fn panic_route() -> StatusCode {
    panic!("secret panic should not leak")
}
async fn limit_route(
    State(state): State<api::state::AppState>,
    id: RequestId,
    headers: axum::http::HeaderMap,
) -> Response {
    let ip = api::http::client_ip::client_ip(&headers, state.config.xff_trusted_hops);
    match state
        .limits
        .check(&limits::policies::SIGN_IN_IP, LimitSubject::Ip(&ip), id)
        .await
    {
        Ok(info) => {
            let mut resp = StatusCode::NO_CONTENT.into_response();
            limits::apply_limit_headers(&mut resp, &info);
            resp
        }
        Err(e) => e.into_response(),
    }
}
fn test_router(state: api::state::AppState) -> Router {
    api::build_router_with_routes(state, |r| {
        r.route("/api/v1/__t/echo", post(echo))
            .route("/api/v1/__t/panic", get(panic_route))
            .route("/api/v1/__t/limit", get(limit_route))
    })
}

#[tokio::test]
async fn api_request_id_on_every_response_and_in_problem() -> Result<(), Box<dyn std::error::Error>>
{
    let response = call(router()?, Method::GET, "/absent", "", None).await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let id = response
        .headers()
        .get("x-request-id")
        .ok_or("missing request id")?
        .to_str()?
        .to_owned();
    let body = problem(response).await?;
    assert_eq!(body["request_id"], id);
    assert_eq!(body["code"], ApiError::NotFound.code());
    Ok(())
}
#[tokio::test]
async fn asvs_v3_4_1_hsts_on_every_response() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    for uri in ["/api/v1/healthz", "/absent", "/api/v1/__t/panic"] {
        let response = call(test_router(state.clone()), Method::GET, uri, "", None).await?;
        assert_eq!(
            response.headers()["strict-transport-security"],
            "max-age=31536000; includeSubDomains"
        );
    }
    Ok(())
}
#[tokio::test]
async fn asvs_v16_5_1_panic_gives_generic_problem() -> Result<(), Box<dyn std::error::Error>> {
    let response = call(
        test_router(fixture()?.0),
        Method::GET,
        "/api/v1/__t/panic",
        "",
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = problem(response).await?;
    assert_eq!(body["code"], "internal_error");
    assert!(!body.to_string().contains("secret panic"));
    Ok(())
}
#[tokio::test]
async fn asvs_v1_5_2_unknown_field_is_400_with_pointer_not_value(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    // The CSRF layer now guards every unsafe method (T-501), so the request
    // carries a real anonymous session, its token and the app origin.
    let (cookie, record) = api::session::store::SessionService::new(&state)
        .create_anonymous()
        .await
        .map_err(|e| format!("{e:?}"))?;
    let cookie = cookie
        .0
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    let response = call_headers(
        test_router(state),
        Method::POST,
        "/api/v1/__t/echo",
        r#"{"value":"ok","intruder":"CANARY"}"#,
        &[
            ("content-type", "application/json"),
            ("cookie", &cookie),
            ("origin", "https://mailtinder.test"),
            ("x-csrf-token", &record.record.csrf_token),
        ],
    )
    .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = problem(response).await?;
    assert_eq!(body["code"], "invalid_request");
    assert!(!body.to_string().contains("CANARY"));
    Ok(())
}
#[tokio::test]
async fn asvs_v2_4_1_sign_in_ip_limit_returns_429_with_retry_after(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    for _ in 0..20 {
        assert_eq!(
            call(
                test_router(state.clone()),
                Method::GET,
                "/api/v1/__t/limit",
                "",
                None
            )
            .await?
            .status(),
            StatusCode::NO_CONTENT
        );
    }
    let response = call(
        test_router(state),
        Method::GET,
        "/api/v1/__t/limit",
        "",
        None,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key("retry-after"));
    Ok(())
}
