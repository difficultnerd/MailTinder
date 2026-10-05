//! Envelope encryption: AES-256-GCM under a per-user `data_key` wrapped by
//! Cloud KMS (S6 section 5, T-302).
//!
//! Nothing here stores plaintext keys anywhere but process memory. All key
//! bytes live in `zeroize::Zeroizing` buffers and are dropped (zeroised) as
//! soon as they are no longer needed.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use domain::UserId;
use ports::{Aad, KeyError, SystemAad};

pub mod envelope;
pub mod kms;
pub mod system;

/// Scheme version 1: AES-256-GCM, 96-bit random nonce, 128-bit tag.
pub const SCHEME_V1: u8 = 0x01;
/// AES-GCM nonce length in bytes.
pub const NONCE_LEN: usize = 12;
/// AES-GCM tag length in bytes.
pub const TAG_LEN: usize = 16;
/// Largest plaintext accepted by `aead_seal` (covers the 5 MiB app folder
/// file, T-405).
pub const MAX_PLAINTEXT: usize = 8 * 1024 * 1024;
/// AAD label for values sealed under a user's `data_key`.
pub const AEAD_AAD_LABEL: &[u8] = b"mailtinder.aead.v1\0";
/// AAD label for the KMS wrap of a user's `data_key`.
pub const DEK_WRAP_AAD_LABEL: &[u8] = b"mailtinder.dek.v1\0";
/// AAD label for values sealed under the system KMS key.
pub const SYSTEM_AAD_LABEL: &[u8] = b"mailtinder.sys.v1\0";

/// `label || user UUID (16 raw bytes) || u32 BE len(scope) || scope UTF-8 ||
/// u32 BE len(field) || field UTF-8`.
///
/// Length-prefixed fields (never plain concatenation) so that
/// `scope = "ab", field = "c"` can never equal `scope = "a", field = "bc"`.
/// The label separates the three AAD kinds so a value from one kind can never
/// authenticate as another.
pub fn encode_aad(aad: &Aad) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(AEAD_AAD_LABEL.len() + 16 + 8 + aad.scope.len() + aad.field.len());
    out.extend_from_slice(AEAD_AAD_LABEL);
    out.extend_from_slice(aad.user.0.as_bytes());
    push_len_prefixed(&mut out, aad.scope.as_bytes());
    push_len_prefixed(&mut out, aad.field.as_bytes());
    out
}

/// `label || u32 BE len(scope) || scope || u32 BE len(field) || field`.
pub fn encode_system_aad(aad: &SystemAad) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(SYSTEM_AAD_LABEL.len() + 8 + aad.scope.len() + aad.field.len());
    out.extend_from_slice(SYSTEM_AAD_LABEL);
    push_len_prefixed(&mut out, aad.scope.as_bytes());
    push_len_prefixed(&mut out, aad.field.as_bytes());
    out
}

/// `DEK_WRAP_AAD_LABEL || user UUID (16 raw bytes)`.
///
/// KMS associated data binds the wrapped key to its user: a wrapped key
/// copied into another user's record fails to unwrap.
pub fn dek_wrap_aad(user: &UserId) -> Vec<u8> {
    let mut out = Vec::with_capacity(DEK_WRAP_AAD_LABEL.len() + 16);
    out.extend_from_slice(DEK_WRAP_AAD_LABEL);
    out.extend_from_slice(user.0.as_bytes());
    out
}

/// Seals `plaintext` under `key` with `aad`, returning
/// `SCHEME_V1 || nonce (12) || ciphertext || tag (16)`.
///
/// # Errors
///
/// Returns `KeyError::Malformed` when `plaintext` exceeds `MAX_PLAINTEXT`.
pub fn aead_seal(
    key: &[u8; 32],
    nonce: [u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, KeyError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(KeyError::Malformed);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| KeyError::Unavailable)?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| KeyError::Unavailable)?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
    out.push(SCHEME_V1);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens a value sealed by `aead_seal`.
///
/// # Errors
///
/// Returns `KeyError::Malformed` for a too-short or wrong-version value, and
/// `KeyError::OpenFailed` for any AEAD failure (wrong key, wrong AAD or
/// tampering — one error on purpose, so `OpenFailed` gives no oracle).
pub fn aead_open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, KeyError> {
    if sealed.len() < 1 + NONCE_LEN + TAG_LEN {
        return Err(KeyError::Malformed);
    }
    if sealed[0] != SCHEME_V1 {
        return Err(KeyError::UnsupportedVersion(sealed[0]));
    }
    let nonce = &sealed[1..=NONCE_LEN];
    let ct = &sealed[1 + NONCE_LEN..];
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| KeyError::Unavailable)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| KeyError::OpenFailed)
}

fn push_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}
