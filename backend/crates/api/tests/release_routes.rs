#![allow(clippy::pedantic)]

//! ASVS V13.4.2 (S10 3.2): the invite route requires both the testkit feature
//! and runtime e2e mode. Advance-clock is not implemented by this task.
//! The workspace's normal test run uses `--all-features`, so this is run
//! separately without features: `cargo test -p api --test release_routes`.
//!
//! T-1101g adds E2E-INFRA AC3 here: a release build, and a `testkit` build
//! without `MT_E2E=1`, both serve no test route and configure no extra trust
//! root. Both cases run against a child process, so the process-wide `MT_E2E`
//! one test sets cannot change another test's answer.

use api::config::ApiConfig;
use api::state::AppState;
use api::{app_state, build_router};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use obs::Sensitive;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// A per-test scratch file inside the build tree (`CARGO_TARGET_TMPDIR`).
/// Never the system temp dir: that is shared, predictable and not hermetic.
fn scratch(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

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

/// E2E-INFRA AC3 (T-1101g), release build: `MT_E2E=1` and a full e2e
/// environment cannot reach a test route, because the `testkit` route table is
/// not compiled in. The guard refuses before any e2e wiring runs, so no extra
/// trust root is configured either.
#[test]
#[cfg(not(feature = "testkit"))]
fn e2e_infra_ac3_release_and_unflagged_testkit_have_no_test_routes_or_roots(
) -> Result<(), Box<dyn std::error::Error>> {
    let cert = scratch(&format!("api-release-ca-{}.pem", std::process::id()));
    std::fs::write(&cert, b"not a certificate")?;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    cmd.env_clear()
        .env("MT_E2E", "1")
        .env("APP_ORIGIN", "https://mailtinder.test")
        .env("GOOGLE_OAUTH_CLIENT_ID", "fake-e2e-client")
        .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
        .env("UNSUB_BASE_URL", "http://127.0.0.1:8")
        .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
        .env("UNSUB_TESTBED_CA_FILE", &cert)
        .env("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
        .env("PORT", "0");
    let output = run_within_5s(&mut cmd)?;
    let text = combined(&output);
    assert!(
        !output.status.success(),
        "MT_E2E in a build without the testkit feature must exit non-zero"
    );
    assert!(
        text.contains("\"outcome\":\"failure\""),
        "start-up must log its failure through tracing"
    );
    assert!(
        !text.contains("\"outcome\":\"success\""),
        "the binary must not reach a running state"
    );
    assert!(!text.contains("panicked"), "startup must not panic");
    let _ = std::fs::remove_file(&cert);
    Ok(())
}

/// E2E-INFRA AC3 (T-1101g), `testkit` build without `MT_E2E=1`: the process
/// takes production wiring even with the whole e2e environment present
/// (including `UNSUB_TESTBED_CA_FILE` and `MT_E2E_UNSUB_DELAY_S`), so it serves
/// no test route and reads no certificate. The production configuration here is
/// deliberately incomplete (`GOOGLE_CLOUD_PROJECT` is absent), so a process
/// that wrongly took the e2e path would come up and serve instead of exiting.
///
/// The router-level half of this case — the invite route is 404 with `MT_E2E`
/// unset and with `MT_E2E=0` — is
/// `asvs_v13_4_2_invite_route_requires_runtime_e2e_mode` above.
#[test]
#[cfg(feature = "testkit")]
fn e2e_infra_ac3_release_and_unflagged_testkit_have_no_test_routes_or_roots(
) -> Result<(), Box<dyn std::error::Error>> {
    let cert = scratch(&format!("api-unflagged-ca-{}.pem", std::process::id()));
    std::fs::write(&cert, b"not a certificate")?;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    cmd.env_clear()
        .env("APP_ORIGIN", "https://mailtinder.test")
        .env("GOOGLE_OAUTH_CLIENT_ID", "fake-e2e-client")
        .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
        .env("UNSUB_BASE_URL", "http://127.0.0.1:8")
        .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
        .env("UNSUB_TESTBED_CA_FILE", &cert)
        .env("MT_E2E_UNSUB_DELAY_S", "3")
        .env("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
        .env("PORT", "0");
    let output = run_within_5s(&mut cmd)?;
    let text = combined(&output);
    assert!(
        !output.status.success(),
        "without MT_E2E the process must not take the e2e path: {text}"
    );
    assert!(
        text.contains("\"outcome\":\"failure\""),
        "start-up must log its failure through tracing"
    );
    assert!(
        !text.contains("\"outcome\":\"success\""),
        "an unflagged testkit build must never reach a running state"
    );
    assert!(!text.contains("panicked"), "startup must not panic");
    let _ = std::fs::remove_file(&cert);
    Ok(())
}

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
