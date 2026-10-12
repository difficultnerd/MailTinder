//! The worker's complete production environment (T-1119).
//!
//! Everything `run` reads from the environment is resolved here, so the service
//! boot check (`api/tests/service_boot_env.rs`) drives the same loader start-up
//! does and the two cannot diverge: a required variable added to the start-up
//! path must go through [`ProductionEnv`] to be covered.

/// The default listen port when `PORT` is unset (Cloud Run sets it).
pub const DEFAULT_PORT: u16 = 8080;

/// A failure while reading the production environment. Only the variable name
/// is ever surfaced.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProductionEnvError {
    /// A required environment variable was missing or empty.
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    /// A variable was present but malformed.
    #[error("invalid value for {0}")]
    Invalid(&'static str),
}

/// Everything `run` needs to start.
pub struct ProductionEnv {
    /// `WORKER_AUDIENCE`: the `aud` every internal ID token must carry.
    pub audience: String,
    /// `WORKER_SCHEDULER_CALLER`: the only service account allowed to call the
    /// internal route.
    pub scheduler_caller: String,
    /// The listen port (`PORT`, default [`DEFAULT_PORT`]).
    pub port: u16,
    /// `GOOGLE_CLOUD_PROJECT`.
    pub project: String,
}

impl ProductionEnv {
    /// Load the complete production environment from the process environment.
    ///
    /// # Errors
    ///
    /// As [`ProductionEnv::from_lookup`].
    pub fn from_env() -> Result<Self, ProductionEnvError> {
        Self::from_lookup(|name| std::env::var(name).ok())
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
        let audience = required(&lookup, "WORKER_AUDIENCE")?;
        let scheduler_caller = required(&lookup, "WORKER_SCHEDULER_CALLER")?;
        let port = match non_empty(lookup("PORT")) {
            None => DEFAULT_PORT,
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| ProductionEnvError::Invalid("PORT"))?,
        };
        let project = required(&lookup, "GOOGLE_CLOUD_PROJECT")?;
        Ok(Self {
            audience,
            scheduler_caller,
            port,
            project,
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

/// A value that is present and not blank.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}
