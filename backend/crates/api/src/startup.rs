//! The `api` binary's start-up wiring (T-500b).
//!
//! `main` is a thin shell: it calls [`run`] and maps the result to an
//! [`std::process::ExitCode`]. This module reads the environment, builds the
//! real ports and serves the router. A failure logs exactly one line and
//! decides the process exit; the mode (production or e2e) is chosen once at
//! start and never falls back.

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
use ports::{
    AppFolderStore, CallerVerifier, HttpEgress, IdentityProvider, InviteMailer, JobScheduler,
    KeyService, MailProvider, Ports, SecretName, Secrets, ServerStore, SystemKeyService,
};
use url::Url;

use crate::config::{ApiConfig, ConfigError};

// The GCP region, KMS key ring, key names, API roots and Cloud Tasks queue.
// Duplicated from `backend/crates/unsub/src/main.rs`, which owns the same
// production values; keep the two in step until they move to a shared place.
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
pub enum SetupError {
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    #[error("e2e mode requires a testkit build")]
    E2eNeedsTestkit,
    #[error("adapter construction failed")]
    Adapter,
    #[error("observability init failed")]
    Obs,
}

impl From<ConfigError> for SetupError {
    fn from(e: ConfigError) -> Self {
        match e {
            ConfigError::Missing(name) => Self::Missing(name),
            ConfigError::Invalid(name) => Self::Invalid(name),
        }
    }
}

impl SetupError {
    /// The variant name only. Never a message, never a value; safe to log.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Missing(_) => "Missing",
            Self::Invalid(_) => "Invalid",
            Self::E2eNeedsTestkit => "E2eNeedsTestkit",
            Self::Adapter => "Adapter",
            Self::Obs => "Obs",
        }
    }
}

/// True when the process should start in e2e mode. False unless the crate was
/// built with the `testkit` feature *and* `MT_E2E=1` is set (S10 3.3). T-1101a
/// mounts its test-only routes only when this is true.
#[cfg(feature = "testkit")]
#[must_use]
pub fn e2e_mode_enabled() -> bool {
    std::env::var("MT_E2E").is_ok_and(|v| v == "1")
}

/// False in any build without the `testkit` feature.
#[cfg(not(feature = "testkit"))]
#[must_use]
pub fn e2e_mode_enabled() -> bool {
    false
}

/// Start the service. Initialises observability, builds the ports for the
/// selected mode and serves until a shutdown signal.
///
/// # Errors
///
/// Returns [`SetupError`] when the environment is invalid, an adapter cannot
/// be built or the listener cannot be bound.
pub async fn run() -> Result<(), SetupError> {
    obs::init(
        "api",
        Arc::new(obs::StdoutSink) as Arc<dyn obs::LogSink>,
        obs::arc(SystemClock),
    )
    .map_err(|_| SetupError::Obs)?;
    obs::register_http_routes(crate::ROUTE_TEMPLATES);

    let (ports, config) = if e2e_mode_enabled() {
        e2e_startup()?
    } else {
        build_production_ports().await?
    };
    serve(ports, config).await
}

#[cfg(feature = "testkit")]
fn e2e_startup() -> Result<(Ports, ApiConfig), SetupError> {
    crate::startup_e2e::build_e2e_from_env()
}

/// A `testkit` build without `MT_E2E=1` never takes the e2e path; a build
/// without the feature refuses e2e outright (doubly guarded, S10 3.3).
#[cfg(not(feature = "testkit"))]
fn e2e_startup() -> Result<(Ports, ApiConfig), SetupError> {
    Err(SetupError::E2eNeedsTestkit)
}

/// Build the production `Ports` and `ApiConfig` from the environment.
///
/// # Errors
///
/// Returns [`SetupError::Missing`]/[`SetupError::Invalid`] for a bad
/// environment and [`SetupError::Adapter`] when an adapter cannot be built.
#[allow(clippy::too_many_lines)]
pub async fn build_production_ports() -> Result<(Ports, ApiConfig), SetupError> {
    let base = ApiConfig::from_env()?;
    let project = env("GOOGLE_CLOUD_PROJECT")?;
    let unsub_base =
        Url::parse(&env("UNSUB_BASE_URL")?).map_err(|_| SetupError::Invalid("UNSUB_BASE_URL"))?;
    let audience = env("UNSUB_AUDIENCE")?;
    let tasks_caller = env("UNSUB_TASKS_CALLER")?;

    let clock = production_clock();
    let rng = production_rng();

    let tokens: Arc<dyn TokenSource> = Arc::new(MetadataTokenSource::new(Arc::clone(&clock)));
    let http = Arc::new(GcpHttp::new(tokens).map_err(|_| SetupError::Adapter)?);
    let egress: Arc<dyn HttpEgress> = Arc::new(
        ProdEgress::new(Service::Api, Arc::new(SystemResolver)).map_err(|_| SetupError::Adapter)?,
    );

    let store: Arc<dyn ServerStore> = Arc::new(FirestoreStore::new(
        Arc::clone(&http),
        FirestoreConfig::new(project.clone()),
    ));
    let keys: Arc<dyn KeyService> = Arc::new(EnvelopeKeyService::new(
        Arc::new(CloudKms::new(
            Arc::clone(&http),
            kms_key(&project, USER_KEY_NAME),
        )),
        Arc::clone(&rng),
        Arc::clone(&clock),
    ));
    let system_keys: Arc<dyn SystemKeyService> = Arc::new(KmsSystemKeyService::new(Arc::new(
        CloudKms::new(Arc::clone(&http), kms_key(&project, SYSTEM_KEY_NAME)),
    )));
    let secrets: Arc<dyn Secrets> = Arc::new(SecretManagerSecrets::new(
        Arc::clone(&http),
        SecretsConfig {
            project_id: project.clone(),
        },
    ));
    let scheduler: Arc<dyn JobScheduler> = Arc::new(CloudTasksScheduler::new(
        Arc::clone(&http),
        TasksConfig {
            queue: format!("projects/{project}/locations/{LOCATION}/queues/{QUEUE_ID}"),
            unsub_base,
            invoker_sa_email: tasks_caller,
            audience,
        },
    ));
    let caller: Arc<dyn CallerVerifier> = Arc::new(GoogleCallerVerifier::new(
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
    let identity: Arc<dyn IdentityProvider> = Arc::new(GoogleIdentity::new(
        GoogleIdentityConfig {
            client_id: base.google_client_id.clone(),
            client_secret,
            auth_endpoint: endpoint("GOOGLE_AUTH_ENDPOINT", GOOGLE_AUTH_ENDPOINT)?,
            token_endpoint: endpoint("GOOGLE_TOKEN_ENDPOINT", GOOGLE_TOKEN_ENDPOINT)?,
            revoke_endpoint: endpoint("GOOGLE_REVOKE_ENDPOINT", GOOGLE_REVOKE_ENDPOINT)?,
            jwks_uri: endpoint("GOOGLE_JWKS_URI", GOOGLE_JWKS_URI)?,
        },
        Arc::clone(&egress),
        Arc::clone(&clock),
    ));

    let gmail_client = GmailHttp::new(
        Arc::clone(&egress),
        endpoint("GMAIL_BASE", GMAIL_BASE)?,
        Arc::clone(&clock),
    );
    let drive_client = GmailHttp::new(
        Arc::clone(&egress),
        endpoint("GOOGLEAPIS_ROOT", GOOGLEAPIS_ROOT)?,
        Arc::clone(&clock),
    );
    let gmail = Arc::new(GmailProvider::new(gmail_client));
    let app_folder: Arc<dyn AppFolderStore> = Arc::new(DriveAppFolder::new(drive_client));
    let invite_mailer: Arc<dyn InviteMailer> = Arc::clone(&gmail) as Arc<dyn InviteMailer>;
    let mail: Arc<dyn MailProvider> = Arc::clone(&gmail) as Arc<dyn MailProvider>;

    let rate_key = secrets
        .get(SecretName::LogPseudonymHmacKey)
        .await
        .map_err(|_| SetupError::Adapter)?;
    let email_lookup_key = secrets
        .get(SecretName::EmailLookupHmacKey)
        .await
        .map_err(|_| SetupError::Adapter)?;

    let ports = Ports {
        clock,
        rng,
        gmail: mail,
        app_folder,
        store,
        keys,
        system_keys,
        scheduler,
        egress,
        identity,
        invite_mailer,
        models: Vec::new(),
        secrets,
        caller,
    };

    Ok((ports, base.with_keys(rate_key, email_lookup_key)))
}

/// Bind and serve the router with graceful shutdown on `SIGTERM` and `SIGINT`.
async fn serve(ports: Ports, config: ApiConfig) -> Result<(), SetupError> {
    let port = config.port;
    let state = crate::app_state(Arc::new(ports), Arc::new(config));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|_| SetupError::Adapter)?;
    tracing::info!(event = "op", route = "api.startup", outcome = "success");
    axum::serve(listener, crate::build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| SetupError::Adapter)?;
    Ok(())
}

/// Resolve when Cloud Run asks the process to stop (`SIGTERM`) or the operator
/// presses Ctrl-C (`SIGINT`).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// A required, non-empty environment variable. Only the name is ever surfaced.
fn env(name: &'static str) -> Result<String, SetupError> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or(SetupError::Missing(name))
}

/// A pinned endpoint URL. `name` is what a failure reports.
fn endpoint(name: &'static str, raw: &str) -> Result<Url, SetupError> {
    Url::parse(raw).map_err(|_| SetupError::Invalid(name))
}

/// The full KMS key resource name for `key` in the project.
fn kms_key(project: &str, key: &str) -> String {
    format!("projects/{project}/locations/{LOCATION}/keyRings/{KEY_RING}/cryptoKeys/{key}")
}
