//! T-500b: process and unit tests for the `api` binary's start-up wiring.
//!
//! The process tests run the real binary with a cleared environment. `#![allow]`
//! is deliberately absent: the crate lints apply here too.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use api::config::{ApiConfig, ConfigError, Mode};

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
    ] {
        let result = ApiConfig::from_lookup(
            |name| match name {
                "APP_ORIGIN" => Some(origin.to_owned()),
                "GOOGLE_OAUTH_CLIENT_ID" => Some("client-id".to_owned()),
                _ => None,
            },
            Mode::Production,
        );
        assert!(
            matches!(result, Err(ConfigError::Invalid("APP_ORIGIN"))),
            "origin {origin:?} was accepted"
        );
    }
}

/// F3: an `http` origin is refused in production, loopback or not.
#[test]
fn api_production_rejects_http_app_origin() {
    for origin in [
        "http://localhost",
        "http://127.0.0.1",
        "http://127.0.0.1:8080",
        "http://[::1]",
        "http://mailtinder.test",
        "http://10.0.0.1",
    ] {
        let result = ApiConfig::from_lookup(
            |name| match name {
                "APP_ORIGIN" => Some(origin.to_owned()),
                "GOOGLE_OAUTH_CLIENT_ID" => Some("client-id".to_owned()),
                _ => None,
            },
            Mode::Production,
        );
        assert!(
            matches!(result, Err(ConfigError::Invalid("APP_ORIGIN"))),
            "origin {origin:?} was accepted in production"
        );
    }
}

/// F3: only loopback `http` is accepted, and only in e2e mode.
#[test]
fn api_e2e_accepts_loopback_http_app_origin_only() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = |origin: &str| {
        let origin = origin.to_owned();
        move |name: &str| match name {
            "APP_ORIGIN" => Some(origin.clone()),
            "GOOGLE_OAUTH_CLIENT_ID" => Some("e2e-client".to_owned()),
            _ => None,
        }
    };
    for origin in [
        "http://127.0.0.1:8080",
        "http://localhost",
        "http://[::1]:9",
        "https://mailtinder.test",
    ] {
        let config = ApiConfig::from_lookup(lookup(origin), Mode::E2e)?;
        assert_eq!(config.app_origin, origin);
    }
    for origin in [
        "http://example.com",
        "http://10.0.0.1",
        "http://192.168.1.1",
    ] {
        let result = ApiConfig::from_lookup(lookup(origin), Mode::E2e);
        assert!(
            matches!(result, Err(ConfigError::Invalid("APP_ORIGIN"))),
            "origin {origin:?} was accepted in e2e"
        );
    }
    Ok(())
}

/// F4: e2e never listens on anything but loopback; production defaults to all
/// interfaces and stays configurable.
#[test]
fn api_e2e_binds_loopback_only() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = |bind: Option<&str>| {
        let bind = bind.map(str::to_owned);
        move |name: &str| match name {
            "APP_ORIGIN" => Some("http://127.0.0.1:8080".to_owned()),
            "GOOGLE_OAUTH_CLIENT_ID" => Some("e2e-client".to_owned()),
            "API_BIND_HOST" => bind.clone(),
            _ => None,
        }
    };
    let config = ApiConfig::from_lookup(lookup(None), Mode::E2e)?;
    assert_eq!(config.bind_host, std::net::IpAddr::from([127, 0, 0, 1]));

    let refused = ApiConfig::from_lookup(lookup(Some("0.0.0.0")), Mode::E2e);
    assert!(matches!(
        refused,
        Err(ConfigError::Invalid("API_BIND_HOST"))
    ));
    Ok(())
}

/// F4: production binds every interface by default and honours `API_BIND_HOST`.
#[test]
fn api_production_bind_host_defaults_to_all_interfaces_and_is_configurable(
) -> Result<(), Box<dyn std::error::Error>> {
    let lookup = |bind: Option<&str>| {
        let bind = bind.map(str::to_owned);
        move |name: &str| match name {
            "APP_ORIGIN" => Some("https://mailtinder.test".to_owned()),
            "GOOGLE_OAUTH_CLIENT_ID" => Some("client-id".to_owned()),
            "API_BIND_HOST" => bind.clone(),
            _ => None,
        }
    };
    let default = ApiConfig::from_lookup(lookup(None), Mode::Production)?;
    assert_eq!(default.bind_host, std::net::IpAddr::from([0, 0, 0, 0]));

    let pinned = ApiConfig::from_lookup(lookup(Some("127.0.0.1")), Mode::Production)?;
    assert_eq!(pinned.bind_host, std::net::IpAddr::from([127, 0, 0, 1]));
    Ok(())
}

/// F5: every required variable, blank or absent, is `Missing(name)`.
#[test]
fn api_empty_required_variable_is_missing() {
    for name in ["APP_ORIGIN", "GOOGLE_OAUTH_CLIENT_ID"] {
        for value in ["", "   "] {
            let result = ApiConfig::from_lookup(
                move |key: &str| match key {
                    "APP_ORIGIN" => Some(if name == "APP_ORIGIN" {
                        value.to_owned()
                    } else {
                        "https://mailtinder.test".to_owned()
                    }),
                    "GOOGLE_OAUTH_CLIENT_ID" => Some(if name == "GOOGLE_OAUTH_CLIENT_ID" {
                        value.to_owned()
                    } else {
                        "client-id".to_owned()
                    }),
                    _ => None,
                },
                Mode::Production,
            );
            assert!(
                matches!(result, Err(ConfigError::Missing(n)) if n == name),
                "blank {name} was not Missing"
            );
        }
    }
}

/// F6: a project id that could break a resource name is refused.
#[test]
fn api_rejects_invalid_project_id() {
    use api::startup::validate_project_id;

    let too_long = "a".repeat(31);
    for bad in [
        "UPPERCASE",
        "abc",
        "ab12",
        "with/slash",
        "project..id",
        "-leading",
        "trailing-",
        too_long.as_str(),
    ] {
        assert!(
            validate_project_id(bad).is_err(),
            "project id {bad:?} was accepted"
        );
    }
    for good in ["demo-mailtinder", "my-project-123", "abcde1"] {
        assert!(
            validate_project_id(good).is_ok(),
            "project id {good:?} was refused"
        );
    }
}

/// N3: no coverage artefact is tracked by git.
#[test]
fn api_no_profraw_tracked() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--", "*.profraw"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        // Not a git checkout (for example a vendored copy): nothing to check.
        return;
    }
    let tracked = String::from_utf8_lossy(&output.stdout);
    assert!(
        tracked.trim().is_empty(),
        "a *.profraw file is tracked by git: {tracked}"
    );
}

/// F7: the two canaries given to a failing start-up are read, then never
/// logged.
#[test]
fn api_log_canary_value_is_read_then_never_logged() -> Result<(), Box<dyn std::error::Error>> {
    const FIRST: &str = "canary-client-id-read-then-hidden";
    const SECOND: &str = "CanaryProjectValueThatIsInvalid";

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    cmd.env_clear()
        .env("APP_ORIGIN", "https://mailtinder.example.com")
        .env("GOOGLE_OAUTH_CLIENT_ID", FIRST)
        .env("GOOGLE_CLOUD_PROJECT", SECOND);

    let output = run_within_5s(&mut cmd)?;
    assert!(!output.status.success(), "expected a non-zero exit");
    let text = combined(&output);
    assert!(
        !text.contains(FIRST),
        "the client id canary leaked into the log"
    );
    assert!(
        !text.contains(SECOND),
        "the invalid-value canary leaked into the log"
    );
    assert!(!text.contains("panicked"), "startup must not panic");
    Ok(())
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
    assert_eq!(
        ApiConfig::from_lookup(lookup(None), Mode::Production)?.port,
        8080
    );
    assert_eq!(
        ApiConfig::from_lookup(lookup(Some("9000")), Mode::Production)?.port,
        9000
    );
    let bad = ApiConfig::from_lookup(lookup(Some("abc")), Mode::Production);
    assert!(matches!(bad, Err(ConfigError::Invalid("PORT"))));
    Ok(())
}

#[cfg(not(feature = "testkit"))]
#[test]
fn api_e2e_env_refused_without_testkit_build() -> Result<(), Box<dyn std::error::Error>> {
    // A complete, otherwise-valid production environment plus `MT_E2E=1`.
    // Without the refusal guard, production wiring would run from this
    // environment, so make that observable: point the GCP HTTP clients at a
    // local socket that is never answered. A production start then blocks and
    // cannot exit within the deadline; the guard must refuse before any of
    // that. (A missing-variable environment would let production exit for the
    // wrong reason and hide the guard, which is the point of this test.)
    let stall = std::net::TcpListener::bind("127.0.0.1:0")?;
    let proxy = format!("http://{}", stall.local_addr()?);

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    cmd.env_clear()
        .env("MT_E2E", "1")
        .env("GOOGLE_CLOUD_PROJECT", "demo-project")
        .env("APP_ORIGIN", "https://mailtinder.example.com")
        .env("GOOGLE_OAUTH_CLIENT_ID", "client-id")
        .env("UNSUB_BASE_URL", "https://unsub.example.com")
        .env("UNSUB_AUDIENCE", "https://unsub.example.com")
        .env(
            "UNSUB_TASKS_CALLER",
            "caller@example.iam.gserviceaccount.com",
        )
        .env("HTTP_PROXY", proxy.clone())
        .env("HTTPS_PROXY", proxy);

    let output = run_within_5s(&mut cmd)?;
    assert!(
        !output.status.success(),
        "MT_E2E in a build without the testkit feature must exit non-zero"
    );
    let text = combined(&output);
    assert!(
        text.contains("\"outcome\":\"failure\""),
        "start-up must log its failure through tracing"
    );
    assert!(
        !text.contains("\"outcome\":\"success\""),
        "the binary must not reach a running state"
    );
    assert!(!text.contains("panicked"), "startup must not panic");
    drop(stall);
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
    // Standalone OAuth and Gmail/Drive must agree on token expiry; a frozen
    // unit-test epoch makes freshly issued provider tokens immediately stale.
    let provider_now = adapters_gcp::production_clock().now();
    assert!((ports.clock.now() - provider_now).abs() < time::Duration::seconds(5));
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

/// F9: construction refuses a non-loopback literal, and the response body read
/// is capped at 1 MiB.
#[cfg(feature = "testkit")]
#[tokio::test]
async fn api_loopback_egress_rejects_non_loopback_ip_and_caps_body(
) -> Result<(), Box<dyn std::error::Error>> {
    use api::startup::SetupError;
    use api::startup_e2e::LoopbackEgress;
    use axum::routing::get;
    use axum::Router;
    use ports::{EgressError, EgressRequest, HttpEgress, HttpMethod};
    use url::Url;

    for literal in [
        "http://8.8.8.8:80",
        "http://10.0.0.1:9",
        "http://[2001:db8::1]:9",
    ] {
        let refused = LoopbackEgress::new(&Url::parse(literal)?);
        assert!(
            matches!(refused, Err(SetupError::Invalid("FAKE_GOOGLE_URL"))),
            "non-loopback {literal} was accepted"
        );
    }
    // The bracketed loopback form is accepted at construction.
    assert!(LoopbackEgress::new(&Url::parse("http://[::1]:9")?).is_ok());

    let app = Router::new().route("/big", get(|| async { vec![b'x'; 1024 * 1024 + 1] }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = Url::parse(&format!("http://{addr}"))?;
    let egress = LoopbackEgress::new(&base)?;
    let refused = egress
        .call(EgressRequest {
            method: HttpMethod::Get,
            url: base.join("big")?,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(2),
        })
        .await;
    assert_eq!(refused, Err(EgressError::ResponseTooLarge));
    Ok(())
}

/// T-1108c: with `MT_PUBLIC_BASE_URL` set the browser-facing authorisation URL
/// is the tunnel's `${MT_PUBLIC_BASE_URL}/fake-google/...` (the one path
/// `e2e_host.py` proxies to the loopback fake-google); unset it is exactly the
/// loopback fake-google, unchanged.
#[cfg(feature = "testkit")]
#[test]
fn demo_phone_signin_goes_through_the_front_door() -> Result<(), Box<dyn std::error::Error>> {
    use api::startup::SetupError;
    use api::startup_e2e::{public_auth_endpoint, public_base_url};
    use url::Url;

    let fake_google = Url::parse("http://127.0.0.1:4010")?;
    // Unset: the loopback fake-google, exactly as before.
    assert_eq!(
        public_auth_endpoint(&fake_google, None)?.as_str(),
        "http://127.0.0.1:4010/o/oauth2/v2/auth"
    );
    // Blank is treated as unset.
    assert!(public_base_url(&|_| Some("   ".to_owned()))?.is_none());

    // Set: the browser is sent to the public tunnel origin at the proxied path.
    let public = Url::parse("https://demo-phone.trycloudflare.com")?;
    assert_eq!(
        public_auth_endpoint(&fake_google, Some(&public))?.as_str(),
        "https://demo-phone.trycloudflare.com/fake-google/o/oauth2/v2/auth"
    );
    let lookup = |name: &str| {
        if name == "MT_PUBLIC_BASE_URL" {
            Some("https://demo-phone.trycloudflare.com".to_owned())
        } else {
            None
        }
    };
    assert_eq!(
        public_base_url(&lookup)?
            .map(|url| url.to_string())
            .as_deref(),
        Some("https://demo-phone.trycloudflare.com/")
    );
    // A value that is not a URL is refused, not silently ignored.
    assert!(matches!(
        public_base_url(&|_| Some("not a url".to_owned())),
        Err(SetupError::Invalid("MT_PUBLIC_BASE_URL"))
    ));
    Ok(())
}
