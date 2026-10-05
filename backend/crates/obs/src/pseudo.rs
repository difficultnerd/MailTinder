//! Pseudonymous user IDs for logs and metrics.
//!
//! A user's real UUID never reaches a log line; only an HMAC pseudonym does.
//! The pseudonym is the first 16 bytes of HMAC-SHA-256 under the log
//! pseudonymisation key, with a domain-separation label so it cannot be
//! confused with any other HMAC use.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::Sensitive;

type HmacSha256 = Hmac<Sha256>;

/// A 32-char lower-case hex pseudonym for a user. Safe to log (C1).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PseudoId(String);

impl PseudoId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PseudoId {
    // The value is C1 (a pseudonym), so printing it is fine.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for PseudoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Derives `PseudoId`s from user UUIDs under a secret key.
pub struct Pseudonymiser {
    key: Sensitive<Vec<u8>>,
}

impl Pseudonymiser {
    #[must_use]
    pub fn new(key: Sensitive<Vec<u8>>) -> Self {
        Self { key }
    }

    /// First 16 bytes of HMAC-SHA-256(key, "mt.log.user.v1" || user UUID bytes),
    /// lower-case hex. Stable for one key, different for another key.
    ///
    /// # Panics
    ///
    /// Panics if the HMAC key is empty (HMAC accepts any key length, so this
    /// cannot happen in practice).
    #[must_use]
    pub fn pseudo_id(&self, user: &uuid::Uuid) -> PseudoId {
        let mut mac = HmacSha256::new_from_slice(self.key.expose())
            .unwrap_or_else(|_| unreachable!("hmac accepts any key length"));
        mac.update(b"mt.log.user.v1");
        mac.update(user.as_bytes());
        let digest = mac.finalize().into_bytes();
        PseudoId(hex::encode(&digest[..16]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_id_is_32_hex_and_stable_per_key() {
        let key1 = Sensitive::new(vec![1u8; 32]);
        let key2 = Sensitive::new(vec![2u8; 32]);
        let p1 = Pseudonymiser::new(key1);
        let p2 = Pseudonymiser::new(key2);
        let user = uuid::Uuid::new_v4();
        let a = p1.pseudo_id(&user);
        let b = p1.pseudo_id(&user);
        let c = p2.pseudo_id(&user);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.as_str().len(), 32);
        assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}
