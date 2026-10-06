//! `unsub` configuration from the environment (T-701).
//!
//! The two values that matter are the audience Cloud Tasks signs the OIDC
//! token for (`UNSUB_AUDIENCE`) and the service account email Cloud Tasks
//! runs as (`UNSUB_TASKS_CALLER`); both are required, so a misconfigured
//! service refuses every call rather than trusting the wrong caller.

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
