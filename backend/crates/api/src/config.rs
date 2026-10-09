//! API service configuration.

use std::net::{IpAddr, Ipv4Addr};

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

/// The start-up mode. It selects which environment rules apply: production is
/// strict; e2e relaxes exactly the rules the local test bed needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Production: `https` origins only, every interface by default.
    Production,
    /// End-to-end tests: loopback `http` origins, listener forced to loopback.
    E2e,
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
    /// The listener host (`API_BIND_HOST`). Every interface in production,
    /// loopback in e2e mode.
    pub bind_host: IpAddr,
}

/// The default port when `PORT` is unset.
pub const DEFAULT_PORT: u16 = 8080;

/// The default listener host: every interface.
pub const DEFAULT_BIND_HOST: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// The e2e listener host: loopback only.
pub const E2E_BIND_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

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
            bind_host: DEFAULT_BIND_HOST,
        })
    }

    /// Build a config from a variable lookup, so callers and tests never touch
    /// the process environment directly.
    ///
    /// Reads `APP_ORIGIN`, `GOOGLE_OAUTH_CLIENT_ID`, `PORT` and
    /// `API_BIND_HOST`. An absent or blank required variable is
    /// [`ConfigError::Missing`]; a present but malformed one is
    /// [`ConfigError::Invalid`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Missing`] for an absent/empty required variable
    /// and [`ConfigError::Invalid`] for a present but malformed one.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        mode: Mode,
    ) -> Result<Self, ConfigError> {
        let app_origin = required(&lookup, "APP_ORIGIN")?;
        validate_origin(&app_origin, mode)?;

        let google_client_id = required(&lookup, "GOOGLE_OAUTH_CLIENT_ID")?;

        let port = match lookup("PORT") {
            None => DEFAULT_PORT,
            Some(raw) if raw.trim().is_empty() => DEFAULT_PORT,
            Some(raw) => raw
                .trim()
                .parse::<u16>()
                .map_err(|_| ConfigError::Invalid("PORT"))?,
        };

        let bind_host = bind_host(&lookup, mode)?;

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
            bind_host,
        })
    }

    /// Build a config from the process environment.
    ///
    /// # Errors
    ///
    /// As [`ApiConfig::from_lookup`].
    pub fn from_env(mode: Mode) -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok(), mode)
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
/// hosts in e2e mode only), a host, no credentials, no trailing slash, no
/// path, no query and no fragment.
fn validate_origin(raw: &str, mode: Mode) -> Result<(), ConfigError> {
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
        "http" if mode == Mode::E2e && is_loopback_host(host) => Ok(()),
        _ => Err(ConfigError::Invalid("APP_ORIGIN")),
    }
}

/// A loopback host as it appears in a URL (bracketed IPv6 included).
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// A required, non-empty variable. An absent or blank value is `Missing`.
fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    lookup(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

/// The listener host: every interface by default in production, forced to
/// loopback in e2e mode where any other value is invalid.
fn bind_host(lookup: &impl Fn(&str) -> Option<String>, mode: Mode) -> Result<IpAddr, ConfigError> {
    let raw = lookup("API_BIND_HOST").filter(|v| !v.trim().is_empty());
    match (mode, raw) {
        (Mode::Production, None) => Ok(DEFAULT_BIND_HOST),
        (Mode::E2e, None) => Ok(E2E_BIND_HOST),
        (Mode::E2e, Some(raw)) => match raw.trim().parse::<IpAddr>() {
            Ok(ip) if ip == E2E_BIND_HOST => Ok(E2E_BIND_HOST),
            _ => Err(ConfigError::Invalid("API_BIND_HOST")),
        },
        (Mode::Production, Some(raw)) => raw
            .trim()
            .parse::<IpAddr>()
            .map_err(|_| ConfigError::Invalid("API_BIND_HOST")),
    }
}
