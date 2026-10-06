//! The `__session` cookie: naming, attributes and the raw session ID.
//!
//! The cookie name is fixed by Firebase Hosting, which strips every other
//! cookie before forwarding to Cloud Run (S7 3.2). The protections that would
//! otherwise come from the `__Host-` prefix are enforced by attribute instead.
//! The raw ID leaves this process only in one `Set-Cookie` header: it is never
//! stored, never in a body or URL, and never logged.

use std::fmt;

use axum::http::{header, HeaderMap, HeaderValue};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use obs::Sensitive;
use ports::{Rng, SessionHash};
use sha2::{Digest, Sha256};

/// The one cookie the browser and the server share.
pub const SESSION_COOKIE: &str = "__session";

/// Length of a base64url-encoded 32-byte value, with no padding.
const RAW_LEN: usize = 43;

/// The exact attribute string for a live session cookie.
const SET_ATTRIBUTES: &str = "Path=/; Secure; HttpOnly; SameSite=Lax";
/// The exact attribute string for clearing the cookie.
const CLEAR_COOKIE: &str = "__session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0";

/// A 32-byte random session ID, base64url without padding (43 characters).
/// Never stored, never logged; only its SHA-256 hash reaches the store.
#[derive(Clone)]
pub struct RawSessionId(Sensitive<String>);

impl RawSessionId {
    /// Generate a fresh ID from the CSPRNG port (`Rng::bytes32`), 256 bits.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self(Sensitive::new(URL_SAFE_NO_PAD.encode(rng.bytes32())))
    }

    /// Parse a client-supplied cookie value, accepting only the exact shape the
    /// server issues (43 characters of `[A-Za-z0-9_-]`). Any other shape finds
    /// no record: the server never adopts a client-chosen ID (fixation).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let ok = value.len() == RAW_LEN
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then(|| Self(Sensitive::new(value.to_owned())))
    }

    /// SHA-256 of the 43-character string's bytes; the store document key.
    #[must_use]
    pub fn hash(&self) -> SessionHash {
        let digest = Sha256::digest(self.0.expose().as_bytes());
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        SessionHash(out)
    }

    /// The raw value. Only `set_cookie` may call this.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl fmt::Debug for RawSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RawSessionId([redacted])")
    }
}

/// The `Set-Cookie` value for a live session: `__session=<v>; Path=/; Secure;
/// HttpOnly; SameSite=Lax`. No `Domain`, no `Max-Age` or `Expires`.
#[must_use]
pub fn set_cookie(raw: &RawSessionId) -> HeaderValue {
    let value = format!("{SESSION_COOKIE}={}; {SET_ATTRIBUTES}", raw.expose());
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("__session="))
}

/// The `Set-Cookie` value that clears the cookie.
#[must_use]
pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static(CLEAR_COOKIE)
}

/// The first `__session` pair across every `Cookie` header, or `None`.
/// A value that is not the exact issued shape is refused.
#[must_use]
pub fn read_cookie(headers: &HeaderMap) -> Option<RawSessionId> {
    for header in headers.get_all(header::COOKIE) {
        let Ok(raw) = header.to_str() else {
            continue;
        };
        for pair in raw.split(';') {
            let pair = pair.trim();
            if let Some((name, value)) = pair.split_once('=') {
                if name.trim() == SESSION_COOKIE {
                    return RawSessionId::parse(value.trim());
                }
            }
        }
    }
    None
}
