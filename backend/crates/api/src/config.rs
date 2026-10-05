//! API service configuration.

use obs::Sensitive;
use url::Url;

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
}

impl ApiConfig {
    /// Build a config with the default `max_body_bytes` (16 KiB) and 1 trusted hop.
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
        })
    }
}
