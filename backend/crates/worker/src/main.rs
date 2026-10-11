//! The `worker` binary: production adapter wiring (T-706).
//!
//! Reads its configuration from the environment, wires the real GCP adapters
//! and serves the one internal sweep route. Cloud Run sets `PORT`; the audience
//! and the Cloud Scheduler caller are required, so a misconfigured service
//! refuses every call rather than trusting the wrong caller.
//!
//! `worker` holds KMS rights (S4 2) because it decrypts a job's target and
//! sender display and seals Needs Attention fields. It has no Gmail and no
//! Drive access, so the unused `Ports` fields are wired to refusing stubs; a
//! call to one is a bug, not a feature.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::process::ExitCode;
use std::sync::Arc;

use adapters_gcp::{
    production_clock, production_rng, CloudKms, EnvelopeKeyService, FirestoreConfig,
    FirestoreStore, GcpHttp, GoogleCallerVerifier, KmsSystemKeyService, MetadataTokenSource,
    SecretManagerSecrets, SecretsConfig, SystemClock, TokenSource,
};
use egress::{ProdEgress, Service, SystemResolver};
use ports::{HttpEgress, Ports, Secrets};
use svc_common::internal_auth::InternalAuthConfig;
use worker::sweep::WorkerState;
use worker::{router, ROUTE_TEMPLATES};

/// The fixed GCP region (T-1102a Terraform `var.region`).
const LOCATION: &str = "us-central1";
/// The fixed KMS key ring (T-1102a Terraform).
const KEY_RING: &str = "mailtinder";
/// The KMS key that wraps a user's `data_key` (T-302). It is named
/// `data-key-kek` in Terraform (`modules/foundation/kms.tf`).
const USER_KEY_NAME: &str = "data-key-kek";
/// The KMS key that seals pre-user data (S6 5).
const SYSTEM_KEY_NAME: &str = "system-fields";
/// The default listen port when `PORT` is unset (Cloud Run sets it).
const DEFAULT_PORT: u16 = 8080;

/// A start-up failure. Nothing is printed through the print macros; the process
/// logs one line through `tracing` and exits non-zero.
#[derive(Debug, thiserror::Error)]
enum SetupError {
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    #[error("adapter")]
    Adapter,
    #[error("obs")]
    Obs,
}

/// Everything `worker` needs to start.
struct Config {
    /// The `aud` every internal ID token must carry.
    audience: String,
    /// The only service account email allowed to call the internal route.
    scheduler_caller: String,
    /// The listen port.
    port: u16,
}

impl Config {
    /// Load the configuration from the process environment.
    fn from_env() -> Result<Self, SetupError> {
        let audience = env("WORKER_AUDIENCE")?;
        let scheduler_caller = env("WORKER_SCHEDULER_CALLER")?;
        let port = match non_empty(std::env::var("PORT").ok()) {
            None => DEFAULT_PORT,
            Some(p) => p.parse::<u16>().map_err(|_| SetupError::Invalid("PORT"))?,
        };
        Ok(Self {
            audience,
            scheduler_caller,
            port,
        })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(event = "op", route = "worker.startup", outcome = "failure", error = %e);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), SetupError> {
    let config = Config::from_env()?;
    let clock = production_clock();
    let rng = production_rng();
    obs::init(
        "worker",
        Arc::new(obs::StdoutSink) as Arc<dyn obs::LogSink>,
        obs::arc(SystemClock),
    )
    .map_err(|_| SetupError::Obs)?;
    obs::register_http_routes(ROUTE_TEMPLATES);

    let project = env("GOOGLE_CLOUD_PROJECT")?;
    let tokens: Arc<dyn TokenSource> = Arc::new(MetadataTokenSource::new(Arc::clone(&clock)));
    let http = Arc::new(GcpHttp::new(tokens).map_err(|_| SetupError::Adapter)?);
    let egress: Arc<dyn HttpEgress> = Arc::new(
        ProdEgress::new(Service::Worker, Arc::new(SystemResolver))
            .map_err(|_| SetupError::Adapter)?,
    );

    let store: Arc<dyn ports::ServerStore> = Arc::new(FirestoreStore::new(
        Arc::clone(&http),
        FirestoreConfig::new(project.clone()),
    ));
    let keys: Arc<dyn ports::KeyService> = Arc::new(EnvelopeKeyService::new(
        Arc::new(CloudKms::new(
            Arc::clone(&http),
            user_key_resource(&project, std::env::var("MT_KMS_KEY").ok()),
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
    let caller: Arc<dyn ports::CallerVerifier> = Arc::new(GoogleCallerVerifier::new(
        Arc::clone(&egress),
        Arc::clone(&clock),
    ));

    let ports = Ports {
        clock,
        rng,
        // The worker has no Gmail and no Drive access (S4 2).
        gmail: Arc::new(unused::NoMail) as Arc<dyn ports::MailProvider>,
        app_folder: Arc::new(unused::NoAppFolder) as Arc<dyn ports::AppFolderStore>,
        store,
        keys,
        system_keys,
        scheduler: Arc::new(unused::NoScheduler) as Arc<dyn ports::JobScheduler>,
        egress,
        identity: Arc::new(unused::NoIdentity) as Arc<dyn ports::IdentityProvider>,
        invite_mailer: Arc::new(unused::NoInviteMailer) as Arc<dyn ports::InviteMailer>,
        models: Vec::new(),
        secrets,
        caller,
    };

    let state = WorkerState {
        ports,
        auth: InternalAuthConfig {
            audience: config.audience,
            allowed_caller_email: config.scheduler_caller,
        },
    };

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port))
        .await
        .map_err(|_| SetupError::Adapter)?;
    tracing::info!(event = "op", route = "worker.startup", outcome = "success");
    axum::serve(listener, router(state))
        .await
        .map_err(|_| SetupError::Adapter)?;
    Ok(())
}

/// A required environment variable.
fn env(name: &'static str) -> Result<String, SetupError> {
    non_empty(std::env::var(name).ok()).ok_or(SetupError::Missing(name))
}

/// A value that is present and not blank.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// The full KMS key resource name for `key` in the project.
fn kms_key(project: &str, key: &str) -> String {
    format!("projects/{project}/locations/{LOCATION}/keyRings/{KEY_RING}/cryptoKeys/{key}")
}

/// The user-data KMS key resource name. Cloud Run injects the deployed key's
/// full resource name as `MT_KMS_KEY` (T-1102b Terraform); it wins when set so
/// a rename or a region change cannot leave the service unwrapping against a
/// stale, hard-coded path. Without it the path is rebuilt from the constants,
/// so a local run still gets one (review F4).
fn user_key_resource(project: &str, injected: Option<String>) -> String {
    injected
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| kms_key(project, USER_KEY_NAME))
}

#[cfg(test)]
mod tests {
    //! The key-name wiring the production start-up depends on (review F4).
    #![allow(clippy::pedantic)]

    use super::*;

    #[test]
    fn user_key_resource_prefers_the_injected_key() {
        let injected = "projects/mailtinder/locations/europe-west1/keyRings/renamed/cryptoKeys/kek";
        assert_eq!(
            user_key_resource("mailtinder", Some(injected.to_owned())),
            injected
        );
    }

    #[test]
    fn user_key_resource_falls_back_to_the_constants() {
        assert_eq!(
            user_key_resource("mailtinder", None),
            kms_key("mailtinder", USER_KEY_NAME)
        );
        assert_eq!(
            user_key_resource("mailtinder", Some("   ".to_owned())),
            kms_key("mailtinder", USER_KEY_NAME)
        );
    }
}

/// Refusing stubs for the `Ports` the worker must not use. A call is a bug.
#[allow(clippy::unused_async, clippy::expect_used)]
mod unused {
    use async_trait::async_trait;
    use domain::{EmailAddress, JobId, LabelSet, MailtoTarget, MessageId, MessageMeta, Provider};
    use obs::Sensitive;
    use ports::{
        AppFolderError, AppFolderStore, AuthRequest, CancelOutcome, ETag, IdClaims, IdError,
        IdentityProvider, InviteLink, InviteMailer, JobScheduler, ListOrder, MailError,
        MailProvider, MailboxCtx, MessagePage, MessageQuery, PageToken, ProviderCapabilities,
        SchedError, TaskName, TokenSet,
    };
    use time::OffsetDateTime;
    use url::Url;

    /// The one error every stub returns.
    fn refused() -> MailError {
        MailError::Invalid("worker has no mail access".to_owned())
    }

    /// A URL constant that cannot fail to parse.
    fn stub_url() -> Url {
        Url::parse("https://worker.invalid/").unwrap_or_else(|_| unreachable!())
    }

    /// The worker has no Gmail access (S4 2).
    pub struct NoMail;

    #[async_trait]
    impl MailProvider for NoMail {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                labels_are_sets: true,
                spam_is_label: true,
            }
        }
        async fn list_inbox(
            &self,
            _mb: &MailboxCtx,
            _page: Option<PageToken>,
            _order: ListOrder,
        ) -> Result<MessagePage, MailError> {
            Err(refused())
        }
        async fn get_meta(
            &self,
            _mb: &MailboxCtx,
            _id: &MessageId,
        ) -> Result<MessageMeta, MailError> {
            Err(refused())
        }
        async fn get_preview(
            &self,
            _mb: &MailboxCtx,
            _id: &MessageId,
        ) -> Result<String, MailError> {
            Err(refused())
        }
        async fn get_text(
            &self,
            mb: &MailboxCtx,
            id: &MessageId,
            _max_chars: usize,
        ) -> Result<String, MailError> {
            self.get_preview(mb, id).await
        }
        async fn set_labels(
            &self,
            _mb: &MailboxCtx,
            _id: &MessageId,
            _add: &LabelSet,
            _remove: &LabelSet,
        ) -> Result<LabelSet, MailError> {
            Err(refused())
        }
        async fn trash(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
            Err(refused())
        }
        async fn report_spam(
            &self,
            _mb: &MailboxCtx,
            _id: &MessageId,
        ) -> Result<LabelSet, MailError> {
            Err(refused())
        }
        async fn restore_labels(
            &self,
            _mb: &MailboxCtx,
            _id: &MessageId,
            _exact: &LabelSet,
        ) -> Result<(), MailError> {
            Err(refused())
        }
        async fn ensure_label(&self, _mb: &MailboxCtx, _name: &str) -> Result<String, MailError> {
            Err(refused())
        }
        async fn send_mailto(&self, _mb: &MailboxCtx, _to: &MailtoTarget) -> Result<(), MailError> {
            Err(refused())
        }
        async fn inbox_count(&self, _mb: &MailboxCtx) -> Result<u64, MailError> {
            Err(refused())
        }
        async fn list_messages(
            &self,
            _mb: &MailboxCtx,
            _q: &MessageQuery,
            _page: Option<PageToken>,
            _max: u32,
        ) -> Result<MessagePage, MailError> {
            Err(refused())
        }
        async fn count_messages(
            &self,
            _mb: &MailboxCtx,
            _q: &MessageQuery,
        ) -> Result<u64, MailError> {
            Err(refused())
        }
        async fn rename_label(
            &self,
            _mb: &MailboxCtx,
            _label_id: &str,
            _new_name: &str,
        ) -> Result<(), MailError> {
            Err(refused())
        }
        async fn remove_label(&self, _mb: &MailboxCtx, _label_id: &str) -> Result<(), MailError> {
            Err(refused())
        }
        fn web_url(&self, _mailbox_address: &str, _id: &MessageId) -> String {
            String::new()
        }
    }

    /// The worker has no Drive access (S4 2).
    pub struct NoAppFolder;

    #[async_trait]
    impl AppFolderStore for NoAppFolder {
        async fn read(&self, _mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
            Err(refused())
        }
        async fn write(
            &self,
            _mb: &MailboxCtx,
            _bytes: &[u8],
            _if_match: Option<&ETag>,
        ) -> Result<ETag, AppFolderError> {
            Err(AppFolderError::Mail(refused()))
        }
        async fn delete(&self, _mb: &MailboxCtx) -> Result<(), MailError> {
            Err(refused())
        }
    }

    /// The worker serves no sign-in route.
    pub struct NoIdentity;

    #[async_trait]
    impl IdentityProvider for NoIdentity {
        fn authorize_url(&self, _req: &AuthRequest) -> Url {
            stub_url()
        }
        async fn exchange(
            &self,
            _code: &str,
            _verifier: &Sensitive<String>,
            _redirect_uri: &Url,
        ) -> Result<TokenSet, IdError> {
            Err(IdError::Unavailable)
        }
        async fn validate_id_token(
            &self,
            _raw: &Sensitive<String>,
            _nonce: &str,
        ) -> Result<IdClaims, IdError> {
            Err(IdError::Unavailable)
        }
        async fn refresh(
            &self,
            _refresh: &Sensitive<String>,
        ) -> Result<Sensitive<String>, IdError> {
            Err(IdError::Unavailable)
        }
        async fn revoke(&self, _token: &Sensitive<String>) -> Result<(), IdError> {
            Err(IdError::Unavailable)
        }
    }

    /// The worker sends no invite email.
    pub struct NoInviteMailer;

    #[async_trait]
    impl InviteMailer for NoInviteMailer {
        async fn send_invite(
            &self,
            _mb: &MailboxCtx,
            _to: &EmailAddress,
            _link: &InviteLink,
        ) -> Result<(), MailError> {
            Err(refused())
        }
    }

    /// The worker schedules no Cloud Tasks jobs.
    pub struct NoScheduler;

    #[async_trait]
    impl JobScheduler for NoScheduler {
        async fn schedule(
            &self,
            _job: &JobId,
            _due_at: OffsetDateTime,
        ) -> Result<TaskName, SchedError> {
            Err(SchedError::Unavailable)
        }
        async fn cancel(&self, _task: &TaskName) -> Result<CancelOutcome, SchedError> {
            Err(SchedError::Unavailable)
        }
    }
}
