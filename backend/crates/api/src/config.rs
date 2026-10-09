//! API service configuration.

use obs::Sensitive;
use url::Url;

/// A configuration failure. Never carries a value, only the variable name.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
}

/// Runtime configuration for the API service.
#[derive(Clone)]
pub struct ApiConfig {
    /// Exact app origin, e.g. `https://mailtinder.example.com`; no trailing slash.
    pub app_origin: String,
    /// Google OAuth client ID (public).
    pub google_client_id: String,
    /// OAuth redirect URI: `{app_origin}/api/v1/auth/google/callback`.
    pub oauth_redirect_uri: Url,
    /// Trusted proxy hops for `X-Forwarded-For` (default 1).
    pub xff_trusted_hops: usize,
    /// HMAC key for rate-limit keys (the log pseudonymisation key).
    pub rate_key: Sensitive<Vec<u8>>,
    /// HMAC key for email lookup (`Secrets::EmailLookupHmacKey`).
    pub email_lookup_key: Sensitive<Vec<u8>>,
    /// Maximum request body size in bytes (default 16 KiB).
    pub max_body_bytes: usize,
    /// The TCP port to bind (`PORT`, default 8080).
    pub port: u16,
}

/// The default port when `PORT` is unset.
pub const DEFAULT_PORT: u16 = 8080;

impl ApiConfig {
    /// Build a config with the default `max_body_bytes` (16 KiB), 1 trusted hop
    /// and the default port.
    /// # Errors
    /// Returns a URL parse error when the configured app origin is invalid.
    #[must_use = "configuration must be applied"]
    pub fn new(
        app_origin: String,
        google_client_id: String,
        rate_key: Sensitive<Vec<u8>>,
        email_lookup_key: Sensitive<Vec<u8>>,
    ) -> Result<Self, url::ParseError> {
        let oauth_redirect_uri = Url::parse(&format!("{app_origin}/api/v1/auth/google/callback"))?;
        Ok(Self {
            app_origin,
            google_client_id,
            oauth_redirect_uri,
            xff_trusted_hops: 1,
            rate_key,
            email_lookup_key,
            max_body_bytes: 16 * 1024,
            port: DEFAULT_PORT,
        })
    }

    /// Build a config from a variable lookup, so callers and tests never touch
    /// the process environment directly.
    ///
    /// Reads `APP_ORIGIN`, `GOOGLE_OAUTH_CLIENT_ID` and `PORT`. The HMAC keys
    /// are left empty; [`ApiConfig::with_keys`] fills them from `Secrets`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Missing`] for an absent/empty required variable
    /// and [`ConfigError::Invalid`] for a present but malformed one.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let app_origin = lookup("APP_ORIGIN").ok_or(ConfigError::Missing("APP_ORIGIN"))?;
        validate_origin(&app_origin)?;

        let google_client_id = lookup("GOOGLE_OAUTH_CLIENT_ID")
            .filter(|v| !v.trim().is_empty())
            .ok_or(ConfigError::Missing("GOOGLE_OAUTH_CLIENT_ID"))?;

        let port = match lookup("PORT") {
            None => DEFAULT_PORT,
            Some(raw) if raw.trim().is_empty() => DEFAULT_PORT,
            Some(raw) => raw
                .trim()
                .parse::<u16>()
                .map_err(|_| ConfigError::Invalid("PORT"))?,
        };

        let oauth_redirect_uri = Url::parse(&format!("{app_origin}/api/v1/auth/google/callback"))
            .map_err(|_| ConfigError::Invalid("APP_ORIGIN"))?;

        Ok(Self {
            app_origin,
            google_client_id,
            oauth_redirect_uri,
            xff_trusted_hops: 1,
            rate_key: Sensitive::new(Vec::new()),
            email_lookup_key: Sensitive::new(Vec::new()),
            max_body_bytes: 16 * 1024,
            port,
        })
    }

    /// Build a config from the process environment.
    ///
    /// # Errors
    ///
    /// As [`ApiConfig::from_lookup`].
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Replace the empty HMAC keys with the values fetched from `Secrets`.
    #[must_use]
    pub fn with_keys(
        mut self,
        rate_key: Sensitive<Vec<u8>>,
        email_lookup_key: Sensitive<Vec<u8>>,
    ) -> Self {
        self.rate_key = rate_key;
        self.email_lookup_key = email_lookup_key;
        self
    }
}

/// Validate an exact app origin: a scheme (`https`, or `http` for loopback
/// only), a host, no credentials, no trailing slash, no path, no query and no
/// fragment.
fn validate_origin(raw: &str) -> Result<(), ConfigError> {
    if raw.is_empty() || raw.ends_with('/') {
        return Err(ConfigError::Invalid("APP_ORIGIN"));
    }
    let url = Url::parse(raw).map_err(|_| ConfigError::Invalid("APP_ORIGIN"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(ConfigError::Invalid("APP_ORIGIN"));
    }
    let Some(host) = url.host_str() else {
        return Err(ConfigError::Invalid("APP_ORIGIN"));
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
            if loopback {
                Ok(())
            } else {
                Err(ConfigError::Invalid("APP_ORIGIN"))
            }
        }
        _ => Err(ConfigError::Invalid("APP_ORIGIN")),
    }
}
