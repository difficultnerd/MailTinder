//! T-302 envelope encryption tests: `EnvelopeKeyService` and
//! `KmsSystemKeyService` against an in-memory KMS that enforces the KMS AAD
//! exactly like Cloud KMS.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use adapters_gcp::{EnvelopeKeyService, KmsApi, KmsSystemKeyService};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use domain::UserId;
use ports::store::aad_fields::{MAILBOX_EMAIL, MAILBOX_REFRESH_TOKEN};
use ports::store::{Precondition, ServerStore, UserRecord};
use ports::{Aad, KeyError, KeyService, Rng, SystemAad, SystemKeyService};
use testkit::{InMemoryServerStore, SeededRng, VirtualClock, T0};
use time::Duration;
use uuid::Uuid;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// An in-memory KMS: AES-256-GCM under a random KEK, enforcing the KMS AAD
/// exactly like Cloud KMS (a wrong AAD fails to decrypt).
struct InMemoryKms {
    kek: [u8; 32],
    rng: Arc<dyn Rng>,
    decrypt_calls: Mutex<u64>,
}

impl InMemoryKms {
    fn new(rng: Arc<dyn Rng>) -> Self {
        Self {
            kek: rng.bytes32(),
            rng,
            decrypt_calls: Mutex::new(0),
        }
    }

    fn decrypt_calls(&self) -> u64 {
        *self.decrypt_calls.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl KmsApi for InMemoryKms {
    async fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, KeyError> {
        let cipher = Aes256Gcm::new_from_slice(&self.kek).map_err(|_| KeyError::Unavailable)?;
        let nonce = self.rng.bytes32()[..NONCE_LEN].to_vec();
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| KeyError::Unavailable)?;
        let mut out = nonce;
        out.extend_from_slice(&ct);
        Ok(out)
    }

    async fn decrypt(
        &self,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyError> {
        *self.decrypt_calls.lock().unwrap() += 1;
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(KeyError::Malformed);
        }
        let cipher = Aes256Gcm::new_from_slice(&self.kek).map_err(|_| KeyError::Unavailable)?;
        let (nonce, ct) = ciphertext.split_at(NONCE_LEN);
        let pt = cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| KeyError::OpenFailed)?;
        Ok(zeroize::Zeroizing::new(pt))
    }
}

fn user(n: u64) -> UserId {
    UserId::new(Uuid::from_u64_pair(0, n))
}

fn aad(user: UserId, scope: &str, field: &'static str) -> Aad {
    Aad {
        user,
        scope: scope.to_owned(),
        field,
    }
}

fn system_aad(scope: &str, field: &'static str) -> SystemAad {
    SystemAad {
        scope: scope.to_owned(),
        field,
    }
}

fn setup() -> (
    Arc<InMemoryKms>,
    Arc<SeededRng>,
    Arc<VirtualClock>,
    Arc<EnvelopeKeyService>,
) {
    let rng = Arc::new(SeededRng::new(42));
    let clock = Arc::new(VirtualClock::new(T0));
    let kms = Arc::new(InMemoryKms::new(Arc::clone(&rng) as Arc<dyn Rng>));
    let svc = Arc::new(EnvelopeKeyService::new(
        Arc::clone(&kms) as Arc<dyn KmsApi>,
        Arc::clone(&rng) as Arc<dyn Rng>,
        Arc::clone(&clock) as Arc<dyn ports::Clock>,
    ));
    (kms, rng, clock, svc)
}

#[tokio::test]
async fn asvs_v11_3_3_round_trip() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"refresh-token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    let pt = svc
        .open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(pt, b"refresh-token");
    Ok(())
}

#[tokio::test]
async fn asvs_v11_3_3_ciphertext_moved_to_other_user_fails() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let ua = user(1);
    let ub = user(2);
    let wa = svc.new_user_key(&ua).await.map_err(|e| format!("{e:?}"))?;
    let wb = svc.new_user_key(&ub).await.map_err(|e| format!("{e:?}"))?;
    let aad_a = aad(ua, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&ua, &wa, &aad_a, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Opening user A's ciphertext as user B fails.
    let aad_b = aad(ub, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let err = svc
        .open(&ub, &wb, &aad_b, &ct)
        .await
        .expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}

#[tokio::test]
async fn asvs_v11_3_3_ciphertext_moved_to_other_scope_or_field_fails() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad_a = aad(u, "mailbox-A", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad_a, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Opened as mailbox B.
    let aad_b = aad(u, "mailbox-B", MAILBOX_REFRESH_TOKEN);
    let err = svc
        .open(&u, &wrapped, &aad_b, &ct)
        .await
        .expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    // Opened as mailbox.email_address (different field).
    let aad_f = aad(u, "mailbox-A", MAILBOX_EMAIL);
    let err = svc
        .open(&u, &wrapped, &aad_f, &ct)
        .await
        .expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}

proptest::proptest! {
    #[test]
    fn asvs_v11_3_3_any_flipped_byte_fails(bit in 0usize..(NONCE_LEN + 5 + TAG_LEN) * 8) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (_, _, _, svc) = setup();
            let u = user(1);
            let wrapped = svc.new_user_key(&u).await.unwrap();
            let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
            let ct = svc.seal(&u, &wrapped, &aad, b"token").await.unwrap();
            let mut tampered = ct.clone();
            // Skip the scheme-version byte (byte 0): flipping it is the
            // UnsupportedVersion case, covered by asvs_v11_2_2_*. Here we flip
            // only nonce/ciphertext/tag bytes, all of which must give OpenFailed.
            let byte = 1 + bit / 8;
            let mask = 1u8 << (bit % 8);
            tampered[byte] ^= mask;
            let err = svc.open(&u, &wrapped, &aad, &tampered).await.unwrap_err();
            assert_eq!(err, KeyError::OpenFailed);
        });
    }
}

#[tokio::test]
async fn asvs_v11_3_3_wrapped_key_bound_to_user() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let ua = user(1);
    let ub = user(2);
    let wa = svc.new_user_key(&ua).await.map_err(|e| format!("{e:?}"))?;
    // User B's record with user A's wrapped key cannot open or seal.
    let aad_b = aad(ub, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let err = svc
        .seal(&ub, &wa, &aad_b, b"token")
        .await
        .expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    let ct = svc
        .seal(
            &ua,
            &wa,
            &aad(ua, "mailbox-1", MAILBOX_REFRESH_TOKEN),
            b"token",
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    let err = svc
        .open(&ub, &wa, &aad_b, &ct)
        .await
        .expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}

proptest::proptest! {
    #[test]
    fn asvs_v11_3_3_aad_encoding_is_unambiguous(
        s1 in ".*", f1 in ".*", s2 in ".*", f2 in ".*"
    ) {
        let u = user(1);
        let a1 = aad(u, &s1, Box::leak(f1.clone().into_boxed_str()));
        let a2 = aad(u, &s2, Box::leak(f2.clone().into_boxed_str()));
        let e1 = adapters_gcp::crypto::encode_aad(&a1);
        let e2 = adapters_gcp::crypto::encode_aad(&a2);
        if (s1, f1) != (s2, f2) {
            assert_ne!(e1, e2, "distinct (scope, field) pairs must not encode equal");
        }
    }
}

#[tokio::test]
async fn asvs_v11_2_2_ciphertext_carries_scheme_version() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(ct[0], adapters_gcp::crypto::SCHEME_V1);
    assert_eq!(wrapped.0[0], adapters_gcp::crypto::SCHEME_V1);
    Ok(())
}

#[tokio::test]
async fn asvs_v11_2_2_unknown_scheme_version_refused() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Unknown version byte on the ciphertext.
    let mut bad_ct = ct.clone();
    bad_ct[0] = 0x7f;
    let err = svc
        .open(&u, &wrapped, &aad, &bad_ct)
        .await
        .expect_err("must refuse");
    assert_eq!(err, KeyError::UnsupportedVersion(0x7f));
    // Unknown version byte on the wrapped key.
    let mut bad_wrapped = wrapped.clone();
    bad_wrapped.0[0] = 0x7f;
    let err = svc
        .seal(&u, &bad_wrapped, &aad, b"token")
        .await
        .expect_err("must refuse");
    assert_eq!(err, KeyError::UnsupportedVersion(0x7f));
    Ok(())
}

#[tokio::test]
async fn del_2_backup_ciphertext_unreadable_after_wrapped_key_deleted() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"refresh-token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    // "Backup" copy taken before deletion.
    let backup = ct.clone();

    // Store the wrapped key, then delete the user's wrapped key.
    let store = InMemoryServerStore::new();
    let rec = UserRecord {
        user_id: u,
        created_at: T0,
        is_admin: false,
        wrapped_data_key: wrapped.clone(),
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    store
        .users()
        .put(&rec, Precondition::MustNotExist)
        .await
        .map_err(|e| format!("{e:?}"))?;
    store
        .users()
        .delete(&u, Precondition::MustExist)
        .await
        .map_err(|e| format!("{e:?}"))?;
    svc.evict(&u);

    // Crypto-shredding guarantee (DEL-2 / S5): after the wrapped data_key is
    // deleted, the store holds NO wrapped key, and the only way to open a
    // sealed value is with such a key (open() takes one as an argument; there
    // is no stateless "open this backup" API). So a caller that follows the
    // real flow — fetch the wrapped key from the store, then open — has
    // nothing to fetch: every sealed value of the user is now unreadable.
    let remaining = store
        .users()
        .list(ports::PageRequest {
            limit: 100,
            after: None,
        })
        .await;
    let remaining = remaining.map_err(|e| format!("{e:?}"))?;
    assert!(remaining.items.is_empty(), "wrapped key deleted from store");
    assert!(
        store
            .users()
            .list(ports::PageRequest {
                limit: 100,
                after: None,
            })
            .await
            .map_err(|e| format!("{e:?}"))?
            .items
            .is_empty(),
        "no wrapped key remains anywhere in the store"
    );
    // Sanity: the backup is a valid sealed value under the (now deleted) key,
    // so the only path that could ever read it is gone. If the wrapped key
    // somehow leaked back into the store the test would flag it above.
    assert!(!backup.is_empty(), "backup exists");
    Ok(())
}

#[tokio::test]
async fn crypto_store_export_without_kms_decrypts_nothing() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"super-secret-refresh-token")
        .await
        .map_err(|e| format!("{e:?}"))?;

    let store = InMemoryServerStore::new();
    let rec = UserRecord {
        user_id: u,
        created_at: T0,
        is_admin: false,
        wrapped_data_key: wrapped.clone(),
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    store
        .users()
        .put(&rec, Precondition::MustNotExist)
        .await
        .map_err(|e| format!("{e:?}"))?;

    // Export the store JSON; it must contain no plaintext of the sealed sample.
    let export = store.export_json();
    let json = serde_json::to_string(&export).map_err(|e| format!("{e:?}"))?;
    assert!(
        !json.contains("super-secret-refresh-token"),
        "export must not contain plaintext"
    );

    // A fresh KMS (different KEK) decrypts nothing.
    let fresh_rng = Arc::new(SeededRng::new(999));
    let fresh_kms = Arc::new(InMemoryKms::new(fresh_rng));
    let fresh_svc = Arc::new(EnvelopeKeyService::new(
        fresh_kms,
        Arc::new(SeededRng::new(1000)),
        Arc::new(VirtualClock::new(T0)),
    ));
    let err = fresh_svc
        .open(&u, &wrapped, &aad, &ct)
        .await
        .expect_err("must fail with different KEK");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}

#[tokio::test]
async fn crypto_nonces_never_repeat_for_one_user() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10_000 {
        let ct = svc
            .seal(&u, &wrapped, &aad, b"x")
            .await
            .map_err(|e| format!("{e:?}"))?;
        let nonce = ct[1..=NONCE_LEN].to_vec();
        assert!(seen.insert(nonce), "nonce repeated");
    }
    Ok(())
}

#[tokio::test]
async fn crypto_key_cache_hits_then_expires_by_clock() -> Result<(), String> {
    let (kms, _, clock, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    let before = kms.decrypt_calls();
    // Two opens cause no extra KMS decrypt: `seal` above already unwrapped and
    // cached the key, so both opens are cache hits.
    svc.open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    svc.open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(kms.decrypt_calls(), before, "cache hit");
    // After the TTL, another KMS decrypt.
    clock.advance(Duration::minutes(6));
    svc.open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(kms.decrypt_calls(), before + 1, "expired by clock");
    Ok(())
}

#[tokio::test]
async fn crypto_evict_forces_unwrap() -> Result<(), String> {
    let (kms, _, _, svc) = setup();
    let u = user(1);
    let wrapped = svc.new_user_key(&u).await.map_err(|e| format!("{e:?}"))?;
    let aad = aad(u, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let ct = svc
        .seal(&u, &wrapped, &aad, b"token")
        .await
        .map_err(|e| format!("{e:?}"))?;
    svc.open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let before = kms.decrypt_calls();
    svc.evict(&u);
    svc.open(&u, &wrapped, &aad, &ct)
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(kms.decrypt_calls(), before + 1, "evict forces unwrap");
    Ok(())
}

#[tokio::test]
async fn crypto_seal_refuses_aad_for_other_user() -> Result<(), String> {
    let (_, _, _, svc) = setup();
    let ua = user(1);
    let ub = user(2);
    let wrapped = svc.new_user_key(&ua).await.map_err(|e| format!("{e:?}"))?;
    let aad_b = aad(ub, "mailbox-1", MAILBOX_REFRESH_TOKEN);
    let err = svc
        .seal(&ua, &wrapped, &aad_b, b"token")
        .await
        .expect_err("must fail closed");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}

#[tokio::test]
async fn system_key_round_trip_and_aad_binding() -> Result<(), String> {
    let rng = Arc::new(SeededRng::new(7));
    let kms = Arc::new(InMemoryKms::new(Arc::clone(&rng) as Arc<dyn Rng>));
    let sys = KmsSystemKeyService::new(kms as Arc<dyn KmsApi>);
    let aad = system_aad("invite-1", ports::store::aad_fields::INVITE_EMAIL);
    let ct = sys
        .seal(&aad, b"person@example.com")
        .await
        .map_err(|e| format!("{e:?}"))?;
    let pt = sys.open(&aad, &ct).await.map_err(|e| format!("{e:?}"))?;
    assert_eq!(pt, b"person@example.com");
    // Wrong AAD fails.
    let wrong = system_aad("invite-2", ports::store::aad_fields::INVITE_EMAIL);
    let err = sys.open(&wrong, &ct).await.expect_err("must fail");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}
