//! ASVS V13.4.2: controls cannot be reached in a service without testkit.
#![cfg(not(feature = "testkit"))]

use api::{app_state, build_router, config::ApiConfig};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use obs::Sensitive;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn asvs_v13_4_2_test_routes_absent_without_testkit_feature(
) -> Result<(), Box<dyn std::error::Error>> {
    let (ports, _) = testkit::fake_ports();
    let config = ApiConfig::new(
        "https://mailtinder.test".into(),
        "synthetic-client".into(),
        Sensitive::new(b"test-rate-key".to_vec()),
        Sensitive::new(b"test-email-key".to_vec()),
    )?;
    let router = build_router(app_state(Arc::new(ports), Arc::new(config)));
    for path in ["/internal/test/advance-clock", "/internal/test/invites"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    Ok(())
}
