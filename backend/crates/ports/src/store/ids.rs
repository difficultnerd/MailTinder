//! ID and hash newtypes for the server store.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

macro_rules! uuid_id {
    ($n:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $n(pub Uuid);
    };
}

uuid_id!(InviteId);
uuid_id!(InviteRequestId);
uuid_id!(NeedsAttentionId);
uuid_id!(SnapshotId);
uuid_id!(EvalId);
uuid_id!(SessionRecordId);

/// Bytes produced by `KeyService::seal` or `SystemKeyService::seal`.
/// Serialises as unpadded base64url. Debug prints `Ciphertext(len=N)`.
#[derive(Clone, PartialEq, Eq)]
pub struct Ciphertext(pub Vec<u8>);

impl fmt::Debug for Ciphertext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Ciphertext").field(&self.0.len()).finish()
    }
}

impl Serialize for Ciphertext {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Ciphertext {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)?;
        Ok(Self(bytes))
    }
}

macro_rules! hash32 {
    ($n:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $n(pub [u8; 32]);

        impl fmt::Debug for $n {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hex = hex(&self.0);
                f.write_str(&hex[..8])
            }
        }

        impl Serialize for $n {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&hex(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $n {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                let bytes = unhex(&s).map_err(serde::de::Error::custom)?;
                Ok(Self(bytes))
            }
        }
    };
}

hash32!(Sha256Hash, "32-byte digests. Serialise as lower-case hex (64 chars). Debug prints the first 8 hex chars only.");
hash32!(EmailLookupHash, "HMAC-SHA-256, email lookup key.");
hash32!(ListKeyHash, "HMAC of the list key (T-605 decides the key).");
hash32!(SessionHash, "SHA-256 of the raw session ID.");

impl SessionHash {
    /// The sessions document ID.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

/// HMAC-SHA-256 of the user ID under the log pseudonymisation key, first 16
/// bytes, lower-case hex (32 chars).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserPseudoId(pub String);

/// Opaque per-limit key built by the caller, already hashed (no raw IP or
/// address). Max 200 chars, `[a-z0-9:_-]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RateLimitKey(pub String);

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn unhex(s: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", s.len()));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_val(chunk[0]).ok_or("invalid hex")?;
        let lo = hex_val(chunk[1]).ok_or("invalid hex")?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}
