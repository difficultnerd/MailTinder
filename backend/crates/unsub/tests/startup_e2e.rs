//! T-1112a: tests for the `unsub` e2e start-up wiring.
//!
//! The process test runs the real binary with a cleared environment. `#![allow]`
//! is deliberately absent: the crate lints apply here too.

#[cfg(not(feature = "testkit"))]
mod no_testkit {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Run `cmd`, killing it if it has not exited within 5 seconds.
    fn run_within_5s(
        cmd: &mut Command,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
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

    /// A complete, otherwise-valid production environment plus `MT_E2E=1`.
    /// Without the refusal guard, production wiring would run from this
    /// environment, so make that observable: point the GCP HTTP clients at a
    /// local socket that is never answered. A production start then blocks and
    /// cannot exit within the deadline; the guard must refuse before any of
    /// that. (A missing-variable environment would let production exit for the
    /// wrong reason and hide the guard, which is the point of this test.)
    #[test]
    fn unsub_e2e_mode_refused_without_testkit_build() -> Result<(), Box<dyn std::error::Error>> {
        let stall = std::net::TcpListener::bind("127.0.0.1:0")?;
        let proxy = format!("http://{}", stall.local_addr()?);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_unsub"));
        cmd.env_clear()
            .env("MT_E2E", "1")
            .env("GOOGLE_CLOUD_PROJECT", "demo-project")
            .env("UNSUB_BASE_URL", "https://unsub.example.com")
            .env("UNSUB_AUDIENCE", "https://unsub.example.com")
            .env(
                "UNSUB_TASKS_CALLER",
                "caller@example.iam.gserviceaccount.com",
            )
            .env("GOOGLE_OAUTH_CLIENT_ID", "client-id")
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
}

#[cfg(feature = "testkit")]
mod e2e {
    use std::net::{IpAddr, SocketAddr};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use obs::Sensitive;
    use ports::CallerVerifier;
    use testkit::InMemoryServerStore;
    use tower::ServiceExt;
    use unsub::runner::UnsubState;
    use unsub::startup_e2e::{self, E2eConfig, E2eError, StaticTokenVerifier};

    /// A token of exactly the accepted minimum length.
    const TOKEN: &str = "test-caller-token-0123456789";

    /// A variable lookup for a synthetic e2e environment. `token` is the value
    /// of `UNSUB_E2E_CALLER_TOKEN`; `None` leaves it unset.
    fn lookup(token: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let token = token.map(str::to_owned);
        move |name: &str| match name {
            "UNSUB_E2E_CALLER_TOKEN" => token.clone(),
            "FAKE_GOOGLE_URL" => Some("http://127.0.0.1:9".to_owned()),
            "UNSUB_TESTBED_URL" => Some("http://127.0.0.1:8".to_owned()),
            "PORT" => Some("0".to_owned()),
            _ => None,
        }
    }

    /// Build e2e state over an in-memory store with `token`.
    fn build(token: &str) -> Result<(UnsubState, E2eConfig), E2eError> {
        let store = Arc::new(InMemoryServerStore::new());
        startup_e2e::build_e2e_ports(store, lookup(Some(token)))
    }

    /// `POST path` through `router` with an optional `Authorization` header.
    async fn status_of(
        router: &axum::Router,
        path: &str,
        auth: Option<&str>,
    ) -> Result<StatusCode, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("POST").uri(path);
        if let Some(value) = auth {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        let response = router.clone().oneshot(builder.body(Body::empty())?).await?;
        Ok(response.status())
    }

    /// Behaviour 1: e2e binds loopback only and serves its health route.
    #[tokio::test]
    async fn unsub_e2e_serves_health_on_loopback_only() -> Result<(), Box<dyn std::error::Error>> {
        let (state, config) = build(TOKEN)?;
        assert_eq!(config.bind.ip(), IpAddr::from([127, 0, 0, 1]));
        assert!(
            config.bind.ip().is_loopback(),
            "e2e must listen on loopback only"
        );

        let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.bind.ip(), 0)).await?;
        let addr = listener.local_addr()?;
        assert!(addr.ip().is_loopback());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, startup_e2e::e2e_router(state)).await;
        });

        let response =
            reqwest::get(format!("http://{addr}{}", startup_e2e::E2E_HEALTH_PATH)).await?;
        assert_eq!(response.status(), 200);
        server.abort();
        Ok(())
    }

    /// Behaviour 2: the verifier accepts only the configured test token.
    #[tokio::test]
    async fn unsub_e2e_verifier_accepts_only_the_test_token(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (state, _config) = build(TOKEN)?;
        let router = startup_e2e::e2e_router(state);
        let path = format!("/internal/v1/unsubscribe-jobs/{}/run", uuid::Uuid::nil());

        let accepted = status_of(&router, &path, Some(&format!("Bearer {TOKEN}"))).await?;
        assert_eq!(
            accepted,
            StatusCode::OK,
            "the configured test token must be accepted"
        );

        for bad in [
            "Bearer wrong-token-0123456789",
            "Bearer ",
            "Bearer",
            "",
            "Basic test-caller-token-0123456789",
        ] {
            let refused = status_of(&router, &path, Some(bad)).await?;
            assert_eq!(
                refused,
                StatusCode::UNAUTHORIZED,
                "Authorization {bad:?} was accepted"
            );
        }
        let absent = status_of(&router, &path, None).await?;
        assert_eq!(absent, StatusCode::UNAUTHORIZED);
        Ok(())
    }

    /// Behaviour 2 (start-up half): an unset, empty or short token refuses the
    /// start, and an empty `Bearer ` header must never match.
    #[tokio::test]
    async fn unsub_e2e_refuses_empty_unset_or_short_token() -> Result<(), Box<dyn std::error::Error>>
    {
        // Unset and blank are missing.
        for token in [None, Some(""), Some("   ")] {
            let store = Arc::new(InMemoryServerStore::new());
            let result = startup_e2e::build_e2e_ports(store, lookup(token));
            assert!(
                matches!(
                    result,
                    Err(E2eError::Missing(n)) if n == startup_e2e::E2E_CALLER_TOKEN_ENV
                ),
                "token {token:?} must be reported missing"
            );
        }

        // Fifteen bytes is short; sixteen is accepted.
        for token in ["short", "0123456789abcde"] {
            let store = Arc::new(InMemoryServerStore::new());
            let result = startup_e2e::build_e2e_ports(store, lookup(Some(token)));
            assert!(
                matches!(
                    result,
                    Err(E2eError::Invalid(n)) if n == startup_e2e::E2E_CALLER_TOKEN_ENV
                ),
                "token of length {} must be refused",
                token.len()
            );
        }
        let (state, _config) = build("0123456789abcdef")?;

        // An empty presented token never matches.
        let verifier = StaticTokenVerifier::new(
            Sensitive::new(TOKEN.to_owned()),
            startup_e2e::E2E_CALLER_EMAIL.to_owned(),
            startup_e2e::E2E_AUDIENCE.to_owned(),
        );
        assert!(verifier
            .verify(&Sensitive::new(String::new()), startup_e2e::E2E_AUDIENCE)
            .await
            .is_err());

        // Nor does an empty `Bearer ` header through the route.
        let router = startup_e2e::e2e_router(state);
        let path = format!("/internal/v1/unsubscribe-jobs/{}/run", uuid::Uuid::nil());
        let refused = status_of(&router, &path, Some("Bearer ")).await?;
        assert_eq!(refused, StatusCode::UNAUTHORIZED);
        Ok(())
    }

    /// Behaviour 3: a testkit build without `MT_E2E=1` never enables e2e, so
    /// production wiring stays the only path.
    #[test]
    fn unsub_testkit_build_without_e2e_env_behaves_as_production() {
        std::env::remove_var("MT_E2E");
        assert!(!startup_e2e::e2e_mode_enabled());
    }
}
