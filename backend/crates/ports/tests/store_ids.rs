//! Coverage + behaviour for the concrete bits of the store mod (ids, versions,
//! cursors, errors). Traits are exercised through the contract suites; the
//! executable helpers here get direct coverage.

use ports::store::{
    Ciphertext, EmailLookupHash, ListKeyHash, Page, PageRequest, Precondition, RateLimitKey,
    SessionHash, Sha256Hash, StoreCursor, StoreError, UserPseudoId, Version, Versioned,
    COLLECTIONS, MAX_PAGE,
};

#[test]
fn max_page_and_collections() {
    assert_eq!(MAX_PAGE, 100);
    assert_eq!(COLLECTIONS.len(), 11);
}

#[test]
fn store_error_display_strings() {
    assert_eq!(StoreError::AlreadyExists.to_string(), "already exists");
    assert_eq!(
        StoreError::PreconditionFailed.to_string(),
        "precondition failed"
    );
    assert_eq!(StoreError::Unavailable.to_string(), "store unavailable");
    assert_eq!(StoreError::Corrupt("x").to_string(), "corrupt record: x");
    assert_eq!(StoreError::Invalid("y").to_string(), "invalid: y");
}

#[test]
fn version_and_precondition_round_trip() {
    let v = Version("v1".into());
    assert_eq!(v, Version("v1".into()));
    assert_eq!(
        Precondition::Matches(v.clone()),
        Precondition::Matches(Version("v1".into()))
    );
    assert_eq!(Precondition::None, Precondition::None);
    let versioned = Versioned {
        record: 7u8,
        version: v,
    };
    assert_eq!(versioned.record, 7u8);
}

#[test]
fn page_request_and_page_and_cursor() {
    let req = PageRequest {
        limit: 20,
        after: Some(StoreCursor("c".into())),
    };
    assert_eq!(req.limit, 20);
    assert!(req.after.is_some());
    let page: Page<u8> = Page {
        items: vec![1, 2],
        next: None,
    };
    assert_eq!(page.items.len(), 2);
    assert!(page.next.is_none());
    assert_eq!(StoreCursor("z".into()), StoreCursor("z".into()));
}

#[test]
fn ciphertext_debug_redacts_bytes_shows_len() {
    let ct = Ciphertext(vec![1, 2, 3, 4, 5]);
    assert_eq!(format!("{ct:?}"), "Ciphertext(5)");
}

#[test]
fn ciphertext_serde_round_trip_and_failure() {
    let ct = Ciphertext(vec![0, 15, 250, 255]);
    let json = serde_json::to_string(&ct).unwrap_or_else(|_| panic!("json"));
    assert_eq!(json, "\"AA_6_w\"");
    let back: Ciphertext = serde_json::from_str(&json).unwrap_or_else(|_| panic!("parse"));
    assert_eq!(back, ct);
    // Invalid base64url fails.
    assert!(serde_json::from_str::<Ciphertext>("\"!!!not base64!!!\"").is_err());
    // Bad type fails.
    assert!(serde_json::from_str::<Ciphertext>("123").is_err());
}

#[test]
fn hash32_serde_round_trip_and_debug_and_failure() {
    let h = Sha256Hash([0xab; 32]);
    let json = serde_json::to_string(&h).unwrap_or_else(|_| panic!("json"));
    assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
    let back: Sha256Hash = serde_json::from_str(&json).unwrap_or_else(|_| panic!("parse"));
    assert_eq!(back, h);
    // Debug prints only the first 8 hex chars.
    assert_eq!(format!("{h:?}"), "abababab");
    // Hex must be 64 chars.
    assert!(serde_json::from_str::<Sha256Hash>("\"abcd\"").is_err());
    // Invalid hex fails.
    assert!(serde_json::from_str::<Sha256Hash>(&format!("\"{}\"", "z".repeat(64))).is_err());
}

#[test]
fn session_hash_to_hex() {
    let h = SessionHash([
        0x12, 0xfe, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ]);
    let hex = h.to_hex();
    assert!(hex.starts_with("12fe2a00"));
    assert_eq!(hex.len(), 64);
}

#[test]
fn other_hash_types_and_id_newtypes() {
    let e = EmailLookupHash([0x01; 32]);
    let l = ListKeyHash([0x02; 32]);
    assert_ne!(e.0, l.0);
    let _ = serde_json::to_string(&e).unwrap_or_else(|_| panic!("json"));

    let uid = UserPseudoId("aabb".into());
    let rk = RateLimitKey("k".into());
    assert_eq!(format!("{uid:?}"), "UserPseudoId(\"aabb\")");
    assert_eq!(format!("{rk:?}"), "RateLimitKey(\"k\")");
}
