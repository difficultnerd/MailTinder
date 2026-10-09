//! T-500b: process and unit tests for the `api` binary's start-up wiring.
//!
//! The process tests run the real binary with a cleared environment. `#![allow]`
//! is deliberately absent: the crate lints apply here too.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use api::config::{ApiConfig, ConfigError};

/// Run `cmd`, killing it if it has not exited within 5 seconds.
fn run_within_5s(cmd: &mut Command) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("process did not exit within 5s".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(child.wait_with_output()?)
}

/// Both streams, as lossy text.
fn combined(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn api_exits_nonzero_without_required_config() -> Result<(), Box<dyn std::error::Error>> {
    let mut first = Command::new(env!("CARGO_BIN_EXE_api"));
    first.env_clear();
    let output = run_within_5s(&mut first)?;
    assert!(!output.status.success(), "expected a non-zero exit");
    let text = combined(&output);
    assert!(!text.contains("panicked"), "startup must not panic");

    let mut second = Command::new(env!("CARGO_BIN_EXE_api"));
    second
        .env_clear()
        .env("GOOGLE_OAUTH_CLIENT_ID", "canary-client-id-123");
    let output = run_within_5s(&mut second)?;
    assert!(!output.status.success(), "expected a non-zero exit");
    let text = combined(&output);
    assert!(
        !text.contains("canary-client-id-123"),
        "a configuration value leaked into the log"
    );
    assert!(!text.contains("panicked"), "startup must not panic");
    Ok(())
}

#[test]
fn api_config_rejects_bad_app_origin() {
    for origin in [
        "https://mailtinder.test/",
        "https://mailtinder.test/api",
        "http://mailtinder.test",
        "",
    ] {
        let result = ApiConfig::from_lookup(|name| match name {
            "APP_ORIGIN" => Some(origin.to_owned()),
            "GOOGLE_OAUTH_CLIENT_ID" => Some("client-id".to_owned()),
            _ => None,
        });
        assert!(
            matches!(result, Err(ConfigError::Invalid("APP_ORIGIN"))),
            "origin {origin:?} was accepted"
        );
    }
}

#[test]
fn api_config_reads_port_default_and_override() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = |port: Option<&str>| {
        let port = port.map(str::to_owned);
        move |name: &str| match name {
            "APP_ORIGIN" => Some("https://mailtinder.test".to_owned()),
            "GOOGLE_OAUTH_CLIENT_ID" => Some("client-id".to_owned()),
            "PORT" => port.clone(),
            _ => None,
        }
    };
    assert_eq!(ApiConfig::from_lookup(lookup(None))?.port, 8080);
    assert_eq!(ApiConfig::from_lookup(lookup(Some("9000")))?.port, 9000);
    let bad = ApiConfig::from_lookup(lookup(Some("abc")));
    assert!(matches!(bad, Err(ConfigError::Invalid("PORT"))));
    Ok(())
}

#[cfg(not(feature = "testkit"))]
#[test]
fn api_e2e_env_refused_without_testkit_build() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    cmd.env_clear().env("MT_E2E", "1");
    let output = run_within_5s(&mut cmd)?;
    assert!(
        !output.status.success(),
        "MT_E2E in a build without the testkit feature must exit non-zero"
    );
    Ok(())
}

#[cfg(feature = "testkit")]
#[tokio::test]
async fn api_e2e_ports_serve_healthz() -> Result<(), Box<dyn std::error::Error>> {
    use api::startup_e2e::build_e2e_ports;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use testkit::InMemoryServerStore;
    use tower::ServiceExt;

    let store = Arc::new(InMemoryServerStore::new());
    let (ports, config) = build_e2e_ports(store, |name| match name {
        "FAKE_GOOGLE_URL" => Some("http://127.0.0.1:1".to_owned()),
        "APP_ORIGIN" => Some("http://127.0.0.1:8080".to_owned()),
        "GOOGLE_OAUTH_CLIENT_ID" => Some("e2e-client".to_owned()),
        _ => None,
    })?;
    let state = api::app_state(Arc::new(ports), Arc::new(config));
    let response = api::build_router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/healthz")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["strict-transport-security"],
        "max-age=31536000; includeSubDomains"
    );
    Ok(())
}

#[cfg(feature = "testkit")]
#[tokio::test]
async fn api_loopback_egress_allows_only_the_fake_google_socket(
) -> Result<(), Box<dyn std::error::Error>> {
    use api::startup_e2e::LoopbackEgress;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use ports::{EgressError, EgressRequest, HttpEgress, HttpMethod};
    use url::Url;

    let app = Router::new().route("/ping", get(|| async { StatusCode::OK }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let base = Url::parse(&format!("http://{addr}"))?;
    let egress = LoopbackEgress::new(&base)?;

    let allowed = egress
        .call(EgressRequest {
            method: HttpMethod::Get,
            url: base.join("ping")?,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(2),
        })
        .await?;
    assert_eq!(allowed.status, 200);

    let other_port = addr.port() + 1;
    let other_socket = Url::parse(&format!("http://127.0.0.1:{other_port}"))?;
    let refused = egress
        .call(EgressRequest {
            method: HttpMethod::Get,
            url: other_socket.join("ping")?,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(2),
        })
        .await;
    assert_eq!(refused, Err(EgressError::HostNotAllowed));

    let other_host = Url::parse("http://example.invalid/ping")?;
    let refused = egress
        .call(EgressRequest {
            method: HttpMethod::Get,
            url: other_host,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(2),
        })
        .await;
    assert_eq!(refused, Err(EgressError::HostNotAllowed));
    Ok(())
}

#[cfg(feature = "testkit")]
#[test]
fn api_testkit_build_without_e2e_env_behaves_as_production() {
    std::env::remove_var("MT_E2E");
    assert!(!api::startup::e2e_mode_enabled());
}
