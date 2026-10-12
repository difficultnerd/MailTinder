//! `unsub` configuration from the environment (T-701).
//!
//! The two values that matter are the audience Cloud Tasks signs the OIDC
//! token for (`UNSUB_AUDIENCE`) and the service account email Cloud Tasks
//! runs as (`UNSUB_TASKS_CALLER`); both are required, so a misconfigured
//! service refuses every call rather than trusting the wrong caller.
//!
//! [`ProductionEnv`] is everything `production_run` reads, extracted so the
//! service boot check (`api/tests/service_boot_env.rs`, T-1119) drives the same
//! loader start-up does.

use url::Url;

/// The default listen port when `PORT` is unset (Cloud Run sets it).
pub const DEFAULT_PORT: u16 = 8080;

/// A configuration failure at start-up.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable was missing or empty.
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    /// `PORT` was set but is not a valid port number.
    #[error("invalid PORT")]
    BadPort,
}

/// Everything `unsub` needs to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsubConfig {
    /// The `aud` every internal ID token must carry.
    pub audience: String,
    /// The only service account email allowed to call the internal route.
    pub tasks_caller: String,
    /// The listen port.
    pub port: u16,
}

impl UnsubConfig {
    /// Load the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Load the configuration from a lookup, so tests need no environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let audience =
            non_empty(lookup("UNSUB_AUDIENCE")).ok_or(ConfigError::Missing("UNSUB_AUDIENCE"))?;
        let tasks_caller = non_empty(lookup("UNSUB_TASKS_CALLER"))
            .ok_or(ConfigError::Missing("UNSUB_TASKS_CALLER"))?;
        let port = match non_empty(lookup("PORT")) {
            None => DEFAULT_PORT,
            Some(p) => p.parse::<u16>().map_err(|_| ConfigError::BadPort)?,
        };
        Ok(Self {
            audience,
            tasks_caller,
            port,
        })
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// A failure while reading the complete production environment (T-1119). Only
/// the variable name is ever surfaced.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProductionEnvError {
    /// A required environment variable was missing or empty.
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    /// A variable was present but malformed.
    #[error("invalid value for {0}")]
    Invalid(&'static str),
}

/// The complete production environment `production_run` reads (T-1119).
///
/// Every required variable of the start-up path is resolved here, and
/// `production_run` resolves it through [`ProductionEnv::from_env`], so the boot
/// check (`api/tests/service_boot_env.rs`) drives the same loader start-up does
/// and the two cannot diverge.
pub struct ProductionEnv {
    /// `UNSUB_AUDIENCE`, `UNSUB_TASKS_CALLER` and `PORT`.
    pub config: UnsubConfig,
    /// `GOOGLE_CLOUD_PROJECT`.
    pub project: String,
    /// `UNSUB_BASE_URL`, parsed: the base of the one-click links.
    pub unsub_base: Url,
    /// `GOOGLE_OAUTH_CLIENT_ID`: the Gmail identity flow's client id.
    pub client_id: String,
    /// `MT_KMS_KEY`: the full resource name of the KMS key that wraps every
    /// user's `data_key`. Terraform sets it in `base_env` and the KMS client is
    /// built from it, so it is required here: without it the service would
    /// silently unwrap against a hard-coded path a rename or a region change
    /// leaves stale (T-1119 review F1).
    pub kms_key: String,
}

impl ProductionEnv {
    /// Load the complete production environment from the process environment.
    ///
    /// # Errors
    ///
    /// As [`ProductionEnv::from_lookup`].
    pub fn from_env() -> Result<Self, ProductionEnvError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Load the complete production environment from a lookup, so tests need no
    /// environment.
    ///
    /// # Errors
    ///
    /// [`ProductionEnvError::Missing`] for an absent or blank required variable
    /// and [`ProductionEnvError::Invalid`] for a present but malformed one.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ProductionEnvError> {
        let config = UnsubConfig::from_lookup(&lookup).map_err(|error| match error {
            ConfigError::Missing(name) => ProductionEnvError::Missing(name),
            ConfigError::BadPort => ProductionEnvError::Invalid("PORT"),
        })?;
        let project = required(&lookup, "GOOGLE_CLOUD_PROJECT")?;
        let unsub_base = Url::parse(&required(&lookup, "UNSUB_BASE_URL")?)
            .map_err(|_| ProductionEnvError::Invalid("UNSUB_BASE_URL"))?;
        let client_id = required(&lookup, "GOOGLE_OAUTH_CLIENT_ID")?;
        let kms_key = required(&lookup, "MT_KMS_KEY")?;
        Ok(Self {
            config,
            project,
            unsub_base,
            client_id,
            kms_key,
        })
    }
}

/// A required, non-empty value from a variable lookup. Only the name is
/// surfaced.
fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ProductionEnvError> {
    non_empty(lookup(name)).ok_or(ProductionEnvError::Missing(name))
}
