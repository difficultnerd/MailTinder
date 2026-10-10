//! T-1112a: tests for the `unsub` e2e start-up wiring.
//!
//! The process tests run the real binary with a cleared environment. `#![allow]`
//! is deliberately absent: the crate lints apply here too.

/// A per-test scratch file inside the build tree (`CARGO_TARGET_TMPDIR`).
/// Never the system temp dir: that is shared, predictable and not hermetic.
fn scratch(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[cfg(not(feature = "testkit"))]
mod no_testkit {
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

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
        // The refusal is the only quick, non-panicking exit available from this
        // environment: the proxy points at a never-answered socket, so if the
        // guard were missing, production wiring would block past the deadline
        // and `run_within_5s` would already have failed above.
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

    /// ASVS V13.4.2 (T-1101g): a release `unsub` has no test routes.
    ///
    /// The e2e router (`/healthz` beside the internal route) lives in
    /// `unsub::startup_e2e`, which this build does not compile
    /// (`#[cfg(feature = "testkit")]`); the production router serves exactly
    /// the one internal route, so the e2e liveness path is a 404. The guard
    /// test above shows the same binary refuses `MT_E2E=1` outright, so even
    /// the environment cannot reach a test route.
    #[tokio::test]
    async fn asvs_v13_4_2_unsub_release_has_no_test_routes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (ports, _fakes) = testkit::fake_ports();
        let state = unsub::runner::UnsubState {
            ports,
            senders: vec![Arc::new(unsub::one_click::OneClickSender)],
            auth: svc_common::internal_auth::InternalAuthConfig {
                audience: "unsub-e2e".to_owned(),
                allowed_caller_email: "e2e-caller@mailtinder.invalid".to_owned(),
            },
        };

        for path in ["/healthz", "/internal/test/advance-clock"] {
            let response = unsub::router(state.clone())
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())?,
                )
                .await?;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "a release build must not serve {path}"
            );
        }
        Ok(())
    }

    /// E2E-INFRA AC3 (T-1101g): a release build has no test routes and no extra
    /// trust roots, even with `MT_E2E=1` and a complete e2e environment
    /// (including a certificate file): the `testkit`-gated e2e wiring is not
    /// compiled, so nothing reads the file and no reachable test route exists.
    #[test]
    fn e2e_infra_ac3_release_and_unflagged_testkit_have_no_test_routes_or_roots(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cert = super::scratch(&format!("unsub-release-ca-{}.pem", std::process::id()));
        std::fs::write(&cert, b"not a certificate")?;

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_unsub"));
        cmd.env_clear()
            .env("MT_E2E", "1")
            .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
            .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
            .env("UNSUB_TESTBED_URL", "https://127.0.0.1:8")
            .env("UNSUB_TESTBED_CA_FILE", &cert)
            .env("PORT", "0");
        let output = run_within_5s(&mut cmd)?;
        let text = combined(&output);
        assert!(!output.status.success(), "MT_E2E must be refused");
        assert!(
            text.contains("\"outcome\":\"failure\""),
            "the refusal is logged, so no test route was ever served"
        );
        assert!(
            !text.contains("\"outcome\":\"success\""),
            "a release build must never reach a running state"
        );
        let _ = std::fs::remove_file(&cert);
        Ok(())
    }
}

#[cfg(feature = "testkit")]
mod e2e {
    use std::io::{Read, Write};
    use std::net::{IpAddr, SocketAddr, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use obs::Sensitive;
    use ports::{
        CallerVerifier, EgressError, EgressRequest, HttpEgress, HttpMethod, OneClickOutcome,
    };
    use testkit::InMemoryServerStore;
    use tower::ServiceExt;
    use unsub::runner::UnsubState;
    use unsub::startup_e2e::{self, E2eConfig, E2eError, LoopbackEgress, StaticTokenVerifier};
    use unsub_testbed::{start, start_with_untrusted_cert};
    use url::Url;

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

    /// An ephemeral loopback port, released before the caller binds it.
    fn free_port() -> Result<u16, Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        Ok(listener.local_addr()?.port())
    }

    /// One `GET` egress request with no body.
    fn get(url: Url) -> EgressRequest {
        EgressRequest {
            method: HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(2),
        }
    }

    /// A loopback server answering `200` on every path; returns its address and
    /// the task handle so the caller can abort it.
    async fn spawn_ok_server(
    ) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok((addr, handle))
    }

    /// Send one HTTP/1.1 request to `addr`, closing after one response, and
    /// return the status code. `None` when the connect or read fails (the
    /// server may not be listening yet).
    fn http_status(addr: SocketAddr, request: &str) -> Option<u16> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok()?;
        stream.write_all(request.as_bytes()).ok()?;
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        text.lines()
            .next()?
            .split_whitespace()
            .nth(1)?
            .parse::<u16>()
            .ok()
    }

    /// Kills and reaps the child on drop, so a failing assertion never leaks a
    /// running server.
    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Wait for `child` to exit, killing it once `timeout` passes.
    fn wait_with_deadline(
        mut child: Child,
        timeout: Duration,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait()?.is_some() {
                return Ok(child.wait_with_output()?);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("process did not exit within the deadline".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Both streams, as lossy text.
    fn combined(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
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

    /// A non-loopback endpoint, or a hostname that is not a loopback IP
    /// literal, is refused at construction — before any connection exists.
    #[test]
    fn unsub_e2e_egress_rejects_non_loopback_endpoint() -> Result<(), Box<dyn std::error::Error>> {
        let empty: &[(&'static str, Url)] = &[];
        assert!(
            matches!(LoopbackEgress::new(empty, None), Err(E2eError::Invalid(_))),
            "an empty allow-list must be refused"
        );

        let public = Url::parse("http://93.184.216.34:80/")?;
        assert!(
            matches!(
                LoopbackEgress::new(&[("FAKE_GOOGLE_URL", public)], None),
                Err(E2eError::Invalid("FAKE_GOOGLE_URL"))
            ),
            "a public IP endpoint must be refused"
        );

        let hostname = Url::parse("http://example.com:80/")?;
        assert!(
            matches!(
                LoopbackEgress::new(&[("UNSUB_TESTBED_URL", hostname)], None),
                Err(E2eError::Invalid("UNSUB_TESTBED_URL"))
            ),
            "a hostname endpoint must be refused"
        );

        // The loopback literals, IPv4 and bracketed IPv6, are accepted.
        assert!(LoopbackEgress::new(
            &[("FAKE_GOOGLE_URL", Url::parse("http://127.0.0.1:9/")?)],
            None
        )
        .is_ok());
        assert!(LoopbackEgress::new(
            &[("UNSUB_TESTBED_URL", Url::parse("http://[::1]:9/")?)],
            None
        )
        .is_ok());
        Ok(())
    }

    /// Behaviour 3: the loopback allow-list opens only the configured socket.
    /// Any other host, port or hostname is refused before a connection.
    #[tokio::test]
    async fn unsub_e2e_egress_allows_only_configured_socket(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (addr, server) = spawn_ok_server().await?;
        let allowed = Url::parse(&format!("http://{addr}/"))?;
        let egress = LoopbackEgress::new(&[("FAKE_GOOGLE_URL", allowed.clone())], None)?;

        // The configured socket is reachable and answered.
        let response = egress.call(get(allowed)).await?;
        assert_eq!(response.status, 200, "the allowed socket must be reachable");

        // A different port on the same loopback address is not in the list.
        let other_port = if addr.port() == 2 { 3 } else { 2 };
        let other = Url::parse(&format!("http://127.0.0.1:{other_port}/"))?;
        assert!(
            matches!(
                egress.call(get(other)).await,
                Err(EgressError::HostNotAllowed)
            ),
            "a port that is not configured must be refused"
        );

        // The hostname form of the same address is never resolved against the
        // allow-list, so it is refused even though it names the same socket.
        let named = Url::parse(&format!("http://localhost:{}/", addr.port()))?;
        assert!(
            matches!(
                egress.call(get(named)).await,
                Err(EgressError::HostNotAllowed)
            ),
            "a hostname must not be resolved against the allow-list"
        );

        // A public IP is refused.
        let public = Url::parse("http://93.184.216.34/")?;
        assert!(
            matches!(
                egress.call(get(public)).await,
                Err(EgressError::HostNotAllowed)
            ),
            "a public IP must be refused"
        );

        server.abort();
        Ok(())
    }

    /// Behaviour 3: a redirect is returned as-is, never followed, so the
    /// allow-list cannot be escaped by a `Location` header.
    #[tokio::test]
    async fn unsub_e2e_egress_does_not_follow_redirects() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let app = axum::Router::new().route(
            "/redirect",
            axum::routing::get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "http://93.184.216.34/after")],
                )
            }),
        );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let base = Url::parse(&format!("http://{addr}/"))?;
        let egress = LoopbackEgress::new(&[("FAKE_GOOGLE_URL", base.clone())], None)?;
        let response = egress.call(get(base.join("redirect")?)).await?;
        assert_eq!(
            response.status, 302,
            "a redirect must be returned, not followed"
        );

        server.abort();
        Ok(())
    }

    /// F2: the real testkit binary, started with `MT_E2E=1`, boots, serves
    /// `/healthz` with 200 and answers an unauthenticated caller with 401.
    #[test]
    fn unsub_e2e_binary_serves_health_and_rejects_without_token(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let port = free_port()?;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let child = Command::new(env!("CARGO_BIN_EXE_unsub"))
            .env_clear()
            .env("MT_E2E", "1")
            .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
            .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
            .env("UNSUB_TESTBED_URL", "http://127.0.0.1:8")
            .env("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
            .env("PORT", port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut child = ChildGuard(child);

        let health_request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            startup_e2e::E2E_HEALTH_PATH
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut health = None;
        while Instant::now() < deadline {
            if child.0.try_wait()?.is_some() {
                break;
            }
            health = http_status(addr, &health_request);
            if health == Some(200) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let unauth_request = format!(
            "POST /internal/v1/unsubscribe-jobs/{}/run HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            uuid::Uuid::nil()
        );
        let unauth = http_status(addr, &unauth_request);

        assert_eq!(
            health,
            Some(200),
            "the e2e binary must serve /healthz with 200"
        );
        assert_eq!(unauth, Some(401), "an unauthenticated caller must get 401");
        Ok(())
    }

    /// Behaviour 3: a testkit build without `MT_E2E=1` takes the production
    /// wiring. The environment here is a *complete, valid e2e environment* but
    /// with an incomplete production config: if `main` wrongly took the e2e
    /// path, the server would boot and stay up (the deadline would trip), so a
    /// quick non-zero failure with no success log proves production was chosen.
    #[test]
    fn unsub_testkit_build_without_e2e_env_behaves_as_production(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let child = Command::new(env!("CARGO_BIN_EXE_unsub"))
            .env_clear()
            .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
            .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
            .env("UNSUB_TESTBED_URL", "http://127.0.0.1:8")
            .env("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
            .env("PORT", "0")
            // Present but not enough: production config also needs UNSUB_AUDIENCE.
            .env("GOOGLE_OAUTH_CLIENT_ID", "client-id")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let output = wait_with_deadline(child, Duration::from_secs(5))?;
        let text = combined(&output);

        assert!(
            !output.status.success(),
            "an incomplete production environment must fail to start"
        );
        assert!(
            text.contains("\"outcome\":\"failure\""),
            "start-up must log its failure through tracing"
        );
        assert!(
            !text.contains("\"outcome\":\"success\""),
            "MT_E2E unset must never reach a running state: {text}"
        );
        assert!(!text.contains("panicked"), "startup must not panic");
        Ok(())
    }

    /// E2E-INFRA AC2 (T-1101g): `unsub` completes a TLS one-click POST to the
    /// testbed trusting only the testbed's per-run CA.
    ///
    /// The certificate is read from `UNSUB_TESTBED_CA_FILE` exactly as the
    /// process reads it (`build_e2e_ports`), and reaches the client as one
    /// `add_root_certificate` root. Without that root the same POST fails the
    /// handshake, and a leaf the CA did not sign is refused even with the root
    /// present: the run's own certificate is what is trusted, not "any TLS".
    /// The production policy is untouched, so a real host stays refused.
    #[tokio::test]
    async fn e2e_infra_ac2_unsub_trusts_only_testbed_ca() -> Result<(), Box<dyn std::error::Error>>
    {
        let testbed = start().await?;
        // A run-scoped certificate file: the public part only, exactly what
        // scripts/e2e.sh points the process at.
        let ca_path = super::scratch(&format!(
            "unsub-e2e-ca-{}.pem",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&ca_path, testbed.test_ca_pem())?;

        let https_base = testbed.https_url("/");
        let ca = ca_path.to_string_lossy().into_owned();
        let store = Arc::new(InMemoryServerStore::new());
        let lookup = move |name: &str| -> Option<String> {
            match name {
                "UNSUB_E2E_CALLER_TOKEN" => Some(TOKEN.to_owned()),
                "FAKE_GOOGLE_URL" => Some("http://127.0.0.1:9".to_owned()),
                "UNSUB_TESTBED_URL" => Some(https_base.to_string()),
                "UNSUB_TESTBED_CA_FILE" => Some(ca.clone()),
                "PORT" => Some("0".to_owned()),
                _ => None,
            }
        };
        let (state, _config) = startup_e2e::build_e2e_ports(store, lookup)?;

        let target = testbed.https_url("/oneclick/200");
        let outcome = state.ports.egress.one_click_post(&target).await?;
        assert_eq!(
            outcome,
            OneClickOutcome::Accepted { status: 200 },
            "the CA-signed TLS listener must accept the one-click POST"
        );
        let records = testbed.requests_to("/oneclick/200");
        assert_eq!(records.len(), 1, "exactly one POST, over TLS");
        assert_eq!(records[0].body, b"List-Unsubscribe=One-Click");

        // Without the root the handshake fails: no other trust anchor covers a
        // loopback self-signed chain.
        let no_root = LoopbackEgress::new(&[("UNSUB_TESTBED_URL", testbed.https_url("/"))], None)?;
        assert!(
            no_root.one_click_post(&target).await.is_err(),
            "without the CA the TLS POST must fail"
        );

        // A leaf the CA did not sign is refused even with the root added.
        let untrusted = start_with_untrusted_cert().await?;
        let wrong = LoopbackEgress::new(
            &[("UNSUB_TESTBED_URL", untrusted.https_url("/"))],
            Some(testbed.test_ca_pem()),
        )?;
        assert!(
            wrong
                .one_click_post(&untrusted.https_url("/oneclick/200"))
                .await
                .is_err(),
            "an untrusted leaf must be refused"
        );

        // The production policy is untouched: a real host is still refused.
        let public = Url::parse("https://u.example.com/oneclick/200")?;
        assert!(
            matches!(
                state.ports.egress.one_click_post(&public).await,
                Err(EgressError::HostNotAllowed)
            ),
            "only the allowed sockets may be reached"
        );

        std::fs::remove_file(&ca_path)?;
        untrusted.shutdown().await;
        testbed.shutdown().await;
        Ok(())
    }

    /// E2E-INFRA AC3 (T-1101g), `testkit` build without `MT_E2E=1`: the process
    /// takes production wiring, so it serves no test route and reads no
    /// certificate file even when the whole e2e environment (including
    /// `UNSUB_TESTBED_CA_FILE`) is present.
    #[test]
    fn e2e_infra_ac3_release_and_unflagged_testkit_have_no_test_routes_or_roots(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cert = super::scratch(&format!(
            "unsub-unflagged-ca-{}.pem",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&cert, b"not a certificate")?;

        let child = Command::new(env!("CARGO_BIN_EXE_unsub"))
            .env_clear()
            .env("UNSUB_E2E_CALLER_TOKEN", "process-test-caller-token")
            .env("FAKE_GOOGLE_URL", "http://127.0.0.1:9")
            .env("UNSUB_TESTBED_URL", "https://127.0.0.1:8")
            .env("UNSUB_TESTBED_CA_FILE", &cert)
            .env("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
            .env("PORT", "0")
            // Present but not enough: production also needs UNSUB_AUDIENCE.
            .env("GOOGLE_OAUTH_CLIENT_ID", "client-id")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let output = wait_with_deadline(child, Duration::from_secs(5))?;
        let text = combined(&output);
        assert!(
            !output.status.success(),
            "without MT_E2E the e2e wiring must not run: {text}"
        );
        assert!(
            text.contains("\"outcome\":\"failure\""),
            "start-up must log its failure through tracing"
        );
        assert!(
            !text.contains("\"outcome\":\"success\""),
            "an unflagged testkit build must never reach a running state"
        );
        std::fs::remove_file(&cert)?;
        Ok(())
    }
}
