//! The production `SystemKeyService`, using a separate KMS key
//! (`.../cryptoKeys/system-fields`) for the few fields that exist before a
//! user does (S6 section 5, T-302).

use std::sync::Arc;

use async_trait::async_trait;
use ports::{KeyError, SystemAad, SystemKeyService};

use super::kms::KmsApi;
use super::{encode_system_aad, SCHEME_V1};

/// Largest plaintext the system key seals (Cloud KMS direct-encrypt limit).
const MAX_SYSTEM_PLAINTEXT: usize = 64 * 1024;

/// The production `SystemKeyService`, encrypting directly with a dedicated
/// `data_key` KMS key so S6's "the KEK wraps every `data_key` and nothing else"
/// stays true.
pub struct KmsSystemKeyService {
    kms: Arc<dyn KmsApi>,
}

impl KmsSystemKeyService {
    /// Builds the service with the system-fields KMS key.
    pub fn new(kms: Arc<dyn KmsApi>) -> Self {
        Self { kms }
    }
}

#[async_trait]
impl SystemKeyService for KmsSystemKeyService {
    async fn seal(&self, aad: &SystemAad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError> {
        if plaintext.len() > MAX_SYSTEM_PLAINTEXT {
            return Err(KeyError::Malformed);
        }
        let kms_ct = self.kms.encrypt(plaintext, &encode_system_aad(aad)).await?;
        let mut out = Vec::with_capacity(1 + kms_ct.len());
        out.push(SCHEME_V1);
        out.extend_from_slice(&kms_ct);
        Ok(out)
    }

    async fn open(&self, aad: &SystemAad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError> {
        if ciphertext.is_empty() || ciphertext[0] != SCHEME_V1 {
            return Err(KeyError::UnsupportedVersion(
                ciphertext.first().copied().unwrap_or(0),
            ));
        }
        let pt = self
            .kms
            .decrypt(&ciphertext[1..], &encode_system_aad(aad))
            .await?;
        Ok(pt.to_vec())
    }
}
