//! The production `KeyService`: each user's `data_key` is wrapped by Cloud KMS
//! and cached (unwrapped) in memory for a short TTL.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::UserId;
use ports::{Aad, Clock, KeyError, KeyService, Rng, WrappedKey};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::kms::KmsApi;
use super::{aead_open, aead_seal, dek_wrap_aad, encode_aad, SCHEME_V1};

/// How long an unwrapped `data_key` stays in the cache before a re-unwrap.
pub const KEY_CACHE_TTL: time::Duration = time::Duration::minutes(5);
/// Maximum number of cached unwrapped keys. When full, the cache is cleared
/// (simple and safe) before inserting.
pub const KEY_CACHE_MAX: usize = 1024;

/// The production `KeyService`, wrapping each user's `data_key` with Cloud KMS.
pub struct EnvelopeKeyService {
    kms: Arc<dyn KmsApi>,
    rng: Arc<dyn Rng>,
    clock: Arc<dyn Clock>,
    cache: Mutex<KeyCache>,
}

/// `(user, hash of wrapped key)` -> `(unwrapped DEK, cache time)`.
type KeyCache = HashMap<(UserId, [u8; 32]), KeyCacheValue>;
/// An unwrapped DEK plus when it entered the cache.
type KeyCacheValue = (zeroize::Zeroizing<[u8; 32]>, OffsetDateTime);

impl EnvelopeKeyService {
    /// Builds the service with the given KMS, randomness and clock.
    pub fn new(kms: Arc<dyn KmsApi>, rng: Arc<dyn Rng>, clock: Arc<dyn Clock>) -> Self {
        Self {
            kms,
            rng,
            clock,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Drop any cached unwrapped key for this user (account deletion, T-803).
    ///
    /// // SHRED: crypto-shredding is only complete because no backup holds the
    /// // deleted `users` document with its wrapped key (the KEK is shared).
    /// // The trial keeps no Firestore backups, point-in-time recovery or
    /// // exports (James, 4 October 2026); enabling any of them breaks DEL-2.
    pub fn evict(&self, user: &UserId) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|_| panic!("cache poisoned"));
        cache.retain(|(u, _), _| u != user);
    }

    /// Unwrap `wrapped` for `user`, using the cache when fresh.
    async fn unwrap(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
    ) -> Result<zeroize::Zeroizing<[u8; 32]>, KeyError> {
        let hash: [u8; 32] = Sha256::digest(&wrapped.0).into();
        let now = self.clock.now();
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(|_| panic!("cache poisoned"));
            if let Some((key, at)) = cache.get(&(*user, hash)) {
                if now - *at < KEY_CACHE_TTL {
                    return Ok(key.clone());
                }
            }
        }
        // Never hold the cache lock across the KMS await.
        if wrapped.0.is_empty() || wrapped.0[0] != SCHEME_V1 {
            return Err(KeyError::UnsupportedVersion(
                wrapped.0.first().copied().unwrap_or(0),
            ));
        }
        let dek = self
            .kms
            .decrypt(&wrapped.0[1..], &dek_wrap_aad(user))
            .await?;
        if dek.len() != 32 {
            return Err(KeyError::Malformed);
        }
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&dek);
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|_| panic!("cache poisoned"));
        if cache.len() >= KEY_CACHE_MAX {
            cache.clear();
        }
        cache.insert((*user, hash), (key.clone(), now));
        Ok(key)
    }
}

#[async_trait]
impl KeyService for EnvelopeKeyService {
    async fn new_user_key(&self, user: &UserId) -> Result<WrappedKey, KeyError> {
        let dek = zeroize::Zeroizing::new(self.rng.bytes32());
        let kms_ct = self.kms.encrypt(&*dek, &dek_wrap_aad(user)).await?;
        // The plaintext `dek` is dropped (zeroised) here; it is not cached,
        // because the caller has not stored the wrapped key yet.
        let mut out = Vec::with_capacity(1 + kms_ct.len());
        out.push(SCHEME_V1);
        out.extend_from_slice(&kms_ct);
        Ok(WrappedKey(out))
    }

    async fn seal(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        if aad.user != *user {
            return Err(KeyError::OpenFailed);
        }
        let key = self.unwrap(user, wrapped).await?;
        let nonce: [u8; 12] = self.rng.bytes32()[..12].try_into().expect("12 bytes");
        aead_seal(&key, nonce, &encode_aad(aad), plaintext)
    }

    async fn open(
        &self,
        user: &UserId,
        wrapped: &WrappedKey,
        aad: &Aad,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        if aad.user != *user {
            return Err(KeyError::OpenFailed);
        }
        let key = self.unwrap(user, wrapped).await?;
        aead_open(&key, &encode_aad(aad), ciphertext)
    }
}
