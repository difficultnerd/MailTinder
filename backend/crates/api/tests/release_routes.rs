#![allow(clippy::pedantic)]

//! ASVS V13.4.2 (S10 3.2): the invite route requires both the testkit feature
//! and runtime e2e mode. Advance-clock is not implemented by this task.
//! The workspace's normal test run uses `--all-features`, so this is run
//! separately without features: `cargo test -p api --test release_routes`.

use api::config::ApiConfig;
use api::state::AppState;
use api::{app_state, build_router};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use obs::Sensitive;
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> Result<AppState, Box<dyn std::error::Error>> {
    let (ports, _fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        "https://mailtinder.test".into(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?;
    Ok(app_state(Arc::new(ports), Arc::new(config)))
}

#[tokio::test]
#[cfg(not(feature = "testkit"))]
async fn asvs_v13_4_2_test_routes_absent_without_testkit_feature(
) -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("MT_E2E", "1");
    assert_invite_route(StatusCode::NOT_FOUND).await
}

#[tokio::test]
#[cfg(feature = "testkit")]
async fn asvs_v13_4_2_invite_route_requires_runtime_e2e_mode(
) -> Result<(), Box<dyn std::error::Error>> {
    std::env::remove_var("MT_E2E");
    assert_invite_route(StatusCode::NOT_FOUND).await?;
    std::env::set_var("MT_E2E", "0");
    assert_invite_route(StatusCode::NOT_FOUND).await?;
    std::env::set_var("MT_E2E", "1");
    assert_invite_route(StatusCode::CREATED).await?;
    std::env::remove_var("MT_E2E");
    Ok(())
}

async fn assert_invite_route(expected: StatusCode) -> Result<(), Box<dyn std::error::Error>> {
    let state = fixture()?;
    // A real anonymous session satisfies the CSRF layer, so a missing route
    // reaches the 404 fallback rather than being refused with 403 first.
    let (cookie, record) = api::session::store::SessionService::new(&state)
        .create_anonymous()
        .await
        .map_err(|e| format!("{e:?}"))?;
    let cookie = cookie
        .0
        .to_str()?
        .split(';')
        .next()
        .ok_or("anonymous session cookie is empty")?
        .to_owned();
    let uri = "/internal/test/invites";
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", &cookie)
        .header("origin", "https://mailtinder.test")
        .header("x-csrf-token", &record.record.csrf_token)
        .body(Body::from(r#"{"email":"invitee@example.com"}"#))?;
    let response = build_router(state.clone()).oneshot(request).await?;
    assert_eq!(response.status(), expected, "route {uri}");
    Ok(())
}
