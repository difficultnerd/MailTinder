//! The list key hash stored on an unsubscribe job (T-605).
//!
//! `[DEFAULT]` S5 says "list key HMAC" without naming the key, so this uses the
//! email lookup HMAC key (`SecretName::EmailLookupHmacKey`, Secret Manager,
//! `api` only). The runner (T-701) batches jobs by this hash; it never holds a
//! readable sender or list ID.

use domain::SenderKey;
use hmac::{Hmac, Mac};
use obs::Sensitive;
use ports::ListKeyHash;
use sha2::Sha256;

/// The HMAC secret behind [`list_key_hash`]; the driver builds it from
/// `SecretName::EmailLookupHmacKey`. Debug prints `[redacted]`.
#[derive(Clone, Debug)]
pub struct HmacKey(pub Sensitive<Vec<u8>>);

/// HMAC-SHA-256 under `key` of `"list:" + sender_key + "\n" + list_id_or_empty`.
#[must_use]
pub fn list_key_hash(key: &HmacKey, sender: &SenderKey, list_id: Option<&str>) -> ListKeyHash {
    match Hmac::<Sha256>::new_from_slice(key.0.expose()) {
        Ok(mut mac) => {
            mac.update(b"list:");
            mac.update(sender.as_str().as_bytes());
            mac.update(b"\n");
            mac.update(list_id.unwrap_or_default().as_bytes());
            let digest = mac.finalize().into_bytes();
            let mut out = [0u8; 32];
            out.copy_from_slice(&digest);
            ListKeyHash(out)
        }
        // HMAC accepts every key length, so this is unreachable; a zeroed hash
        // simply matches nothing rather than panicking in a request path.
        Err(_) => ListKeyHash([0u8; 32]),
    }
}
