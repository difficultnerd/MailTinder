//! The one HTTP client for Google Cloud platform APIs.
//!
//! Refuses any URL whose host is not in `PLATFORM_HOSTS` (or the emulator
//! socket over http). This is not the `HttpEgress` allowlist (T-306):
//! user-influenced URLs never reach `GcpHttp`.

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::token_source::TokenSource;

/// The only production hosts this client may reach.
pub const PLATFORM_HOSTS: [&str; 4] = [
    "firestore.googleapis.com",
    "cloudkms.googleapis.com",
    "secretmanager.googleapis.com",
    "cloudtasks.googleapis.com",
];

/// A Google Cloud platform API error, mapped from the REST error envelope.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GcpError {
    #[error("host not allowed")]
    HostNotAllowed,
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("permission denied")]
    PermissionDenied,
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    AlreadyExists,
    #[error("failed precondition")]
    FailedPrecondition,
    #[error("conflict")]
    Aborted,
    #[error("unavailable")]
    Unavailable,
    #[error("bad response")]
    BadResponse,
}

/// The shared HTTP client for Google Cloud platform APIs.
pub struct GcpHttp {
    client: reqwest::Client,
    tokens: Arc<dyn TokenSource>,
    emulator: Option<SocketAddr>,
}

impl GcpHttp {
    /// Production client: https only, no redirects, 10-second timeout, rustls.
    ///
    /// # Errors
    ///
    /// Returns `HostNotAllowed` if the client cannot be built (never in
    /// practice; kept for signature symmetry).
    pub fn new(tokens: Arc<dyn TokenSource>) -> Result<Self, GcpError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| GcpError::BadResponse)?;
        Ok(Self {
            client,
            tokens,
            emulator: None,
        })
    }

    /// Emulator client: allows http to the emulator socket.
    ///
    /// # Errors
    ///
    /// Returns `HostNotAllowed` if the client cannot be built.
    pub fn with_emulator(
        tokens: Arc<dyn TokenSource>,
        emulator: SocketAddr,
    ) -> Result<Self, GcpError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(false)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| GcpError::BadResponse)?;
        Ok(Self {
            client,
            tokens,
            emulator: Some(emulator),
        })
    }

    /// The emulator socket, if this client is configured for one.
    pub fn emulator(&self) -> Option<SocketAddr> {
        self.emulator
    }

    /// Performs a JSON request, refusing any URL whose host is not allowed.
    ///
    /// # Errors
    ///
    /// Returns a `GcpError` for refused hosts, transport failures and
    /// non-success status codes.
    pub async fn json<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        url: &Url,
        body: Option<&B>,
    ) -> Result<T, GcpError> {
        self.check_host(url)?;
        let token = self.tokens.bearer().await?;
        let mut req = self
            .client
            .request(method, url.clone())
            .bearer_auth(token.expose());
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|_| GcpError::Unavailable)?;
        let status = resp.status();
        if status.is_success() {
            return resp.json().await.map_err(|_| GcpError::BadResponse);
        }
        // Map the error envelope.
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let code = body
            .pointer("/error/status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Err(map_status(status.as_u16(), code, message))
    }

    /// Performs a request that expects no response body (e.g. a `DELETE`),
    /// refusing any URL whose host is not allowed. A 2xx status is success;
    /// other statuses map to `GcpError` from the error envelope.
    ///
    /// # Errors
    ///
    /// Returns a `GcpError` for refused hosts, transport failures and
    /// non-success status codes.
    pub async fn send(&self, method: Method, url: &Url) -> Result<(), GcpError> {
        self.check_host(url)?;
        let token = self.tokens.bearer().await?;
        let resp = self
            .client
            .request(method, url.clone())
            .bearer_auth(token.expose())
            .send()
            .await
            .map_err(|_| GcpError::Unavailable)?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let code = body
            .pointer("/error/status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Err(map_status(status.as_u16(), code, message))
    }

    fn check_host(&self, url: &Url) -> Result<(), GcpError> {
        if let Some(emulator) = self.emulator {
            if url.scheme() == "http"
                && url.host_str() == Some(&emulator.ip().to_string())
                && url.port() == Some(emulator.port())
            {
                return Ok(());
            }
        }
        let host = url.host_str().ok_or(GcpError::HostNotAllowed)?;
        if PLATFORM_HOSTS.contains(&host) {
            return Ok(());
        }
        Err(GcpError::HostNotAllowed)
    }
}

fn map_status(status: u16, code: &str, _message: &str) -> GcpError {
    match (status, code) {
        (401, _) => GcpError::Unauthenticated,
        (403, _) => GcpError::PermissionDenied,
        (404, _) => GcpError::NotFound,
        (409, "ALREADY_EXISTS") => GcpError::AlreadyExists,
        (409, _) => GcpError::Aborted,
        (400, "FAILED_PRECONDITION") | (412, _) => GcpError::FailedPrecondition,
        (429, _) | (500, _) | (502, _) | (503, _) | (504, _) => GcpError::Unavailable,
        _ => GcpError::BadResponse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_source::StaticTokenSource;
    use obs::Sensitive;

    fn client() -> GcpHttp {
        let tokens: Arc<dyn TokenSource> =
            Arc::new(StaticTokenSource(Sensitive::new("t".to_owned())));
        GcpHttp::new(tokens).expect("client")
    }

    #[tokio::test]
    async fn gcp_http_refuses_non_platform_host() {
        let c = client();
        let url = Url::parse("https://evil.example.com/v1/x").expect("url");
        let err = c
            .json::<serde_json::Value, serde_json::Value>(
                Method::GET,
                &url,
                None::<&serde_json::Value>,
            )
            .await
            .expect_err("must refuse");
        assert_eq!(err, GcpError::HostNotAllowed);
    }

    #[tokio::test]
    async fn gcp_http_allows_platform_host() {
        let c = client();
        // firestore.googleapis.com is allowed; the call fails on transport
        // (no network in unit tests) but not on host refusal.
        let url = Url::parse("https://firestore.googleapis.com/v1/projects/x").expect("url");
        let err = c
            .json::<serde_json::Value, serde_json::Value>(
                Method::GET,
                &url,
                None::<&serde_json::Value>,
            )
            .await
            .expect_err("transport failure expected");
        assert_ne!(err, GcpError::HostNotAllowed);
    }
}
