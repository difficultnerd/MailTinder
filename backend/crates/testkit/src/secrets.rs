//! A fake `Secrets` with a mutable map.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use obs::Sensitive;
use ports::{SecretError, SecretName, Secrets};

/// A fake secrets store.
pub struct FakeSecrets {
    map: Mutex<HashMap<SecretName, Vec<u8>>>,
}

impl FakeSecrets {
    pub fn with_defaults() -> Self {
        let s = Self {
            map: Mutex::new(HashMap::new()),
        };
        s.set(SecretName::GoogleOAuthClientSecret, b"fake-client-secret");
        s.set(SecretName::JevApiKey, b"fake-jev-key");
        s.set(SecretName::EmailLookupHmacKey, b"fake-email-hmac-key");
        s.set(SecretName::LogPseudonymHmacKey, b"fake-log-hmac-key");
        s
    }

    pub fn set(&self, n: SecretName, v: &[u8]) {
        self.map
            .lock()
            .unwrap_or_else(|_| panic!("secrets poisoned"))
            .insert(n, v.to_vec());
    }

    pub fn remove(&self, n: SecretName) {
        self.map
            .lock()
            .unwrap_or_else(|_| panic!("secrets poisoned"))
            .remove(&n);
    }
}

#[async_trait]
impl Secrets for FakeSecrets {
    async fn get(&self, name: SecretName) -> Result<Sensitive<Vec<u8>>, SecretError> {
        let map = self
            .map
            .lock()
            .unwrap_or_else(|_| panic!("secrets poisoned"));
        map.get(&name)
            .cloned()
            .map(Sensitive::new)
            .ok_or(SecretError::Missing)
    }
}
