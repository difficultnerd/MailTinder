//! A seeded random source for deterministic tests.

use std::sync::atomic::{AtomicU64, Ordering};

use ports::Rng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A deterministic `Rng`: `bytes32` is SHA-256 of `seed_le || counter_le`.
pub struct SeededRng {
    seed: u64,
    counter: AtomicU64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: AtomicU64::new(0),
        }
    }
}

impl Rng for SeededRng {
    fn bytes32(&self) -> [u8; 32] {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut hasher = Sha256::new();
        hasher.update(self.seed.to_le_bytes());
        hasher.update(counter.to_le_bytes());
        hasher.finalize().into()
    }

    fn uuid_v4(&self) -> Uuid {
        let bytes = self.bytes32();
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&bytes[..16]);
        buf[6] = (buf[6] & 0x0f) | 0x40;
        buf[8] = (buf[8] & 0x3f) | 0x80;
        Uuid::from_bytes(buf)
    }
}
