//! The production `JobScheduler`: one Cloud Task per unsubscribe job.
//!
//! A task is named after the job ID, scheduled for the due time, and calls
//! `unsub`'s API-INT-1 with a Google-signed OIDC token. Cancelling deletes the
//! task. The job ID is in the URL path and the body is `{}`, so no target,
//! address or token travels in the task (S7 5.12 loads the job by ID).

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use domain::JobId;
use ports::{CancelOutcome, JobScheduler, SchedError, TaskName};
use reqwest::Method;
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

use crate::gcp_http::{GcpError, GcpHttp};

/// The one-click timeout is 10 s; the dispatch deadline leaves room for token
/// minting.
pub const DISPATCH_DEADLINE_S: u64 = 60;

/// Configuration for the Cloud Tasks scheduler.
#[derive(Clone, Debug)]
pub struct TasksConfig {
    /// `projects/{p}/locations/us-central1/queues/unsubscribe`.
    pub queue: String,
    /// `https://unsub-<hash>-uc.a.run.app`.
    pub unsub_base: Url,
    /// The service account Cloud Tasks signs the OIDC token as.
    pub invoker_sa_email: String,
    /// The audience `unsub` checks (S7 5.12).
    pub audience: String,
}

/// The Cloud Tasks REST client, bound to one queue.
pub struct CloudTasksScheduler {
    http: Arc<GcpHttp>,
    cfg: TasksConfig,
    /// `https://cloudtasks.googleapis.com/v2/`.
    base: Url,
}

impl CloudTasksScheduler {
    /// Production client against the real Cloud Tasks API.
    pub fn new(http: Arc<GcpHttp>, cfg: TasksConfig) -> Self {
        let base = Url::parse("https://cloudtasks.googleapis.com/v2/")
            .unwrap_or_else(|_| panic!("valid base url"));
        Self { http, cfg, base }
    }

    /// Test-only constructor pointing at a local stub.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base(http: Arc<GcpHttp>, cfg: TasksConfig, base: Url) -> Self {
        Self { http, cfg, base }
    }

    /// The full task name `{queue}/tasks/{name}`.
    fn full_name(&self, name: &TaskName) -> String {
        format!("{}/tasks/{}", self.cfg.queue, name.0)
    }

    /// The `unsub` run URL for a job: `{unsub_base}/internal/v1/unsubscribe-jobs/{job}/run`.
    fn run_url(&self, job: &JobId) -> Url {
        let mut url = self.cfg.unsub_base.clone();
        url.set_path(&format!(
            "/internal/v1/unsubscribe-jobs/{}/run",
            job.0.simple()
        ));
        url
    }
}

/// The Cloud Tasks `Task` create request body.
#[derive(Serialize)]
struct CreateTaskRequest<'a> {
    task: TaskBody<'a>,
}

#[derive(Serialize)]
struct TaskBody<'a> {
    name: &'a str,
    #[serde(rename = "scheduleTime")]
    schedule_time: String,
    #[serde(rename = "dispatchDeadline")]
    dispatch_deadline: &'a str,
    #[serde(rename = "httpRequest")]
    http_request: HttpRequest<'a>,
}

#[derive(Serialize)]
struct HttpRequest<'a> {
    url: String,
    #[serde(rename = "httpMethod")]
    http_method: &'a str,
    headers: serde_json::Map<String, serde_json::Value>,
    body: String,
    #[serde(rename = "oidcToken")]
    oidc_token: OidcToken<'a>,
}

#[derive(Serialize)]
struct OidcToken<'a> {
    #[serde(rename = "serviceAccountEmail")]
    service_account_email: &'a str,
    audience: &'a str,
}

/// The Cloud Tasks `Task` create response. The body is not read; a 2xx status
/// is enough.
#[derive(serde::Deserialize)]
struct TaskResponse {}

#[async_trait]
impl JobScheduler for CloudTasksScheduler {
    async fn schedule(&self, job: &JobId, due_at: OffsetDateTime) -> Result<TaskName, SchedError> {
        let name = TaskName::for_job(job);
        let full = self.full_name(&name);
        let mut url = self.base.clone();
        url.set_path(&format!("/v2/{}/tasks", self.cfg.queue));

        let body = CreateTaskRequest {
            task: TaskBody {
                name: &full,
                schedule_time: due_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|_| SchedError::Rejected("tasks_create"))?,
                dispatch_deadline: &format!("{DISPATCH_DEADLINE_S}s"),
                http_request: HttpRequest {
                    url: self.run_url(job).to_string(),
                    http_method: "POST",
                    headers: serde_json::Map::from_iter([(
                        "Content-Type".to_owned(),
                        serde_json::Value::String("application/json".to_owned()),
                    )]),
                    body: base64::engine::general_purpose::STANDARD.encode(b"{}"),
                    oidc_token: OidcToken {
                        service_account_email: &self.cfg.invoker_sa_email,
                        audience: &self.cfg.audience,
                    },
                },
            },
        };

        match self
            .http
            .json::<CreateTaskRequest<'_>, TaskResponse>(Method::POST, &url, Some(&body))
            .await
        {
            Ok(_) => Ok(name),
            Err(GcpError::AlreadyExists) => Ok(name),
            Err(GcpError::Unavailable) => Err(SchedError::Unavailable),
            Err(GcpError::PermissionDenied)
            | Err(GcpError::Unauthenticated)
            | Err(GcpError::NotFound)
            | Err(GcpError::BadResponse) => Err(SchedError::Rejected("tasks_create")),
            Err(_) => Err(SchedError::Unavailable),
        }
    }

    async fn cancel(&self, task: &TaskName) -> Result<CancelOutcome, SchedError> {
        let mut url = self.base.clone();
        url.set_path(&format!("/v2/{}", self.full_name(task)));
        match self.http.send(Method::DELETE, &url).await {
            Ok(()) => Ok(CancelOutcome::Cancelled),
            Err(GcpError::NotFound) => Ok(CancelOutcome::NotFound),
            Err(GcpError::FailedPrecondition) => Ok(CancelOutcome::AlreadyRunning),
            Err(_) => Err(SchedError::Unavailable),
        }
    }
}
