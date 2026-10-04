//! The `KeyService` and `SystemKeyService` ports: KMS-wrapped user keys.

use std::fmt;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::UserId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// KMS ciphertext of a user's `data_key`. Debug prints only the length.
#[derive(Clone, PartialEq, Eq)]
pub struct WrappedKey(pub Vec<u8>);

impl fmt::Debug for WrappedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WrappedKey").field(&self.0.len()).finish()
    }
}

impl Serialize for WrappedKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for WrappedKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)?;
        Ok(Self(bytes))
    }
}

/// Associated data for one encrypted value. Field names are constants, never
/// user input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aad {
    pub user: UserId,
    pub scope: String,
    pub field: &'static str,
}

/// Associated data for values encrypted before any user exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemAad {
    pub scope: String,
    pub field: &'static str,
}

/// A key service error. `OpenFailed` deliberately does not say whether the
/// key, the AAD or the tag was wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("key service unavailable")]
    Unavailable,
    #[error("open failed")]
    OpenFailed,
    #[error("malformed ciphertext")]
    Malformed,
    #[error("unsupported scheme version {0}")]
    UnsupportedVersion(u8),
    #[error("denied")]
    Denied,
}

/// Seals and opens values under a user's KMS-wrapped `data_key`.
#[async_trait]
pub trait KeyService: Send + Sync {
    async fn new_user_key(&self, user: &UserId) -> Result<WrappedKey, KeyError>;
    async fn seal(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError>;
    async fn open(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, KeyError>;
}

/// Encrypts the few fields that exist before a user does: invite and
/// invite-request email addresses and `pre_auth` session fields (S3).
#[async_trait]
pub trait SystemKeyService: Send + Sync {
    async fn seal(&self, aad: &SystemAad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError>;
    async fn open(&self, aad: &SystemAad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError>;
}
