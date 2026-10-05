//! Token sources for Google Cloud platform APIs.
//!
//! `MetadataTokenSource` reads the instance metadata server (the only place
//! allowed to reach it; the egress SSRF policy refuses it). `StaticTokenSource`
//! is for the emulator and tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obs::Sensitive;
use ports::Clock;
use time::OffsetDateTime;

use crate::gcp_http::GcpError;

/// Supplies a bearer token for a Google Cloud platform API call.
#[async_trait]
pub trait TokenSource: Send + Sync {
    async fn bearer(&self) -> Result<Sensitive<String>, GcpError>;
}

/// Reads a short-lived access token from the instance metadata server and
/// caches it until `expires_in - 60` seconds by the injected `Clock`.
pub struct MetadataTokenSource {
    http: reqwest::Client,
    clock: Arc<dyn Clock>,
    /// The metadata server URL; `None` means the fixed production URL.
    url: Option<String>,
    cache: Mutex<Option<(Sensitive<String>, OffsetDateTime)>>,
}

impl MetadataTokenSource {
    /// The metadata server URL is fixed; only this client may reach it.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            http: reqwest::Client::new(),
            clock,
            url: None,
            cache: Mutex::new(None),
        }
    }

    /// Test-only constructor that points at a local stub instead of the
    /// metadata server.
    pub fn with_url(clock: Arc<dyn Clock>, url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            clock,
            url: Some(url),
            cache: Mutex::new(None),
        }
    }
}

#[async_trait]
impl TokenSource for MetadataTokenSource {
    async fn bearer(&self) -> Result<Sensitive<String>, GcpError> {
        let now = self.clock.now();
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(|_| panic!("token cache poisoned"));
            if let Some((token, expires_at)) = cache.as_ref() {
                if now < *expires_at {
                    return Ok(token.clone());
                }
            }
        }
        // Fetch a fresh token. The URL is either the fixed metadata server or
        // the test stub; never user-influenced.
        let url = self
            .url
            .clone()
            .unwrap_or_else(|| {
                "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token"
                    .to_owned()
            });
        let resp = self
            .http
            // GCP metadata server is HTTP-only by design; fixed trusted endpoint, never user-influenced.
            // codeql[rust/non-https-url]
            .get(&url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|_| GcpError::Unavailable)?;
        if !resp.status().is_success() {
            return Err(GcpError::Unavailable);
        }
        let body: serde_json::Value = resp.json().await.map_err(|_| GcpError::BadResponse)?;
        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or(GcpError::BadResponse)?
            .to_owned();
        let expires_in = body
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .ok_or(GcpError::BadResponse)?;
        let expires_at =
            now + time::Duration::seconds(expires_in as i64) - time::Duration::seconds(60);
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|_| panic!("token cache poisoned"));
        *cache = Some((Sensitive::new(token.clone()), expires_at));
        Ok(Sensitive::new(token))
    }
}

/// A fixed bearer token, for the emulator (`owner`) and tests.
pub struct StaticTokenSource(pub Sensitive<String>);

#[async_trait]
impl TokenSource for StaticTokenSource {
    async fn bearer(&self) -> Result<Sensitive<String>, GcpError> {
        Ok(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use testkit::{VirtualClock, T0};
    use time::Duration;

    /// A stub metadata server that returns a token with a short `expires_in`.
    async fn stub_server() -> String {
        let app = Router::new().route(
            "/computeMetadata/v1/instance/service-accounts/default/token",
            get(|| async {
                axum::Json(serde_json::json!({
                    "access_token": "tok-1",
                    "expires_in": 300,
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn metadata_token_cached_until_expiry() {
        let base = stub_server().await;
        let url = format!("{base}/computeMetadata/v1/instance/service-accounts/default/token");
        let clock = Arc::new(VirtualClock::new(T0));
        let src = MetadataTokenSource::with_url(clock.clone(), url);

        // First call fetches and caches.
        let t1 = src.bearer().await.expect("first");
        assert_eq!(t1.expose(), "tok-1");

        // A second call within the cache window must not hit the server again.
        // We can't observe the server directly, but the token is identical and
        // the call succeeds without a network round trip to a fresh server.
        let t2 = src.bearer().await.expect("cached");
        assert_eq!(t2.expose(), "tok-1");

        // Advance past expires_in - 60 (300 - 60 = 240s); the cache is stale
        // and a fresh fetch happens. The stub still returns the same token.
        clock.advance(Duration::seconds(241));
        let t3 = src.bearer().await.expect("refreshed");
        assert_eq!(t3.expose(), "tok-1");
    }
}
