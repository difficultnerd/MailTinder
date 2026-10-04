//! Exercise the real `OsRng` and `SystemClock` (T-202b coverage for adapters-gcp).

use adapters_gcp::{OsRng, SystemClock};
use ports::{Clock, Rng};

#[test]
fn os_rng_bytes32_is_present_and_differs_between_calls() {
    let rng = OsRng;
    let a = rng.bytes32();
    let b = rng.bytes32();
    assert_eq!(a.len(), 32);
    // Two draws must differ (astronomically likely not to collide).
    assert_ne!(a, b);
}

#[test]
fn os_rng_uuid_v4_is_valid() {
    let rng = OsRng;
    let u = rng.uuid_v4();
    assert_eq!(u.get_version(), Some(uuid::Version::Random));
    assert_eq!(u.get_variant(), uuid::Variant::RFC4122);
    // Two draws differ.
    assert_ne!(u, rng.uuid_v4());
}

#[test]
fn system_clock_now_tracks_wall_clock() {
    let before = time::OffsetDateTime::now_utc();
    let now = SystemClock.now();
    let after = time::OffsetDateTime::now_utc();
    assert!(now >= before && now <= after);
}
