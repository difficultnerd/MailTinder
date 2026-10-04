# T-302: KMS envelope encryption and the per-user data key

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | strong | about 450 lines of code plus about 350 lines of tests | T-202b, T-301 |

**Read only these spec sections:** S6 section 3 row T1, section 5 whole (`docs/specs/S6-security.md`); S5 "Keys and secrets held outside Firestore" and the `users/{id}: wrapped_data_key` and `mailboxes/{id}: refresh token` rows, DEL-2 (`docs/specs/S5-data-inventory.md`); S10 7.2 "Crypto" row (`docs/specs/S10-test-strategy.md`); ASVS register rows V11.2.2, V11.3.1, V11.3.2, V11.3.3, V11.4.1, V11.5.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-201a-port-traits.md` ("keys.rs" block) and `docs/backlog/T-201b-server-store-traits.md` (`aad_fields`). Nothing else is needed.

## Goal

`adapters-gcp` gains `EnvelopeKeyService`, the production `KeyService`: each user has one random 256-bit `data_key`, wrapped by a Cloud KMS key encryption key (KEK) and stored only in wrapped form; values are sealed with AES-256-GCM under the unwrapped `data_key` with associated data that binds every ciphertext to its user, scope and field. It also gains `KmsSystemKeyService` for the few fields that exist before a user does. Deleting a user's wrapped key makes every ciphertext of that user unreadable (crypto-shredding). Nothing here stores plaintext keys anywhere but process memory.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gcp/src/crypto/mod.rs` | constants, formats, AAD encoding, `aead_seal`, `aead_open` (pure) |
| Create | `backend/crates/adapters-gcp/src/crypto/kms.rs` | `KmsApi` trait, `CloudKms` (REST through `GcpHttp`) |
| Create | `backend/crates/adapters-gcp/src/crypto/envelope.rs` | `EnvelopeKeyService` with the unwrapped key cache |
| Create | `backend/crates/adapters-gcp/src/crypto/system.rs` | `KmsSystemKeyService` |
| Create | `backend/crates/adapters-gcp/tests/crypto_envelope.rs` | tests with an in-memory KMS defined in the test file |
| Create | `backend/crates/adapters-gcp/tests/kms_rest_shape.rs` | `CloudKms` against a local `axum` stub |
| Change | `backend/crates/adapters-gcp/Cargo.toml` | add `aes-gcm` (features `aes`, `zeroize`), `zeroize`, `crc32c`, `sha2` |
| Change | `.semgrep/mailtinder.yml` | crypto rules (below); T-003 created the file |
| Change | `.semgrep/tests/mailtinder.rs` | rule test cases for the new rules |

## Types and signatures

```rust
// crypto/mod.rs
pub const SCHEME_V1: u8 = 0x01;                 // AES-256-GCM, 96-bit random nonce, 128-bit tag
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const MAX_PLAINTEXT: usize = 8 * 1024 * 1024; // [DEFAULT] covers the 5 MiB app folder file (T-405)
pub const AEAD_AAD_LABEL: &[u8] = b"mailtinder.aead.v1\0";
pub const DEK_WRAP_AAD_LABEL: &[u8] = b"mailtinder.dek.v1\0";
pub const SYSTEM_AAD_LABEL: &[u8] = b"mailtinder.sys.v1\0";

/// label || user UUID (16 raw bytes) || u32 BE len(scope) || scope UTF-8 || u32 BE len(field) || field UTF-8
pub fn encode_aad(aad: &ports::Aad) -> Vec<u8>;
/// label || u32 BE len(scope) || scope || u32 BE len(field) || field
pub fn encode_system_aad(aad: &ports::SystemAad) -> Vec<u8>;
/// DEK_WRAP_AAD_LABEL || user UUID (16 raw bytes)
pub fn dek_wrap_aad(user: &UserId) -> Vec<u8>;
/// Output: SCHEME_V1 || nonce (12) || ciphertext || tag (16)
pub fn aead_seal(key: &[u8; 32], nonce: [u8; 12], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, KeyError>;
pub fn aead_open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, KeyError>;

// crypto/kms.rs
#[async_trait::async_trait]
pub trait KmsApi: Send + Sync {
    async fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, KeyError>;
    async fn decrypt(&self, ciphertext: &[u8], aad: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyError>;
}
pub struct CloudKms { http: Arc<GcpHttp>, key_name: String } // projects/{p}/locations/us-central1/keyRings/{r}/cryptoKeys/{k}
impl CloudKms { pub fn new(http: Arc<GcpHttp>, key_name: String) -> Self; }

// crypto/envelope.rs
pub const KEY_CACHE_TTL: time::Duration = time::Duration::minutes(5); // [DEFAULT]
pub const KEY_CACHE_MAX: usize = 1024;                                // [DEFAULT]
pub struct EnvelopeKeyService { kms: Arc<dyn KmsApi>, rng: Arc<dyn Rng>, clock: Arc<dyn Clock>,
    cache: Mutex<HashMap<(UserId, [u8; 32]), (zeroize::Zeroizing<[u8; 32]>, OffsetDateTime)>> }
impl EnvelopeKeyService {
    pub fn new(kms: Arc<dyn KmsApi>, rng: Arc<dyn Rng>, clock: Arc<dyn Clock>) -> Self;
    /// Drop any cached unwrapped key for this user (account deletion, T-803).
    pub fn evict(&self, user: &UserId);
}
impl KeyService for EnvelopeKeyService { /* below */ }

// crypto/system.rs
pub struct KmsSystemKeyService { kms: Arc<dyn KmsApi> }  // a SECOND KMS key: ".../cryptoKeys/system-fields" [DEFAULT]
impl SystemKeyService for KmsSystemKeyService { /* below */ }
```

## Algorithm

Formats (all versioned for crypto agility, ASVS V11.2.2):

- `WrappedKey` bytes = `SCHEME_V1 || <KMS ciphertext>`.
- Sealed value = `SCHEME_V1 || nonce(12) || AES-256-GCM ciphertext || tag(16)`.
- System-sealed value = `SCHEME_V1 || <KMS ciphertext>`.

1. `encode_aad`: length-prefixed fields (never plain concatenation, where `scope = "ab", field = "c"` would equal `scope = "a", field = "bc"`). The label separates the three AAD kinds so a value from one kind can never authenticate as another.
2. `aead_seal(key, nonce, aad, pt)`: refuse `pt.len() > MAX_PLAINTEXT` (`Malformed`); `Aes256Gcm::new(key).encrypt(nonce, Payload { msg: pt, aad })`; prefix `SCHEME_V1` and the nonce.
3. `aead_open(key, aad, sealed)`: length `< 1 + 12 + 16` gives `Malformed`; first byte not `SCHEME_V1` gives `UnsupportedVersion(b)`; decrypt with the nonce from bytes 1..13; any AEAD error gives `OpenFailed` (one error for wrong key, wrong AAD or tampering).
4. `new_user_key(user)`:
   1. `dek = Zeroizing::new(rng.bytes32())`.
   2. `kms_ct = kms.encrypt(&dek, &dek_wrap_aad(user))`. KMS associated data binds the wrapped key to its user: a wrapped key copied into another user's record fails to unwrap.
   3. Return `WrappedKey([SCHEME_V1] ++ kms_ct)`. The plaintext `dek` is dropped (zeroised) here; it is not cached, because the caller has not stored the wrapped key yet.
5. `unwrap(user, wrapped)` (private): cache key = `(user, SHA-256(wrapped bytes))`. Hit and younger than `KEY_CACHE_TTL` by `Clock`: use it. Else check the version byte, `kms.decrypt(rest, dek_wrap_aad(user))`, require exactly 32 bytes (else `Malformed`), insert into the cache (if the cache is full, clear it first `[DEFAULT]`, simple and safe). Never hold the cache lock across the KMS `.await`.
6. `seal(user, wrapped, aad, pt)`: require `aad.user == *user` (else `OpenFailed`, a caller bug); `key = unwrap`; nonce = first 12 bytes of `rng.bytes32()`; `aead_seal(key, nonce, encode_aad(aad), pt)`.
7. `open(user, wrapped, aad, ct)`: same `aad.user` check; `key = unwrap`; `aead_open`.
8. `CloudKms::encrypt`: `POST https://cloudkms.googleapis.com/v1/{key_name}:encrypt` with JSON `{"plaintext": b64(pt), "additionalAuthenticatedData": b64(aad), "plaintextCrc32c": "<crc32c(pt) as decimal string>", "additionalAuthenticatedDataCrc32c": "<crc32c(aad)>"}` (standard base64 with padding, as the API expects). Check the response: `verifiedPlaintextCrc32c == true`, `verifiedAdditionalAuthenticatedDataCrc32c == true`, and `crc32c(b64decode(ciphertext)) == ciphertextCrc32c`; any mismatch is `Unavailable` (corruption in transit; the caller may retry). Return the decoded `ciphertext`.
9. `CloudKms::decrypt`: `POST .../{key_name}:decrypt` (the crypto key name, not a version: KMS picks the version from the ciphertext, so yearly rotation needs no code) with `{"ciphertext", "additionalAuthenticatedData", "ciphertextCrc32c", "additionalAuthenticatedDataCrc32c"}`; check `crc32c(plaintext) == plaintextCrc32c`. Error mapping: HTTP 400 `INVALID_ARGUMENT` (wrong AAD, corrupt ciphertext) gives `OpenFailed`; 403 and 404 give `Denied`; 429, 5xx, timeout give `Unavailable`.
10. `KmsSystemKeyService` `[DEFAULT]`: `seal(aad, pt)` = `SCHEME_V1 || kms_system.encrypt(pt, encode_system_aad(aad))` (direct KMS encryption, plaintext at most 64 KiB, which KMS allows); `open` reverses. It uses a separate KMS key so S6's "the KEK wraps every data_key and nothing else" stays true. This key needs a decision from James (reported spec gap) and Terraform in T-1102.
11. Crypto-shredding: there is no destroy call on the port. T-803 deletes `users/{id}.wrapped_data_key` (and the user document) after the steps that still need it, then calls `evict(user)`. Every value sealed under that user's `data_key` becomes unreadable, because the only copy of `data_key` was the wrapped one.
12. Semgrep rules in `.semgrep/mailtinder.yml` (ASVS V11.3.1, V11.3.2, V11.4.1, V11.5.1), excluding `.semgrep/**`:
    - `mailtinder-crypto-approved-aead-only`: matches `Ecb`, `Cbc`, `Ctr<`, `Cfb`, `Ofb`, `Aes256::new`, `Pkcs1v15Encrypt`, `ChaCha20::new` (unauthenticated) anywhere under `backend/`.
    - `mailtinder-crypto-no-weak-hash`: matches `md5::`, `Md5`, `sha1::`, `Sha1` under `backend/`.
    - `mailtinder-no-non-csprng-secrets`: matches `rand::thread_rng`, `SmallRng`, `StdRng::seed_from_u64` outside `backend/crates/testkit/**`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| DEL-2 | After the wrapped `data_key` is deleted, a copy of an encrypted field taken earlier cannot be decrypted |
| V11.3.3 | Ciphertexts are AEAD with associated data binding user, scope and field; moving or altering one fails |
| V11.2.2 | Every ciphertext and wrapped key carries a scheme version byte; unknown versions are refused |

V11.3.1, V11.3.2, V11.4.1 and V11.5.1 are `semgrep` rows: the rules below verify them.

## Tests that must pass

All in `adapters-gcp/tests/crypto_envelope.rs` unless noted, using an `InMemoryKms` written in the test file (AES-256-GCM under a random KEK, enforcing the KMS AAD exactly like Cloud KMS), `SeededRng` and `VirtualClock`.

- `asvs_v11_3_3_round_trip` (unit).
- `asvs_v11_3_3_ciphertext_moved_to_other_user_fails` (unit).
- `asvs_v11_3_3_ciphertext_moved_to_other_scope_or_field_fails` (unit: refresh token of mailbox A opened as mailbox B, and as `mailbox.email_address`).
- `asvs_v11_3_3_any_flipped_byte_fails` (property, `proptest`: flip any bit of the sealed value; `open` fails).
- `asvs_v11_3_3_wrapped_key_bound_to_user` (unit: user B's record with user A's wrapped key cannot open or seal).
- `asvs_v11_3_3_aad_encoding_is_unambiguous` (property: distinct (scope, field) pairs never encode equal).
- `asvs_v11_2_2_ciphertext_carries_scheme_version` and `asvs_v11_2_2_unknown_scheme_version_refused` (unit).
- `del_2_backup_ciphertext_unreadable_after_wrapped_key_deleted` (unit: seal a refresh token, copy the bytes as a "backup", delete the user's wrapped key from an `InMemoryServerStore`, `evict`; then opening the backup with every wrapped key left in the store fails, and no API exists to open without one).
- `crypto_store_export_without_kms_decrypts_nothing` (unit: export the store JSON; with a fresh `InMemoryKms` (different KEK) nothing opens; the export contains no plaintext of any sealed sample).
- `crypto_nonces_never_repeat_for_one_user` (unit: 10,000 seals for one user with `SeededRng`; all nonces distinct).
- `crypto_key_cache_hits_then_expires_by_clock` (unit: two `open`s cause one KMS decrypt; after `KEY_CACHE_TTL` another).
- `crypto_evict_forces_unwrap` (unit).
- `crypto_seal_refuses_aad_for_other_user` (unit).
- `system_key_round_trip_and_aad_binding` (unit).
- `kms_rest_encrypt_sends_base64_and_crc32c_and_checks_verification` and `kms_rest_decrypt_maps_400_to_open_failed` (`kms_rest_shape.rs`, against a local `axum` stub through `GcpHttp::with_emulator`-style test wiring).
- Semgrep rule tests for the three new rules (`.semgrep/tests/mailtinder.rs`, run by the `privacy-checks` job).

## Edge cases and traps

- Use AES-256-GCM through the `aes-gcm` crate only; never hand-roll GCM or use a raw block cipher.
- The nonce must come from `Rng` (CSPRNG in production, `OsRng`), never a counter shared across instances and never derived from the plaintext. With random 96-bit nonces one key stays far below the 2^32 messages-per-key limit at trial scale; note it in a comment.
- Never cache keys by user alone: after a re-key the old entry would serve the wrong key. Cache by user plus a hash of the wrapped bytes.
- Never put plaintext keys, wrapped keys, AAD scopes or ciphertext in logs, errors or `Debug`. `KeyError` carries no detail on purpose.
- `Zeroizing` everything that holds key bytes. Do not `clone()` a key out of `Zeroizing` into a plain array.
- KMS JSON uses standard base64 with padding; our stored formats use raw bytes (the store encodes them, T-201b). Do not mix them up.
- Do not call KMS `:decrypt` with a key version name; use the crypto key name so rotation works.
- `aad.user` must equal the `user` argument; a mismatch is a programming error that must fail closed, not be ignored.
- Firestore backups: crypto-shredding is only complete if no backup still holds the deleted `users` document with its wrapped key, because the KEK is shared. Reported as a spec gap: T-1102 must not enable point-in-time recovery or scheduled exports that cover `users` beyond the 24-hour deletion window, or James must choose a per-user KMS key. Add a `// SHRED:` comment at `evict` pointing to this.
- Keep `cargo deny` green: no `aws-lc-sys`, no `openssl`, no `rsa`.

## Out of scope

- Deleting the user and ordering the deletion steps (T-803).
- Re-keying a user (S6 "on demand"): later task; the version byte makes it possible.
- Terraform for the KMS key ring, the two keys and IAM (T-1102). Staging smoke against real KMS (T-1105).
- Sealed token format (T-303), which builds on `KeyService`.

## Security review checklist

- AES-256-GCM via `aes-gcm`; 96-bit nonce from the CSPRNG `Rng` on every seal; 128-bit tag; no other cipher or mode anywhere (Semgrep rules present and tested).
- AAD for every sealed value is `label || user || len-prefixed scope || len-prefixed field`, and `aad_fields` constants are used by every caller; the refresh token AAD is user plus mailbox ID plus field (S6 5).
- Wrapped key: KMS encrypt with AAD = label plus user ID; unwrap with the same; KMS decrypt uses the crypto key name, not a version.
- CRC32C integrity fields are sent and verified on both KMS calls.
- Every ciphertext and wrapped key starts with the scheme version byte; unknown versions fail closed.
- Unwrapped keys exist only in `Zeroizing` memory, in a cache keyed by user plus wrapped-key hash, TTL by `Clock`, with `evict`.
- `OpenFailed` gives no oracle (same error for key, AAD and tag failures); nothing secret is logged.
- The system key is a separate KMS key and is only used for invite, invite request and `pre_auth` fields.
- DEL-2 test exists and the backup caveat is written down for T-1102 and James.
- Only `api`, `unsub` and `worker` will hold KMS encrypt and decrypt (Terraform, T-1102); nothing in this code assumes broader access.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A strong-model review has signed off the checklist above in the pull request.
- The register rows V11.2.2 and V11.3.3 cite the test names above.
