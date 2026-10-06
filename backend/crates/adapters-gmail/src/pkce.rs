//! PKCE (RFC 7636) helpers: the verifier, its S256 challenge, `state` and
//! `nonce` values (ASVS V10.2.1, V10.5.1).
//!
//! Randomness comes only from the [`Rng`] port (CONVENTIONS), never a
//! thread-local generator.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use obs::Sensitive;
use ports::Rng;
use sha2::{Digest, Sha256};

/// A PKCE verifier and its S256 challenge. Debug prints the challenge only.
#[derive(Debug)]
pub struct Pkce {
    /// The high-entropy verifier; never logged.
    pub verifier: Sensitive<String>,
    /// `base64url(SHA-256(verifier))`, the `code_challenge`.
    pub challenge: String,
}

/// A fresh PKCE pair: 32 random bytes as a 43-character base64url verifier and
/// its S256 challenge.
pub fn new_pkce(rng: &dyn Rng) -> Pkce {
    let verifier = URL_SAFE_NO_PAD.encode(rng.bytes32());
    let challenge = challenge_for(&verifier);
    Pkce {
        verifier: Sensitive::new(verifier),
        challenge,
    }
}

/// A fresh `state` or `nonce`: 32 random bytes as a 43-character base64url
/// string, wrapped so it cannot be logged.
pub fn new_state_or_nonce(rng: &dyn Rng) -> Sensitive<String> {
    Sensitive::new(URL_SAFE_NO_PAD.encode(rng.bytes32()))
}

/// `base64url(SHA-256(ASCII(verifier)))`, the S256 challenge (RFC 7636 4.2).
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use testkit::SeededRng;

    /// RFC 7636 appendix B: the S256 challenge is the SHA-256 of the verifier.
    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pkce_verifier_and_state_are_43_char_base64url() {
        let rng = SeededRng::new(7);
        let pkce = new_pkce(&rng);
        assert_eq!(pkce.verifier.expose().len(), 43);
        assert_eq!(pkce.challenge, challenge_for(pkce.verifier.expose()));
        let state = new_state_or_nonce(&rng);
        assert_eq!(state.expose().len(), 43);
    }
}
