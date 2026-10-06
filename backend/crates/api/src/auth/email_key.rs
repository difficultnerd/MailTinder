//! The keyed email lookup hash: `EmailLookupHash` for invites and requests.

use domain::EmailAddress;
use hmac::{Hmac, Mac};
use obs::Sensitive;
use ports::EmailLookupHash;
use sha2::Sha256;

/// HMAC-SHA-256 under `ApiConfig::email_lookup_key` of the address's lower-case
/// `lookup_form()` (T-404). Only the hash is stored or compared, so an invite
/// row never holds a readable address.
#[must_use]
pub fn email_lookup_hash(key: &Sensitive<Vec<u8>>, email: &EmailAddress) -> EmailLookupHash {
    match Hmac::<Sha256>::new_from_slice(key.expose()) {
        Ok(mut mac) => {
            mac.update(email.lookup_form().as_bytes());
            let digest = mac.finalize().into_bytes();
            let mut out = [0u8; 32];
            out.copy_from_slice(&digest);
            EmailLookupHash(out)
        }
        // HMAC accepts every key length, so this is unreachable; a zeroed hash
        // simply matches nothing rather than panicking in a request path.
        Err(_) => EmailLookupHash([0u8; 32]),
    }
}
