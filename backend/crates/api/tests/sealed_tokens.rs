//! T-303 sealed-token tests: the ASVS V9 matrix, expiry, cross-type,
//! cross-user, cross-session, TTL and status mapping, against
//! `EnvelopeKeyService` with an in-memory KMS and a virtual clock.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use adapters_gcp::{EnvelopeKeyService, KmsApi};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use api::sealed::{
    http_status_for, SealedTokens, TokenError, TokenType, MAX_TOKEN_LEN, MAX_TOKEN_TTL,
    TOKEN_PREFIX,
};
use domain::UserId;
use ports::store::SessionRecordId;
use ports::{Clock, KeyError, KeyService, Rng, WrappedKey};
use serde::{Deserialize, Serialize};
use testkit::{SeededRng, VirtualClock, T0};
use time::Duration;
use uuid::Uuid;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// An in-memory KMS: AES-256-GCM under a random KEK, enforcing the KMS AAD
/// exactly like Cloud KMS (a wrong AAD fails to decrypt). Copied from T-302's
/// tests.
struct InMemoryKms {
    kek: [u8; 32],
    rng: Arc<dyn Rng>,
}

impl InMemoryKms {
    fn new(rng: Arc<dyn Rng>) -> Self {
        Self {
            kek: rng.bytes32(),
            rng,
        }
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

/// A payload type used with the module. `deny_unknown_fields` so an unknown
/// field is rejected (S7 2.1).
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TokenPayload {
    value: u64,
}

fn user(n: u64) -> UserId {
    UserId::new(Uuid::from_u64_pair(0, n))
}

fn session(n: u64) -> SessionRecordId {
    SessionRecordId(Uuid::from_u64_pair(1, n))
}

/// A test harness: the key service (to mint wrapped keys) and the token
/// service under test, sharing one virtual clock.
struct Harness {
    clock: Arc<VirtualClock>,
    keys: Arc<EnvelopeKeyService>,
    tokens: Arc<SealedTokens>,
}

fn setup() -> Harness {
    let rng = Arc::new(SeededRng::new(42));
    let clock = Arc::new(VirtualClock::new(T0));
    let kms = Arc::new(InMemoryKms::new(Arc::clone(&rng) as Arc<dyn Rng>));
    let keys = Arc::new(EnvelopeKeyService::new(
        Arc::clone(&kms) as Arc<dyn KmsApi>,
        Arc::clone(&rng) as Arc<dyn Rng>,
        Arc::clone(&clock) as Arc<dyn ports::Clock>,
    ));
    let tokens = Arc::new(SealedTokens::new(
        Arc::clone(&keys) as Arc<dyn KeyService>,
        Arc::clone(&clock) as Arc<dyn ports::Clock>,
    ));
    Harness {
        clock,
        keys,
        tokens,
    }
}

/// Seals a token for `user`/`session` expiring `ttl` from the harness clock.
async fn seal(
    h: &Harness,
    ty: TokenType,
    user: &UserId,
    wrapped: &WrappedKey,
    session: &SessionRecordId,
    ttl: Duration,
) -> Result<String, TokenError> {
    h.tokens
        .seal(
            ty,
            user,
            wrapped,
            session,
            h.clock.now() + ttl,
            &TokenPayload { value: 7 },
        )
        .await
}

#[tokio::test]
async fn asvs_v9_1_1_tampered_body_refused() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &u, &wrapped, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Flip every byte of the body in turn; each must be refused. A flip that
    // makes the token non-UTF-8 cannot be passed to `open` (it takes `&str`),
    // so it is trivially malformed; we test every flip that keeps the token a
    // valid string, and each must be refused.
    let body = token.rsplit('.').next().unwrap();
    for i in 0..body.len() {
        for mask in [1u8, 2, 4, 8, 16, 32, 64, 128] {
            let mut bytes = token.clone().into_bytes();
            let body_start = bytes.len() - body.len();
            bytes[body_start + i] ^= mask;
            let Ok(tampered) = String::from_utf8(bytes) else {
                continue;
            };
            let err = h
                .tokens
                .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &tampered)
                .await
                .expect_err("tampered body must be refused");
            assert!(
                matches!(err, TokenError::Invalid | TokenError::Malformed),
                "got {err:?}"
            );
        }
    }
    // Sanity: the untouched token still opens.
    h.tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &token)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}
#[tokio::test]
async fn asvs_v9_1_2_unknown_prefix_refused() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &u, &wrapped, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    for prefix in ["mt0", "mt2", "MT1", ""] {
        let rest = &token[TOKEN_PREFIX.len()..];
        let tampered = format!("{prefix}{rest}");
        let err = h
            .tokens
            .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &tampered)
            .await
            .expect_err("unknown prefix must be refused");
        assert_eq!(err, TokenError::Malformed, "prefix {prefix:?}");
    }
    Ok(())
}

#[tokio::test]
async fn asvs_v9_1_3_no_raw_key_constructor() -> Result<(), String> {
    // Compile-time: `SealedTokens::new` takes `Arc<dyn KeyService>` only; there
    // is no constructor that takes raw key bytes. Runtime: a token sealed for
    // user A's wrapped key fails with user B's wrapped key.
    let h = setup();
    let ua = user(1);
    let ub = user(2);
    let s = session(1);
    let wa = h
        .keys
        .new_user_key(&ua)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let wb = h
        .keys
        .new_user_key(&ub)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &ua, &wa, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &ub, &wb, &s, &token)
        .await
        .expect_err("user B's key must not open user A's token");
    assert_eq!(err, TokenError::Invalid);
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_1_expired_token_refused_at_boundary() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let now = h.clock.now();
    let token = h
        .tokens
        .seal(
            TokenType::Cursor,
            &u,
            &wrapped,
            &s,
            now + Duration::hours(1),
            &TokenPayload { value: 7 },
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    // now == exp - 1 opens.
    h.clock.set(now + Duration::hours(1) - Duration::seconds(1));
    h.tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &token)
        .await
        .map_err(|e| format!("{e:?}"))?;
    // now == exp is Expired.
    h.clock.set(now + Duration::hours(1));
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &token)
        .await
        .expect_err("must be expired at the boundary");
    assert_eq!(err, TokenError::Expired);
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_1_altered_expiry_refused() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &u, &wrapped, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Rewrite part 2 (the clear expiry) to a later time.
    let parts: Vec<&str> = token.splitn(4, '.').collect();
    let later = h.clock.now().unix_timestamp() + 86_400;
    let tampered = format!("{}.{}.{}.{}", parts[0], parts[1], later, parts[3]);
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &tampered)
        .await
        .expect_err("altered expiry must fail authentication");
    assert_eq!(err, TokenError::Invalid);
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_2_every_type_refused_as_every_other_type() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    for &sealed_ty in &TokenType::ALL {
        let token = seal(&h, sealed_ty, &u, &wrapped, &s, Duration::hours(1))
            .await
            .map_err(|e| format!("{e:?}"))?;
        for &expected in &TokenType::ALL {
            if expected == sealed_ty {
                // Same type opens.
                h.tokens
                    .open::<TokenPayload>(expected, &u, &wrapped, &s, &token)
                    .await
                    .map_err(|e| format!("{e:?}"))?;
            } else {
                // Different type: the unchanged token at the wrong route is
                // refused with WrongType.
                let err = h
                    .tokens
                    .open::<TokenPayload>(expected, &u, &wrapped, &s, &token)
                    .await
                    .expect_err("wrong type must be refused");
                assert_eq!(err, TokenError::WrongType, "{sealed_ty:?} as {expected:?}");

                // Relabel the clear type to the expected one; the AEAD must
                // still fail because the AAD is rebuilt from the expected type.
                let parts: Vec<&str> = token.splitn(4, '.').collect();
                let relabelled =
                    format!("{}.{}.{}.{}", parts[0], expected.wire(), parts[2], parts[3]);
                let err = h
                    .tokens
                    .open::<TokenPayload>(expected, &u, &wrapped, &s, &relabelled)
                    .await
                    .expect_err("relabelled type must be refused");
                assert_eq!(
                    err,
                    TokenError::Invalid,
                    "{sealed_ty:?} relabelled as {expected:?}"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_3_other_user_refused() -> Result<(), String> {
    let h = setup();
    let ua = user(1);
    let ub = user(2);
    let s = session(1);
    let wa = h
        .keys
        .new_user_key(&ua)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let wb = h
        .keys
        .new_user_key(&ub)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &ua, &wa, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &ub, &wb, &s, &token)
        .await
        .expect_err("other user must be refused");
    assert_eq!(err, TokenError::Invalid);
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_3_other_session_record_refused() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s1 = session(1);
    let s2 = session(2);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Cursor, &u, &wrapped, &s1, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s2, &token)
        .await
        .expect_err("other session record must be refused");
    assert_eq!(err, TokenError::Invalid);
    Ok(())
}

#[tokio::test]
async fn sw_05_ac4_undo_token_survives_session_id_rotation() -> Result<(), String> {
    // The token binds to `session_record_id`, which is stable across session
    // rotation. Opening with the same record ID (a new session hash is not
    // part of the token) must succeed.
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let token = seal(&h, TokenType::Undo, &u, &wrapped, &s, Duration::hours(1))
        .await
        .map_err(|e| format!("{e:?}"))?;
    // The same session_record_id after rotation opens the undo token.
    h.tokens
        .open::<TokenPayload>(TokenType::Undo, &u, &wrapped, &s, &token)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[tokio::test]
async fn sealed_ttl_over_twelve_hours_refused_at_seal() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let now = h.clock.now();
    let err = h
        .tokens
        .seal(
            TokenType::Cursor,
            &u,
            &wrapped,
            &s,
            now + MAX_TOKEN_TTL + Duration::seconds(1),
            &TokenPayload { value: 7 },
        )
        .await
        .expect_err("ttl over 12 hours must be refused at seal");
    assert_eq!(err, TokenError::TtlTooLong);
    // Exactly 12 hours is allowed.
    h.tokens
        .seal(
            TokenType::Cursor,
            &u,
            &wrapped,
            &s,
            now + MAX_TOKEN_TTL,
            &TokenPayload { value: 7 },
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[tokio::test]
async fn sealed_oversized_or_non_ascii_token_is_malformed_without_decoding() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Oversized.
    let big = "x".repeat(MAX_TOKEN_LEN + 1);
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &big)
        .await
        .expect_err("oversized token must be malformed");
    assert_eq!(err, TokenError::Malformed);
    // Non-ASCII.
    let non_ascii = "mt1.cursor.1234.\u{00e9}";
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, non_ascii)
        .await
        .expect_err("non-ascii token must be malformed");
    assert_eq!(err, TokenError::Malformed);
    Ok(())
}

#[tokio::test]
async fn sealed_unknown_payload_field_is_invalid() -> Result<(), String> {
    let h = setup();
    let u = user(1);
    let s = session(1);
    let wrapped = h
        .keys
        .new_user_key(&u)
        .await
        .map_err(|e| format!("{e:?}"))?;
    // Seal a payload with an extra field, then open as the strict `Payload`.
    let now = h.clock.now();
    let token = h
        .tokens
        .seal(
            TokenType::Cursor,
            &u,
            &wrapped,
            &s,
            now + Duration::hours(1),
            &serde_json::json!({ "value": 7, "extra": 1 }),
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    let err = h
        .tokens
        .open::<TokenPayload>(TokenType::Cursor, &u, &wrapped, &s, &token)
        .await
        .expect_err("unknown payload field must be invalid");
    assert_eq!(err, TokenError::Invalid);
    Ok(())
}

#[test]
fn sealed_undo_failures_map_to_410_others_to_400() {
    // Undo failures map to 410 undo_expired; all others to 400 invalid_request.
    for err in [
        TokenError::Malformed,
        TokenError::WrongType,
        TokenError::Expired,
        TokenError::Invalid,
        TokenError::TtlTooLong,
    ] {
        assert_eq!(
            http_status_for(TokenType::Undo, err),
            (410, "undo_expired"),
            "{err:?}"
        );
        assert_eq!(
            http_status_for(TokenType::Cursor, err),
            (400, "invalid_request"),
            "{err:?}"
        );
    }
    // Unavailable maps to 503 for every type.
    assert_eq!(
        http_status_for(TokenType::Undo, TokenError::Unavailable),
        (503, "provider_unavailable")
    );
    assert_eq!(
        http_status_for(TokenType::Cursor, TokenError::Unavailable),
        (503, "provider_unavailable")
    );
}
