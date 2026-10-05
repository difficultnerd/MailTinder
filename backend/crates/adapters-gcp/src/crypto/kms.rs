//! Cloud KMS REST client (`KmsApi`) used by the envelope and system key
//! services. All JSON is built with `serde_json::json!`; keys and ciphertext
//! are never logged.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use ports::KeyError;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::gcp_http::{GcpError, GcpHttp};

/// The KMS REST API surface the envelope and system key services need.
#[async_trait]
pub trait KmsApi: Send + Sync {
    /// Encrypt `plaintext` with the KMS key, binding `aad`.
    async fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, KeyError>;
    /// Decrypt `ciphertext` with the KMS key, requiring `aad`.
    async fn decrypt(
        &self,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyError>;
}

/// The production KMS client, talking to Cloud KMS over `GcpHttp`.
///
/// `key_name` is the full resource name, for example
/// `projects/{p}/locations/us-central1/keyRings/{r}/cryptoKeys/{k}`.
pub struct CloudKms {
    http: Arc<GcpHttp>,
    key_name: String,
}

impl CloudKms {
    /// Builds a client for the given crypto key.
    pub fn new(http: Arc<GcpHttp>, key_name: String) -> Self {
        Self { http, key_name }
    }

    fn base_url(&self) -> Url {
        match self.http.emulator() {
            Some(addr) => Url::parse(&format!("http://{addr}/v1/{}", self.key_name))
                .unwrap_or_else(|_| panic!("valid kms base url")),
            None => Url::parse(&format!(
                "https://cloudkms.googleapis.com/v1/{}",
                self.key_name
            ))
            .unwrap_or_else(|_| panic!("valid kms base url")),
        }
    }
}

#[derive(Deserialize)]
struct EncryptResponse {
    ciphertext: String,
    #[serde(rename = "ciphertextCrc32c")]
    ciphertext_crc32c: String,
    #[serde(rename = "verifiedPlaintextCrc32c", default)]
    verified_plaintext_crc32c: bool,
    #[serde(rename = "verifiedAdditionalAuthenticatedDataCrc32c", default)]
    verified_additional_authenticated_data_crc32c: bool,
}

#[derive(Deserialize)]
struct DecryptResponse {
    plaintext: String,
    #[serde(rename = "plaintextCrc32c")]
    plaintext_crc32c: String,
}

#[async_trait]
impl KmsApi for CloudKms {
    async fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, KeyError> {
        let mut url = self.base_url();
        url.set_path(&format!("{}:encrypt", self.base_url().path()));
        let body = json!({
            "plaintext": b64(plaintext),
            "additionalAuthenticatedData": b64(aad),
            "plaintextCrc32c": crc32c::crc32c(plaintext).to_string(),
            "additionalAuthenticatedDataCrc32c": crc32c::crc32c(aad).to_string(),
        });
        let resp: EncryptResponse = self
            .http
            .json(Method::POST, &url, Some(&body))
            .await
            .map_err(map_encrypt_err)?;
        // Verify the CRC32C integrity fields the API echoes back.
        if !resp.verified_plaintext_crc32c || !resp.verified_additional_authenticated_data_crc32c {
            return Err(KeyError::Unavailable);
        }
        let ct = b64_decode(&resp.ciphertext).map_err(|_| KeyError::Unavailable)?;
        let expected = resp
            .ciphertext_crc32c
            .parse::<u32>()
            .map_err(|_| KeyError::Unavailable)?;
        if crc32c::crc32c(&ct) != expected {
            return Err(KeyError::Unavailable);
        }
        Ok(ct)
    }

    async fn decrypt(
        &self,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyError> {
        let mut url = self.base_url();
        url.set_path(&format!("{}:decrypt", self.base_url().path()));
        let body = json!({
            "ciphertext": b64(ciphertext),
            "additionalAuthenticatedData": b64(aad),
            "ciphertextCrc32c": crc32c::crc32c(ciphertext).to_string(),
            "additionalAuthenticatedDataCrc32c": crc32c::crc32c(aad).to_string(),
        });
        let resp: DecryptResponse = self
            .http
            .json(Method::POST, &url, Some(&body))
            .await
            .map_err(map_decrypt_err)?;
        let pt = b64_decode(&resp.plaintext).map_err(|_| KeyError::OpenFailed)?;
        let expected = resp
            .plaintext_crc32c
            .parse::<u32>()
            .map_err(|_| KeyError::OpenFailed)?;
        if crc32c::crc32c(&pt) != expected {
            return Err(KeyError::OpenFailed);
        }
        Ok(zeroize::Zeroizing::new(pt))
    }
}

fn map_encrypt_err(e: GcpError) -> KeyError {
    match e {
        GcpError::Unavailable => KeyError::Unavailable,
        GcpError::PermissionDenied | GcpError::NotFound => KeyError::Denied,
        _ => KeyError::Unavailable,
    }
}

fn map_decrypt_err(e: GcpError) -> KeyError {
    match e {
        // A 400 INVALID_ARGUMENT (wrong AAD, corrupt ciphertext) surfaces as
        // BadResponse through GcpHttp; it is an open failure.
        GcpError::BadResponse => KeyError::OpenFailed,
        GcpError::Unavailable => KeyError::Unavailable,
        GcpError::PermissionDenied | GcpError::NotFound => KeyError::Denied,
        _ => KeyError::OpenFailed,
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(s)
}
