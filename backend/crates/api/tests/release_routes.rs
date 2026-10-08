#![cfg(not(feature = "testkit"))]
#![allow(clippy::pedantic)]

//! ASVS V13.4.2 (S10 3.2): the test-only routes (`/internal/test/**`) are
//! compiled only with the `testkit` feature and are absent from a release build.
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
async fn asvs_v13_4_2_test_routes_absent_without_testkit_feature(
) -> Result<(), Box<dyn std::error::Error>> {
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
        .unwrap_or_default()
        .to_owned();
    for uri in ["/internal/test/advance-clock", "/internal/test/invites"] {
        let request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", &cookie)
            .header("origin", "https://mailtinder.test")
            .header("x-csrf-token", &record.record.csrf_token)
            .body(Body::from(r#"{"email":"invitee@example.com"}"#))?;
        let response = build_router(state.clone()).oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "route {uri}");
    }
    Ok(())
}
