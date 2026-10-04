//! The `Rng` port: the only source of randomness in the system.

use uuid::Uuid;

/// The only source of randomness. Real implementation in `os_rng.rs`.
pub trait Rng: Send + Sync {
    /// CSPRNG in production.
    fn bytes32(&self) -> [u8; 32];
    fn uuid_v4(&self) -> Uuid;
}
