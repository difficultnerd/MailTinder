//! The `unsub` binary: production adapter wiring (T-701).
//!
//! Reads its configuration from the environment, wires the real GCP adapters
//! and serves the one internal route. Cloud Run sets `PORT`; the audience and
//! the Cloud Tasks caller are required, so a misconfigured service refuses
//! every call rather than trusting the wrong caller.
//!
//! The one-click sender (T-702) and the mailto sender (T-703) are both
//! registered below.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::process::ExitCode;
use std::sync::Arc;

use adapters_gcp::{
    production_clock, production_rng, CloudKms, CloudTasksScheduler, EnvelopeKeyService,
    FirestoreConfig, FirestoreStore, GcpHttp, GoogleCallerVerifier, KmsSystemKeyService,
    MetadataTokenSource, SecretManagerSecrets, SecretsConfig, SystemClock, TasksConfig,
    TokenSource,
};
use adapters_gmail::identity::{
    GoogleIdentity, GoogleIdentityConfig, GOOGLE_AUTH_ENDPOINT, GOOGLE_JWKS_URI,
    GOOGLE_REVOKE_ENDPOINT, GOOGLE_TOKEN_ENDPOINT,
};
use adapters_gmail::{DriveAppFolder, GmailHttp, GmailProvider};
use egress::{ProdEgress, Service, SystemResolver};
use obs::Sensitive;
use ports::{HttpEgress, InviteMailer, MailProvider, Ports, SecretName, Secrets};
use svc_common::internal_auth::InternalAuthConfig;
use unsub::config::UnsubConfig;
use unsub::mailto::MailtoSender;
use unsub::runner::UnsubState;
use unsub::sender::UnsubSender;
use unsub::{router, ROUTE_TEMPLATES};
use url::Url;

/// The fixed GCP region (T-1102a Terraform `var.region`).
const LOCATION: &str = "us-central1";
/// The fixed KMS key ring (T-1102a Terraform).
const KEY_RING: &str = "mailtinder";
/// The KMS key that wraps a user's `data_key` (T-302).
const USER_KEY_NAME: &str = "user-data";
/// The KMS key that seals pre-user data (S6 5).
const SYSTEM_KEY_NAME: &str = "system-fields";
/// The Gmail REST base, as `GmailHttp` expects it.
const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// The Google API root that the Drive app-folder store uses.
const GOOGLEAPIS_ROOT: &str = "https://www.googleapis.com";
/// The Cloud Tasks queue for unsubscribe jobs.
const QUEUE_ID: &str = "unsubscribe";

/// A start-up failure. Nothing is printed through the print macros; the
/// process logs one line through `tracing` and exits non-zero.
#[derive(Debug, thiserror::Error)]
enum SetupError {
    #[error("config")]
    Config(unsub::config::ConfigError),
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    #[error("e2e mode requires a testkit build")]
    #[allow(dead_code)]
    E2eNeedsTestkit,
    #[error("e2e start-up failed")]
    #[allow(dead_code)]
    E2e,
    #[error("adapter")]
    Adapter,
    #[error("obs")]
    Obs,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(event = "op", route = "unsub.startup", outcome = "failure", error = %e);
            ExitCode::FAILURE
        }
    }
}

/// True when the process was asked for e2e mode (`MT_E2E=1`). Read at run time
/// and independently of the build: a build without the `testkit` feature uses
/// it to refuse e2e outright instead of silently starting as production
/// (S10 3.3).
fn e2e_requested() -> bool {
    std::env::var("MT_E2E").is_ok_and(|v| v == "1")
}

/// Select the start-up wiring once: e2e only when `MT_E2E=1`, otherwise the
/// production wiring exactly as before. Observability is initialised first so
/// a refusal (or a failure) is always logged.
async fn run() -> Result<(), SetupError> {
    obs::init(
        "unsub",
        Arc::new(obs::StdoutSink) as Arc<dyn obs::LogSink>,
        obs::arc(SystemClock),
    )
    .map_err(|_| SetupError::Obs)?;
    obs::register_http_routes(ROUTE_TEMPLATES);

    if e2e_requested() {
        return e2e_startup().await;
    }
    production_run().await
}

/// e2e start-up (T-1112a). Reachable only when the crate is built with the
/// `testkit` feature.
#[cfg(feature = "testkit")]
async fn e2e_startup() -> Result<(), SetupError> {
    unsub::startup_e2e::run_e2e()
        .await
        .map_err(|_| SetupError::E2e)
}

/// e2e was requested but this build has no `testkit` feature: refuse to start
/// rather than run production wiring under an e2e environment (S10 3.3).
#[cfg(not(feature = "testkit"))]
#[allow(clippy::unused_async)]
async fn e2e_startup() -> Result<(), SetupError> {
    Err(SetupError::E2eNeedsTestkit)
}

#[allow(clippy::too_many_lines)]
async fn production_run() -> Result<(), SetupError> {
    let config = UnsubConfig::from_env().map_err(SetupError::Config)?;
    let clock = production_clock();
    let rng = production_rng();

    let project = env("GOOGLE_CLOUD_PROJECT")?;
    let unsub_base =
        Url::parse(&env("UNSUB_BASE_URL")?).map_err(|_| SetupError::Invalid("UNSUB_BASE_URL"))?;
    let client_id = env("GOOGLE_OAUTH_CLIENT_ID")?;

    let tokens: Arc<dyn TokenSource> = Arc::new(MetadataTokenSource::new(Arc::clone(&clock)));
    let http = Arc::new(GcpHttp::new(tokens).map_err(|_| SetupError::Adapter)?);
    let egress: Arc<dyn HttpEgress> = Arc::new(
        ProdEgress::new(Service::Unsub, Arc::new(SystemResolver))
            .map_err(|_| SetupError::Adapter)?,
    );

    let store: Arc<dyn ports::ServerStore> = Arc::new(FirestoreStore::new(
        Arc::clone(&http),
        FirestoreConfig::new(project.clone()),
    ));
    let keys: Arc<dyn ports::KeyService> = Arc::new(EnvelopeKeyService::new(
        Arc::new(CloudKms::new(
            Arc::clone(&http),
            kms_key(&project, USER_KEY_NAME),
        )),
        Arc::clone(&rng),
        Arc::clone(&clock),
    ));
    let system_keys: Arc<dyn ports::SystemKeyService> =
        Arc::new(KmsSystemKeyService::new(Arc::new(CloudKms::new(
            Arc::clone(&http),
            kms_key(&project, SYSTEM_KEY_NAME),
        ))));
    let secrets: Arc<dyn Secrets> = Arc::new(SecretManagerSecrets::new(
        Arc::clone(&http),
        SecretsConfig {
            project_id: project.clone(),
        },
    ));
    let scheduler: Arc<dyn ports::JobScheduler> = Arc::new(CloudTasksScheduler::new(
        Arc::clone(&http),
        TasksConfig {
            queue: format!("projects/{project}/locations/{LOCATION}/queues/{QUEUE_ID}"),
            unsub_base,
            invoker_sa_email: config.tasks_caller.clone(),
            audience: config.audience.clone(),
        },
    ));

    let caller: Arc<dyn ports::CallerVerifier> = Arc::new(GoogleCallerVerifier::new(
        Arc::clone(&egress),
        Arc::clone(&clock),
    ));

    let client_secret = secrets
        .get(SecretName::GoogleOAuthClientSecret)
        .await
        .map_err(|_| SetupError::Adapter)?;
    let client_secret = Sensitive::new(
        String::from_utf8(client_secret.expose().clone()).map_err(|_| SetupError::Adapter)?,
    );
    let identity: Arc<dyn ports::IdentityProvider> = Arc::new(GoogleIdentity::new(
        GoogleIdentityConfig {
            client_id,
            client_secret,
            auth_endpoint: endpoint(GOOGLE_AUTH_ENDPOINT)?,
            token_endpoint: endpoint(GOOGLE_TOKEN_ENDPOINT)?,
            revoke_endpoint: endpoint(GOOGLE_REVOKE_ENDPOINT)?,
            jwks_uri: endpoint(GOOGLE_JWKS_URI)?,
        },
        Arc::clone(&egress),
        Arc::clone(&clock),
    ));

    let gmail_client = GmailHttp::new(
        Arc::clone(&egress),
        endpoint(GMAIL_BASE)?,
        Arc::clone(&clock),
    );
    let drive_client = GmailHttp::new(
        Arc::clone(&egress),
        endpoint(GOOGLEAPIS_ROOT)?,
        Arc::clone(&clock),
    );
    let gmail = Arc::new(GmailProvider::new(gmail_client));
    let app_folder: Arc<dyn ports::AppFolderStore> = Arc::new(DriveAppFolder::new(drive_client));
    let invite_mailer: Arc<dyn InviteMailer> = Arc::clone(&gmail) as Arc<dyn InviteMailer>;
    let mail: Arc<dyn MailProvider> = Arc::clone(&gmail) as Arc<dyn MailProvider>;

    let ports = Ports {
        clock,
        rng,
        gmail: mail,
        app_folder,
        store,
        keys,
        system_keys,
        scheduler,
        egress: Arc::clone(&egress),
        identity,
        invite_mailer,
        models: Vec::new(),
        secrets,
        caller,
    };

    let state = UnsubState {
        ports,
        // T-702 (one-click) and T-703 (mailto) both register their sender here.
        senders: vec![
            Arc::new(unsub::one_click::OneClickSender) as Arc<dyn UnsubSender>,
            Arc::new(MailtoSender) as Arc<dyn UnsubSender>,
        ],
        auth: InternalAuthConfig {
            audience: config.audience,
            allowed_caller_email: config.tasks_caller,
        },
    };

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port))
        .await
        .map_err(|_| SetupError::Adapter)?;
    tracing::info!(event = "op", route = "unsub.startup", outcome = "success");
    axum::serve(listener, router(state))
        .await
        .map_err(|_| SetupError::Adapter)?;
    Ok(())
}

/// A required environment variable.
fn env(name: &'static str) -> Result<String, SetupError> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or(SetupError::Missing(name))
}

/// A pinned endpoint URL.
fn endpoint(raw: &str) -> Result<Url, SetupError> {
    Url::parse(raw).map_err(|_| SetupError::Invalid("endpoint"))
}

/// The full KMS key resource name for `key` in the project.
fn kms_key(project: &str, key: &str) -> String {
    format!("projects/{project}/locations/{LOCATION}/keyRings/{KEY_RING}/cryptoKeys/{key}")
}
