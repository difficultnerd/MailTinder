//! Fake key services with real AES-256-GCM and an in-memory key wrap.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use domain::UserId;
use ports::{Aad, KeyError, KeyService, Rng, SystemAad, SystemKeyService, WrappedKey};

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A fake `KeyService` with real AES-256-GCM and an in-memory key wrap.
pub struct FakeKeyService {
    kek: [u8; 32],
    rng: Arc<dyn Rng>,
    calls: AtomicU64,
    /// One-shot failures armed per AAD `field` (see [`FakeKeyService::fail_field`]).
    fail_fields: Mutex<HashMap<String, u32>>,
}

impl FakeKeyService {
    pub fn new(rng: Arc<dyn Rng>) -> Self {
        let kek = rng.bytes32();
        Self {
            kek,
            rng,
            calls: AtomicU64::new(0),
            fail_fields: Mutex::new(HashMap::new()),
        }
    }

    pub fn unwrap_calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// Make the next `n` `seal`/`open` calls whose AAD `field` equals `field`
    /// fail with `KeyError::Unavailable`, so a test can reach the failure path
    /// around one specific sealed value (for example the refresh token of a
    /// mailbox, `aad_fields::MAILBOX_REFRESH_TOKEN`) without touching the
    /// others. Mirrors the one-shot `fail_*` hooks on the mailbox, app-folder
    /// and store fakes.
    pub fn fail_field(&self, field: &str, n: u32) {
        let mut armed = self
            .fail_fields
            .lock()
            .unwrap_or_else(|_| panic!("keys poisoned"));
        let slot = armed.entry(field.to_owned()).or_insert(0);
        *slot = slot.saturating_add(n);
    }

    /// Take one armed failure for `field`, if any is left.
    fn take_failure(&self, field: &str) -> Result<(), KeyError> {
        let mut armed = self
            .fail_fields
            .lock()
            .unwrap_or_else(|_| panic!("keys poisoned"));
        match armed.get_mut(field) {
            Some(left) if *left > 0 => {
                *left -= 1;
                Err(KeyError::Unavailable)
            }
            _ => Ok(()),
        }
    }

    fn unwrap(&self, wrapped: &WrappedKey, user: &UserId) -> Result<[u8; 32], KeyError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if wrapped.0.len() < NONCE_LEN + TAG_LEN {
            return Err(KeyError::Malformed);
        }
        let cipher = Aes256Gcm::new_from_slice(&self.kek).map_err(|_| KeyError::Unavailable)?;
        let (nonce, ct) = wrapped.0.split_at(NONCE_LEN);
        let aad = format!("fake.dek|{}", hex(user.0.as_bytes()));
        let pt = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| KeyError::OpenFailed)?;
        pt.try_into().map_err(|_| KeyError::Malformed)
    }
}

#[async_trait]
impl KeyService for FakeKeyService {
    async fn new_user_key(&self, user: &UserId) -> Result<WrappedKey, KeyError> {
        let dek = self.rng.bytes32();
        let nonce = self.rng.bytes32()[..NONCE_LEN].to_vec();
        let cipher = Aes256Gcm::new_from_slice(&self.kek).map_err(|_| KeyError::Unavailable)?;
        let aad = format!("fake.dek|{}", hex(user.0.as_bytes()));
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &dek,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| KeyError::Unavailable)?;
        let mut out = nonce;
        out.extend_from_slice(&ct);
        Ok(WrappedKey(out))
    }

    async fn seal(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.take_failure(aad.field)?;
        let dek = self.unwrap(wrapped, user)?;
        let cipher = Aes256Gcm::new_from_slice(&dek).map_err(|_| KeyError::Unavailable)?;
        let nonce = self.rng.bytes32()[..NONCE_LEN].to_vec();
        let aad_bytes = aad_bytes(user, aad);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| KeyError::Unavailable)?;
        let mut out = nonce;
        out.extend_from_slice(&ct);
        Ok(out)
    }

    async fn open(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.take_failure(aad.field)?;
        let dek = self.unwrap(wrapped, user)?;
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(KeyError::Malformed);
        }
        let cipher = Aes256Gcm::new_from_slice(&dek).map_err(|_| KeyError::Unavailable)?;
        let (nonce, ct) = ciphertext.split_at(NONCE_LEN);
        let aad_bytes = aad_bytes(user, aad);
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| KeyError::OpenFailed)
    }
}

/// A fake `SystemKeyService` with one fixed key.
pub struct FakeSystemKeyService {
    key: [u8; 32],
    rng: Arc<dyn Rng>,
}

impl FakeSystemKeyService {
    pub fn new(rng: Arc<dyn Rng>) -> Self {
        Self {
            key: rng.bytes32(),
            rng,
        }
    }
}

#[async_trait]
impl SystemKeyService for FakeSystemKeyService {
    async fn seal(&self, aad: &SystemAad, plaintext: &[u8]) -> Result<Vec<u8>, KeyError> {
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| KeyError::Unavailable)?;
        let nonce = self.rng.bytes32()[..NONCE_LEN].to_vec();
        let aad_bytes = system_aad_bytes(aad);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| KeyError::Unavailable)?;
        let mut out = nonce;
        out.extend_from_slice(&ct);
        Ok(out)
    }

    async fn open(&self, aad: &SystemAad, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError> {
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(KeyError::Malformed);
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| KeyError::Unavailable)?;
        let (nonce, ct) = ciphertext.split_at(NONCE_LEN);
        let aad_bytes = system_aad_bytes(aad);
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| KeyError::OpenFailed)
    }
}

fn aad_bytes(user: &UserId, aad: &Aad) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(user.0.as_bytes());
    out.extend_from_slice(
        &u32::try_from(aad.scope.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(aad.scope.as_bytes());
    out.extend_from_slice(
        &u32::try_from(aad.field.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(aad.field.as_bytes());
    out
}

fn system_aad_bytes(aad: &SystemAad) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(
        &u32::try_from(aad.scope.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(aad.scope.as_bytes());
    out.extend_from_slice(
        &u32::try_from(aad.field.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(aad.field.as_bytes());
    out
}
