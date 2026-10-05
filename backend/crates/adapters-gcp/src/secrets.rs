//! The production `Secrets` port: reads the four secrets from Secret Manager
//! once and keeps them in memory.
//!
//! Values are read once per process (rotation means adding a version and
//! restarting the service, S4 5.7). The two HMAC keys rotate yearly; the old
//! version is still needed to find old records, but that multi-version read
//! is out of scope here and reported as a gap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use obs::Sensitive;
use ports::{SecretError, SecretName, Secrets};
use reqwest::Method;
use serde::Deserialize;
use url::Url;

use crate::gcp_http::{GcpError, GcpHttp};

/// Configuration for the Secret Manager client.
#[derive(Clone, Debug)]
pub struct SecretsConfig {
    pub project_id: String,
}

/// The production `Secrets` over the Secret Manager REST API.
pub struct SecretManagerSecrets {
    http: Arc<GcpHttp>,
    cfg: SecretsConfig,
    /// `https://secretmanager.googleapis.com/v1/`.
    base: Url,
    cache: Mutex<HashMap<SecretName, Sensitive<Vec<u8>>>>,
}

impl SecretManagerSecrets {
    /// Production client against the real Secret Manager API.
    pub fn new(http: Arc<GcpHttp>, cfg: SecretsConfig) -> Self {
        let base = Url::parse("https://secretmanager.googleapis.com/v1/")
            .unwrap_or_else(|_| panic!("valid base url"));
        Self {
            http,
            cfg,
            base,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Test-only constructor pointing at a local stub.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base(http: Arc<GcpHttp>, cfg: SecretsConfig, base: Url) -> Self {
        Self {
            http,
            cfg,
            base,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Load the secrets this service needs at start-up; fail start-up if any
    /// is missing.
    ///
    /// # Errors
    ///
    /// Returns the first `SecretError` encountered.
    pub async fn preload(&self, names: &[SecretName]) -> Result<(), SecretError> {
        for name in names {
            self.get(*name).await?;
        }
        Ok(())
    }

    /// The access URL for the latest version of a secret.
    fn access_url(&self, name: SecretName) -> Url {
        let mut url = self.base.clone();
        url.set_path(&format!(
            "v1/projects/{}/secrets/{}/versions/latest:access",
            self.cfg.project_id,
            name.secret_id()
        ));
        url
    }
}

/// The Secret Manager `access` response.
#[derive(Deserialize)]
struct AccessResponse {
    #[serde(default)]
    payload: Payload,
}

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    data: String,
    #[serde(default, rename = "dataCrc32c")]
    data_crc32c: Option<String>,
}

#[async_trait]
impl Secrets for SecretManagerSecrets {
    async fn get(&self, name: SecretName) -> Result<Sensitive<Vec<u8>>, SecretError> {
        if let Some(v) = self
            .cache
            .lock()
            .unwrap_or_else(|_| panic!("cache poisoned"))
            .get(&name)
        {
            return Ok(v.clone());
        }
        let url = self.access_url(name);
        let resp: AccessResponse = match self
            .http
            .json::<serde_json::Value, AccessResponse>(
                Method::GET,
                &url,
                None::<&serde_json::Value>,
            )
            .await
        {
            Ok(r) => r,
            Err(GcpError::NotFound) => return Err(SecretError::Missing),
            Err(GcpError::PermissionDenied) | Err(GcpError::Unauthenticated) => {
                return Err(SecretError::Denied)
            }
            Err(_) => return Err(SecretError::Unavailable),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(resp.payload.data)
            .map_err(|_| SecretError::Unavailable)?;
        if let Some(crc) = resp.payload.data_crc32c {
            let expected: u32 = crc.parse().map_err(|_| SecretError::Unavailable)?;
            if crc32c::crc32c(&bytes) != expected {
                return Err(SecretError::Unavailable);
            }
        }
        let value = Sensitive::new(bytes);
        self.cache
            .lock()
            .unwrap_or_else(|_| panic!("cache poisoned"))
            .insert(name, value.clone());
        Ok(value)
    }
}
