# T-303: Sealed tokens

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | strong | about 250 lines of code plus about 300 lines of tests | T-302 |

**Read only these spec sections:** S6 section 3 row T22 and section 5 "Sealed tokens" paragraph (`docs/specs/S6-security.md`); S7 2 rows "Opaque tokens" and 2.1 whole, S7 4 rows `invalid_request` and `undo_expired` (`docs/specs/S7-api-contract.md`); S10 7.2 "Sealed tokens" row (`docs/specs/S10-test-strategy.md`); ASVS register rows V9.1.1, V9.1.2, V9.1.3, V9.2.1, V9.2.2, V9.2.3, V9.2.4 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-302-kms-envelope-data-key.md` ("Types and signatures" and Algorithm steps 1 to 7). Nothing else is needed.

## Goal

The `api` crate gains `SealedTokens`, the one scheme for every opaque token the server hands the browser (`cursor`, `undo`, `classification`, `prompt_ref`): AES-256-GCM under the user's `data_key` through `KeyService`, with the token type, user ID, session record ID and expiry in the associated data, and type and expiry also in clear. A token of one type can never be accepted as another, for another user or session, or after its expiry. Routes (T-602c, T-604, T-605, T-705 and others) call only `seal` and `open`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/sealed.rs` | `TokenType`, `SealedTokens`, `TokenError` |
| Change | `backend/crates/api/src/lib.rs` | `pub mod sealed;` |
| Change | `backend/crates/api/Cargo.toml` | deps `ports`, `domain`, `obs`, `serde`, `serde_json`, `base64`, `time`; dev `testkit`, `adapters-gcp` (for `EnvelopeKeyService` in tests), `proptest` |
| Create | `backend/crates/api/tests/sealed_tokens.rs` | the matrix below |

## Types and signatures

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenType { Cursor, Undo, Classification, PromptRef }
impl TokenType {
    pub const ALL: [TokenType; 4] = [TokenType::Cursor, TokenType::Undo, TokenType::Classification, TokenType::PromptRef];
    pub fn wire(self) -> &'static str;        // "cursor", "undo", "classification", "prompt_ref"
    pub fn aad_field(self) -> &'static str;   // "sealed.cursor", "sealed.undo", "sealed.classification", "sealed.prompt_ref"
}

pub const TOKEN_PREFIX: &str = "mt1";                         // scheme version 1
pub const MAX_TOKEN_LEN: usize = 8192;                        // [DEFAULT] checked before any decoding
pub const MAX_TOKEN_TTL: time::Duration = time::Duration::hours(12); // S7 3.2 absolute session lifetime

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")] Malformed,     // shape, length, base64, prefix
    #[error("wrong token type")] WrongType,    // clear type differs from the expected type
    #[error("token expired")] Expired,         // now >= exp
    #[error("token invalid")] Invalid,         // AEAD open failed: other user, session, type, expiry, or tampering
    #[error("key service unavailable")] Unavailable,
    #[error("ttl too long")] TtlTooLong,       // programming error at seal time
}

pub struct SealedTokens { keys: Arc<dyn KeyService>, clock: Arc<dyn Clock> }
impl SealedTokens {
    pub fn new(keys: Arc<dyn KeyService>, clock: Arc<dyn Clock>) -> Self;
    pub async fn seal<T: Serialize>(&self, ty: TokenType, user: &UserId, wrapped: &WrappedKey,
        session: &SessionRecordId, expires_at: OffsetDateTime, payload: &T) -> Result<String, TokenError>;
    pub async fn open<T: DeserializeOwned>(&self, expected: TokenType, user: &UserId, wrapped: &WrappedKey,
        session: &SessionRecordId, token: &str) -> Result<T, TokenError>;
}
/// Route helper: every failure is 400 invalid_request, except an undo route, where every failure is 410 undo_expired.
pub fn http_status_for(expected: TokenType, err: TokenError) -> (u16, &'static str);
```

Wire format: `mt1.<type>.<exp>.<body>`

- `<type>`: `TokenType::wire()`.
- `<exp>`: expiry as Unix seconds, decimal, digits only, no sign, at most 12 digits.
- `<body>`: unpadded base64url of the `KeyService::seal` output (`0x01 || nonce || ciphertext || tag`, T-302).
- Associated data: `Aad { user, scope: format!("{}|{}", session.0.hyphenated(), exp), field: ty.aad_field() }`, which T-302 encodes as `label || user || len scope || scope || len field || field`. So type, user ID, session record ID and expiry are all authenticated (S6 5).

## Algorithm

1. `seal(ty, user, wrapped, session, expires_at, payload)`:
   1. `now = clock.now()`. If `expires_at <= now` or `expires_at - now > MAX_TOKEN_TTL`, return `TtlTooLong` (callers pass `min(now + 12 h, session absolute expiry)`).
   2. `exp = expires_at.unix_timestamp()`.
   3. `plaintext = serde_json::to_vec(payload)`.
   4. `sealed = keys.seal(user, wrapped, &aad(ty, user, session, exp), &plaintext)`; map `KeyError::Unavailable` to `Unavailable`, any other error to `Invalid`.
   5. Return `format!("{TOKEN_PREFIX}.{}.{exp}.{}", ty.wire(), b64url_nopad(sealed))`; if longer than `MAX_TOKEN_LEN`, return `Malformed` (the payload is too big; a programming error).
2. `open(expected, user, wrapped, session, token)`, in this order:
   1. `token.len() > MAX_TOKEN_LEN` or not ASCII: `Malformed`.
   2. `splitn(4, '.')` gives exactly four parts; part 0 must equal `TOKEN_PREFIX`; else `Malformed`.
   3. Part 1 must be one of the four wire names, else `Malformed`; if it is not `expected.wire()`, return `WrongType`. (The AEAD below would also fail; this check only gives a clearer error.)
   4. Part 2: digits only, 1 to 12 chars, parse `i64`; else `Malformed`.
   5. If `clock.now().unix_timestamp() >= exp`: `Expired`.
   6. Part 3: base64url decode (no padding allowed); else `Malformed`.
   7. Build the AAD from `expected` (never from the type the token claims), `user`, `session` and the parsed `exp`. `keys.open(...)`; any `KeyError` other than `Unavailable` gives `Invalid`.
   8. `serde_json::from_slice::<T>`; failure gives `Invalid`. Payload types used with this module derive `Deserialize` with `#[serde(deny_unknown_fields)]`.
3. `http_status_for`: `Unavailable` gives `(503, "provider_unavailable")` `[DEFAULT]`; otherwise `Undo` gives `(410, "undo_expired")` and every other type `(400, "invalid_request")` (S7 2.1).
4. The key comes only from `KeyService` with the user's KMS-wrapped `data_key` (ASVS V9.1.3). There is no constructor that takes raw key bytes, no algorithm field in the token (the algorithm is fixed by the `mt1` prefix, ASVS V9.1.2), and no `none` option.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| V9.1.1 | A token is accepted only after its GCM tag verifies |
| V9.1.2 | The algorithm is fixed by the scheme prefix; no token can select another algorithm |
| V9.1.3 | The sealing key comes only from the user's KMS-unwrapped `data_key` |
| V9.2.1 | A token at or past its expiry is refused, and the expiry cannot be altered |
| V9.2.2 | A token of one type is refused at a route that expects another type |
| V9.2.3 | A token is bound to one user and one session record |
| SW-05 AC4 | Undo tokens keep working after the session ID rotates, because they bind to `session_record_id` |

## Tests that must pass

All in `api/tests/sealed_tokens.rs`, using `EnvelopeKeyService` with the in-memory KMS from T-302's tests (copy the small struct) or `testkit::FakeKeyService`, plus `VirtualClock`.

- `asvs_v9_1_1_tampered_body_refused` (property, `proptest`: flip any byte of the body; `Invalid` or `Malformed`, never `Ok`).
- `asvs_v9_1_2_unknown_prefix_refused` (unit: `mt0`, `mt2`, `MT1`, empty prefix).
- `asvs_v9_1_3_no_raw_key_constructor` (unit: compile-time; `SealedTokens::new` takes `Arc<dyn KeyService>` only; plus a token sealed for user A's wrapped key fails with user B's wrapped key).
- `asvs_v9_2_1_expired_token_refused_at_boundary` (unit: `now == exp` is `Expired`; `now == exp - 1` opens).
- `asvs_v9_2_1_altered_expiry_refused` (unit: rewrite part 2 to a later time; `Invalid`).
- `asvs_v9_2_2_every_type_refused_as_every_other_type` (unit: 4 x 3 matrix; both "relabelled clear type" and "unchanged token at the wrong route" give a refusal).
- `asvs_v9_2_3_other_user_refused` and `asvs_v9_2_3_other_session_record_refused` (unit).
- `sw_05_ac4_undo_token_survives_session_id_rotation` (unit: same `session_record_id`, new session hash; opens).
- `sealed_ttl_over_twelve_hours_refused_at_seal` (unit).
- `sealed_oversized_or_non_ascii_token_is_malformed_without_decoding` (unit).
- `sealed_unknown_payload_field_is_invalid` (unit).
- `sealed_undo_failures_map_to_410_others_to_400` (unit on `http_status_for`).

## Edge cases and traps

- Rebuild the associated data from the type the route expects, never from part 1 of the token. Using the claimed type would let a cursor be replayed as an undo token whenever the two payloads happen to parse.
- `Expired` is checked before decryption to save a KMS call, but the expiry is still inside the AAD, so editing the clear expiry fails authentication.
- Do not use `splitn` with a limit that lets extra dots into the body; base64url never contains `.`, so exactly four parts is right.
- Base64url without padding only; refuse `=` and standard base64 characters `+` and `/`.
- Never log tokens, payloads, user IDs or session record IDs from this module.
- Do not add a cache of opened tokens; each open is stateless.
- The session record ID is `SessionRecordId`, never the cookie value or `session_hash` (S7 2.1).
- Payload structs must not include the type or user ID; those live in the AAD.
- `TokenError` must not say which check failed beyond the variants above; routes collapse them to one status anyway.

## Out of scope

- Payload contents for each route (T-602c, T-604, T-605, T-608, T-705).
- Session handling and `session_record_id` creation (T-501).

## Security review checklist

- Wire format is exactly `mt1.<type>.<exp>.<b64url body>`, length-checked before decoding.
- AAD = token type, user ID, session record ID, expiry (S6 5), rebuilt from the expected type; scope string is `session|exp` and is length-prefixed by T-302's encoder.
- AES-256-GCM via `KeyService` only (no second implementation); random 96-bit nonce per token (T-302).
- Expiry: refused at `now >= exp`; maximum lifetime 12 hours enforced at seal.
- Every cross-type, cross-user, cross-session, tampered and expired case is in the test matrix and fails closed.
- Undo failures map to `410 undo_expired`, all others to `400 invalid_request`; no detail leaks.
- No JWT library, no `alg` header, no `none`.
- Nothing from this module is logged.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A strong-model review has signed off the checklist above in the pull request.
