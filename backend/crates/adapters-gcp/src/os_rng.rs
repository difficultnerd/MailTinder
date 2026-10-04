//! The real `Rng`: the only place allowed to read the OS random source.

use ports::Rng;
use uuid::Uuid;

/// The real random source. The only OS random call in the codebase (T-003
/// Semgrep rule allows this file only).
pub struct OsRng;

impl Rng for OsRng {
    fn bytes32(&self) -> [u8; 32] {
        let mut buf = [0u8; 32];
        if let Err(_e) = getrandom::getrandom(&mut buf) {
            // A CSPRNG failure is unrecoverable; never return weak bytes. This
            // is the one place a hard stop is right (T-202b).
            std::process::abort();
        }
        buf
    }

    fn uuid_v4(&self) -> Uuid {
        let mut buf = [0u8; 16];
        if let Err(_e) = getrandom::getrandom(&mut buf) {
            // See `bytes32`; a hard stop is the only safe response.
            std::process::abort();
        }
        Uuid::from_bytes(buf)
    }
}
