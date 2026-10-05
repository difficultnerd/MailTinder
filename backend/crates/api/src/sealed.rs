//! Sealed tokens: the one scheme for every opaque token the server hands the
//! browser (`cursor`, `undo`, `classification`, `prompt_ref`).
//!
//! A token is AES-256-GCM under the user's KMS-wrapped `data_key` through
//! `KeyService`, with the token type, user ID, session record ID and expiry in
//! the associated data, and type and expiry also in clear. A token of one type
//! can never be accepted as another, for another user or session, or after its
//! expiry (S6 5, S7 2.1, ASVS V9.1, V9.2).
//!
//! Wire format: `mt1.<type>.<exp>.<body>` where `<type>` is the wire name,
//! `<exp>` is the expiry as decimal Unix seconds (1 to 12 digits, no sign) and
//! `<body>` is unpadded base64url of the `KeyService::seal` output
//! (`0x01 || nonce || ciphertext || tag`, T-302).
//!
//! Nothing from this module is ever logged: no tokens, payloads, user IDs or
//! session record IDs (S5, S6 5).

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::UserId;
use ports::store::SessionRecordId;
use ports::{Aad, Clock, KeyError, KeyService, WrappedKey};
use serde::de::DeserializeOwned;
use serde::Serialize;
use time::OffsetDateTime;

/// Scheme version 1 (`mt1`). The algorithm is fixed by this prefix; a token
/// can never select another algorithm (ASVS V9.1.2).
pub const TOKEN_PREFIX: &str = "mt1";
/// Maximum token length, checked before any decoding (S7 2.1).
pub const MAX_TOKEN_LEN: usize = 8192;
/// Maximum token lifetime: the S7 3.2 absolute session lifetime. Callers pass
/// `min(now + 12 h, session absolute expiry)`.
pub const MAX_TOKEN_TTL: time::Duration = time::Duration::hours(12);

/// The kind of opaque token. The type is bound into the associated data, so a
/// token of one type can never be accepted as another (ASVS V9.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenType {
    Cursor,
    Undo,
    Classification,
    PromptRef,
}

impl TokenType {
    /// All token types, in a stable order.
    pub const ALL: [TokenType; 4] = [
        TokenType::Cursor,
        TokenType::Undo,
        TokenType::Classification,
        TokenType::PromptRef,
    ];

    /// The clear wire name carried in the token.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            TokenType::Cursor => "cursor",
            TokenType::Undo => "undo",
            TokenType::Classification => "classification",
            TokenType::PromptRef => "prompt_ref",
        }
    }

    /// The `Aad::field` for this token type (S6 5).
    #[must_use]
    pub fn aad_field(self) -> &'static str {
        match self {
            TokenType::Cursor => "sealed.cursor",
            TokenType::Undo => "sealed.undo",
            TokenType::Classification => "sealed.classification",
            TokenType::PromptRef => "sealed.prompt_ref",
        }
    }
}

/// A sealed-token error. The variants deliberately do not say which check
/// failed beyond the coarse categories; routes collapse them to one status
/// anyway (S7 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")]
    Malformed,
    #[error("wrong token type")]
    WrongType,
    #[error("token expired")]
    Expired,
    #[error("token invalid")]
    Invalid,
    #[error("key service unavailable")]
    Unavailable,
    #[error("ttl too long")]
    TtlTooLong,
}

/// Seals and opens opaque tokens under a user's KMS-wrapped `data_key`.
pub struct SealedTokens {
    keys: Arc<dyn KeyService>,
    clock: Arc<dyn Clock>,
}

impl SealedTokens {
    /// Builds the token service. The key comes only from `KeyService` with the
    /// user's KMS-wrapped `data_key`; there is no constructor that takes raw
    /// key bytes (ASVS V9.1.3).
    pub fn new(keys: Arc<dyn KeyService>, clock: Arc<dyn Clock>) -> Self {
        Self { keys, clock }
    }

    /// Seals `payload` as a token of type `ty` for `user` and `session`,
    /// expiring at `expires_at`.
    ///
    /// # Errors
    ///
    /// `TtlTooLong` when `expires_at` is not in the future or is more than
    /// `MAX_TOKEN_TTL` away; `Unavailable` when the key service is down;
    /// `Invalid` when the payload cannot be serialised or the key service
    /// rejects the seal; `Malformed` when the resulting token is oversized.
    pub async fn seal<T: Serialize>(
        &self,
        ty: TokenType,
        user: &UserId,
        wrapped: &WrappedKey,
        session: &SessionRecordId,
        expires_at: OffsetDateTime,
        payload: &T,
    ) -> Result<String, TokenError> {
        let now = self.clock.now();
        if expires_at <= now || expires_at - now > MAX_TOKEN_TTL {
            return Err(TokenError::TtlTooLong);
        }
        let exp = expires_at.unix_timestamp();
        let plaintext = serde_json::to_vec(payload).map_err(|_| TokenError::Invalid)?;
        let aad = aad(ty, user, session, exp);
        let sealed = self
            .keys
            .seal(user, wrapped, &aad, &plaintext)
            .await
            .map_err(|e| map_key_error(&e))?;
        let body = URL_SAFE_NO_PAD.encode(&sealed);
        let token = format!("{TOKEN_PREFIX}.{}.{exp}.{body}", ty.wire());
        if token.len() > MAX_TOKEN_LEN {
            return Err(TokenError::Malformed);
        }
        Ok(token)
    }

    /// Opens `token`, expecting type `expected` for `user` and `session`.
    ///
    /// # Errors
    ///
    /// `Malformed` for a token that is oversized, non-ASCII, wrongly shaped,
    /// has an unknown prefix or type, a bad expiry, or an undecodable body;
    /// `WrongType` when the clear type is a different known type;
    /// `Expired` when `now >= exp`; `Unavailable` when the key service is
    /// down; `Invalid` when the AEAD open or payload deserialisation fails.
    pub async fn open<T: DeserializeOwned>(
        &self,
        expected: TokenType,
        user: &UserId,
        wrapped: &WrappedKey,
        session: &SessionRecordId,
        token: &str,
    ) -> Result<T, TokenError> {
        if token.len() > MAX_TOKEN_LEN || !token.is_ascii() {
            return Err(TokenError::Malformed);
        }
        let parts: Vec<&str> = token.splitn(4, '.').collect();
        if parts.len() != 4 || parts[0] != TOKEN_PREFIX {
            return Err(TokenError::Malformed);
        }
        // Part 1: the clear type. If it is not a known wire name it is
        // malformed; if it is a different known type, give the clearer error.
        let claimed = wire_to_type(parts[1]).ok_or(TokenError::Malformed)?;
        if claimed != expected {
            return Err(TokenError::WrongType);
        }
        // Part 2: the clear expiry, digits only, 1 to 12 chars.
        let exp = parse_exp(parts[2])?;
        if self.clock.now().unix_timestamp() >= exp {
            return Err(TokenError::Expired);
        }
        // Part 3: unpadded base64url body.
        let body = URL_SAFE_NO_PAD
            .decode(parts[3].as_bytes())
            .map_err(|_| TokenError::Malformed)?;
        // Rebuild the AAD from the type the route expects, never from the type
        // the token claims (S6 5). The expiry is inside the AAD, so editing the
        // clear expiry fails authentication.
        let aad = aad(expected, user, session, exp);
        let plaintext = self
            .keys
            .open(user, wrapped, &aad, &body)
            .await
            .map_err(|e| map_key_error(&e))?;
        serde_json::from_slice(&plaintext).map_err(|_| TokenError::Invalid)
    }
}

/// Route helper: every failure is `400 invalid_request`, except an undo route,
/// where every failure is `410 undo_expired` (S7 2.1, S7 4).
#[must_use]
pub fn http_status_for(expected: TokenType, err: TokenError) -> (u16, &'static str) {
    match err {
        TokenError::Unavailable => (503, "provider_unavailable"),
        _ if expected == TokenType::Undo => (410, "undo_expired"),
        _ => (400, "invalid_request"),
    }
}

/// The associated data for a token: type, user ID, session record ID and
/// expiry, encoded as `label || user || len scope || scope || len field ||
/// field` by T-302's `encode_aad`. The scope string is `session|exp`.
fn aad(ty: TokenType, user: &UserId, session: &SessionRecordId, exp: i64) -> Aad {
    Aad {
        user: *user,
        scope: format!("{}|{exp}", session.0.hyphenated()),
        field: ty.aad_field(),
    }
}

/// Maps a `KeyError` to a `TokenError`. `Unavailable` stays `Unavailable`;
/// every other key error is `Invalid` (the AEAD open failed: other user,
/// session, type, expiry, or tampering).
fn map_key_error(e: &KeyError) -> TokenError {
    match e {
        KeyError::Unavailable => TokenError::Unavailable,
        _ => TokenError::Invalid,
    }
}

/// Parses the clear expiry: digits only, 1 to 12 chars, fits in `i64`.
fn parse_exp(s: &str) -> Result<i64, TokenError> {
    if s.is_empty() || s.len() > 12 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TokenError::Malformed);
    }
    s.parse::<i64>().map_err(|_| TokenError::Malformed)
}

/// Maps a clear wire name to a `TokenType`, or `None` if it is not one of the
/// four known names.
fn wire_to_type(wire: &str) -> Option<TokenType> {
    TokenType::ALL.into_iter().find(|t| t.wire() == wire)
}
