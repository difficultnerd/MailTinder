//! T-304: the `JobScheduler` contract suite and request-shape tests against a
//! local `axum` stub of the Cloud Tasks REST API, wired through
//! `GcpHttp::with_emulator`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use adapters_gcp::{CloudTasksScheduler, GcpHttp, StaticTokenSource, TasksConfig, TokenSource};
use async_trait::async_trait;
use base64::Engine;
use obs::Sensitive;
use ports::{JobScheduler, TaskName};
use testkit::contract::job_scheduler::{job_scheduler, SchedulerControl, SchedulerTarget};
use time::OffsetDateTime;
use url::Url;

const QUEUE: &str = "projects/p/locations/us-central1/queues/unsubscribe";
const UNSUB_BASE: &str = "https://unsub-abc123-uc.a.run.app";
const SA_EMAIL: &str = "tasks@unsub.iam.gserviceaccount.com";
const AUDIENCE: &str = "https://unsub-abc123-uc.a.run.app";

/// The state of a task in the stub.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StubState {
    Pending,
    Running,
    Done,
    Deleted,
}

/// A stub Cloud Tasks API: keeps a map of tasks keyed by full name, answers
/// `POST` (409 on a duplicate name), `DELETE` (404 when missing, 400
/// `FAILED_PRECONDITION` when marked running), and records the last request
/// body for the shape test.
struct Stub {
    tasks: Mutex<HashMap<String, StubState>>,
    last_body: Mutex<Option<serde_json::Value>>,
}

impl Stub {
    fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            last_body: Mutex::new(None),
        }
    }

    fn last_body(&self) -> serde_json::Value {
        self.last_body
            .lock()
            .unwrap()
            .clone()
            .expect("a request was recorded")
    }
}

async fn stub_server() -> (SocketAddr, Arc<Stub>) {
    let stub = Arc::new(Stub::new());
    let stub_route = Arc::clone(&stub);

    let app = axum::Router::new().route(
        "/v2/{*rest}",
        axum::routing::any(move |method: axum::http::Method, uri: axum::http::Uri, body: axum::body::Bytes| {
            let stub = Arc::clone(&stub_route);
            async move {
                let path = uri.path();
                let full_name = path.trim_start_matches("/v2/");
                match method {
                    axum::http::Method::POST => {
                        let json: serde_json::Value =
                            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                        *stub.last_body.lock().unwrap() = Some(json.clone());
                        let name = json
                            .pointer("/task/name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_owned();
                        let mut tasks = stub.tasks.lock().unwrap();
                        if tasks.contains_key(&name) {
                            return (
                                axum::http::StatusCode::CONFLICT,
                                axum::Json(serde_json::json!({
                                    "error": { "code": 409, "status": "ALREADY_EXISTS", "message": "dup" }
                                })),
                            );
                        }
                        tasks.insert(name.clone(), StubState::Pending);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({ "name": name })),
                        )
                    }
                    axum::http::Method::DELETE => {
                                            let mut tasks = stub.tasks.lock().unwrap();
                                            match tasks.get(&full_name.to_owned()) {
                                                None => (
                                                    axum::http::StatusCode::NOT_FOUND,
                                                    axum::Json(serde_json::json!({
                                                        "error": { "code": 404, "status": "NOT_FOUND", "message": "missing" }
                                                    })),
                                                ),
                                                Some(StubState::Running) => (
                                                    axum::http::StatusCode::BAD_REQUEST,
                                                    axum::Json(serde_json::json!({
                                                        "error": { "code": 400, "status": "FAILED_PRECONDITION", "message": "running" }
                                                    })),
                                                ),
                                                Some(StubState::Deleted | StubState::Done) => (
                                                    axum::http::StatusCode::NOT_FOUND,
                                                    axum::Json(serde_json::json!({
                                                        "error": { "code": 404, "status": "NOT_FOUND", "message": "gone" }
                                                    })),
                                                ),
                                                Some(_) => {
                                                    tasks.insert(full_name.to_owned(), StubState::Deleted);
                                                    (
                                                        axum::http::StatusCode::OK,
                                                        axum::Json(serde_json::json!({})),
                                                    )
                                                }
                                            }
                                        }
                    _ => (
                        axum::http::StatusCode::METHOD_NOT_ALLOWED,
                        axum::Json(serde_json::json!({})),
                    ),
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, stub)
}

fn cfg() -> TasksConfig {
    TasksConfig {
        queue: QUEUE.to_owned(),
        unsub_base: Url::parse(UNSUB_BASE).expect("url"),
        invoker_sa_email: SA_EMAIL.to_owned(),
        audience: AUDIENCE.to_owned(),
    }
}

fn client(addr: SocketAddr) -> CloudTasksScheduler {
    let tokens: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(Sensitive::new("t".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).expect("emulator client"));
    let base = Url::parse(&format!("http://{addr}/v2/")).expect("url");
    CloudTasksScheduler::with_base(http, cfg(), base)
}

/// The stub implements `SchedulerControl` by flipping its internal state.
struct StubControl {
    stub: Arc<Stub>,
}

#[async_trait]
impl SchedulerControl for StubControl {
    async fn mark_running(&self, task: &TaskName) -> Result<(), String> {
        let full = format!("{QUEUE}/tasks/{}", task.0);
        let mut tasks = self.stub.tasks.lock().unwrap();
        if let Some(state) = tasks.get_mut(&full) {
            if *state == StubState::Pending {
                *state = StubState::Running;
            }
        }
        Ok(())
    }

    async fn mark_done(&self, task: &TaskName) -> Result<(), String> {
        let full = format!("{QUEUE}/tasks/{}", task.0);
        let mut tasks = self.stub.tasks.lock().unwrap();
        if let Some(state) = tasks.get_mut(&full) {
            if *state == StubState::Running {
                *state = StubState::Done;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn job_scheduler_contract_cloud_tasks_stub() -> Result<(), String> {
    let (addr, stub) = stub_server().await;
    job_scheduler(|| async {
        let sched = client(addr);
        SchedulerTarget {
            scheduler: Arc::new(sched),
            control: Arc::new(StubControl {
                stub: Arc::clone(&stub),
            }),
        }
    })
    .await
}

/// The create request body has the exact shape Cloud Tasks expects: task name,
/// `scheduleTime` in UTC, URL path with the job ID, empty JSON body, `oidcToken`
/// with the configured email and audience, `dispatchDeadline` 60 s, and no
/// other headers.
#[tokio::test]
async fn cloud_tasks_create_body_shape() -> Result<(), String> {
    let (addr, stub) = stub_server().await;
    let sched = client(addr);
    let job = domain::JobId(uuid::Uuid::from_u128(0x9001));
    let due = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid");
    sched
        .schedule(&job, due)
        .await
        .map_err(|e| format!("{e:?}"))?;

    let body = stub.last_body();
    let task = body.get("task").ok_or("no task")?;
    assert_eq!(
        task["name"],
        serde_json::Value::String(format!("{QUEUE}/tasks/job-{}", job.0.simple()))
    );
    assert_eq!(
        task["scheduleTime"],
        serde_json::Value::String("2023-11-14T22:13:20Z".into())
    );
    assert_eq!(
        task["dispatchDeadline"],
        serde_json::Value::String("60s".into())
    );
    let http = task.get("httpRequest").ok_or("no httpRequest")?;
    assert_eq!(
        http["url"],
        serde_json::Value::String(format!(
            "{UNSUB_BASE}/internal/v1/unsubscribe-jobs/{}/run",
            job.0.simple()
        ))
    );
    assert_eq!(http["httpMethod"], serde_json::Value::String("POST".into()));
    assert_eq!(
        http["body"],
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(b"{}"))
    );
    let headers = http.get("headers").ok_or("no headers")?;
    assert_eq!(
        headers["Content-Type"],
        serde_json::Value::String("application/json".into())
    );
    assert_eq!(headers.as_object().map(serde_json::Map::len), Some(1));
    let oidc = http.get("oidcToken").ok_or("no oidcToken")?;
    assert_eq!(
        oidc["serviceAccountEmail"],
        serde_json::Value::String(SA_EMAIL.into())
    );
    assert_eq!(oidc["audience"], serde_json::Value::String(AUDIENCE.into()));
    Ok(())
}

/// A 409 `ALREADY_EXISTS` on create is idempotent: the same task name returns.
#[tokio::test]
async fn cloud_tasks_create_conflict_is_idempotent() -> Result<(), String> {
    let (addr, _stub) = stub_server().await;
    let sched = client(addr);
    let job = domain::JobId(uuid::Uuid::from_u128(0x9002));
    let due = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid");
    let n1 = sched
        .schedule(&job, due)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let n2 = sched
        .schedule(&job, due)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if n1 != n2 {
        return Err("conflict should return the same name".into());
    }
    Ok(())
}

/// 429 and 503 map to `Unavailable`; 403 maps to `Rejected`.
#[tokio::test]
async fn cloud_tasks_errors_map_to_sched_error() -> Result<(), String> {
    // 429 -> Unavailable
    {
        let app = axum::Router::new().route(
            "/v2/{*rest}",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "error": { "code": 429, "status": "UNAVAILABLE", "message": "busy" }
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let sched = client(addr);
        let job = domain::JobId(uuid::Uuid::from_u128(0x9003));
        let err = sched
            .schedule(
                &job,
                OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid"),
            )
            .await
            .expect_err("429 must fail");
        if !matches!(err, ports::SchedError::Unavailable) {
            return Err(format!("429 should be Unavailable, got {err:?}"));
        }
    }
    // 503 -> Unavailable
    {
        let app = axum::Router::new().route(
            "/v2/{*rest}",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(serde_json::json!({
                        "error": { "code": 503, "status": "UNAVAILABLE", "message": "down" }
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let sched = client(addr);
        let job = domain::JobId(uuid::Uuid::from_u128(0x9004));
        let err = sched
            .schedule(
                &job,
                OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid"),
            )
            .await
            .expect_err("503 must fail");
        if !matches!(err, ports::SchedError::Unavailable) {
            return Err(format!("503 should be Unavailable, got {err:?}"));
        }
    }
    // 403 -> Rejected
    {
        let app = axum::Router::new().route(
            "/v2/{*rest}",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "error": { "code": 403, "status": "PERMISSION_DENIED", "message": "no" }
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let sched = client(addr);
        let job = domain::JobId(uuid::Uuid::from_u128(0x9005));
        let err = sched
            .schedule(
                &job,
                OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid"),
            )
            .await
            .expect_err("403 must fail");
        if !matches!(err, ports::SchedError::Rejected(_)) {
            return Err(format!("403 should be Rejected, got {err:?}"));
        }
    }
    Ok(())
}
