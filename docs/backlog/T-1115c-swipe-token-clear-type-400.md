# T-1115c: Swipe: an unrecognised clear token type is a 400, as the contract says

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | tiny: about 10 lines of code plus tests | T-1114 |

**Read only these spec sections:** S7 section 5.5 and the swipe request row (`docs/specs/S7-api-contract.md:390`). Nothing else is needed.

**Why this task exists (verified 11 Oct 2026 from `main`; review finding N1 of PR #20).** S7 says: "`classification_token` is required. If its clear type field is not `classification`, the server returns `400 invalid_request`. Any other failure to open it (an earlier session, expiry or tampering) is not an error." The code does not do that for an unrecognised type name:
- `backend/crates/api/src/sealed.rs:177`: `let claimed = wire_to_type(parts[1]).ok_or(TokenError::Malformed)?;` returns `Malformed` when the clear type string is not a known wire name.
- `backend/crates/api/src/services/swipe.rs:346`: `Err(_) => Ok(())` treats every error other than `WrongType` and `Unavailable` as "proceed". So a token with a clear type of `foo` is silently accepted; the contract requires `400 invalid_request` (the same response as a known token of another type, `swipe.rs:337-340`).
No privilege is gained (the action is driven by fresh provider headers, not the token), so this is a contract-conformance fix, not a vulnerability.

## Goal

A well-formed token (prefix and four parts) whose clear type is anything other than `classification` yields `400 invalid_request` with the field `classification_token`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/sealed.rs` | line 177: an unknown clear type name returns `TokenError::WrongType` (only after the prefix and part-count checks of lines 172 to 175 pass) |
| Change | `backend/crates/api/tests/classify.rs` (or the swipe test file that covers S7 5.5) | add the cases below |

## Algorithm

1. In `open`, map an unknown wire name to `WrongType` instead of `Malformed`. Leave the prefix check, the part-count check, the expiry parse and the AES-GCM failure handling exactly as they are (those still proceed silently).
2. Do not change `swipe.rs`; its existing `WrongType` arm already returns `400 invalid_request`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-05 | A token whose clear type is not `classification` (known other type or an unknown name) gets `400 invalid_request` naming `classification_token` |
| SW-05 | A token that is expired, from an earlier session, tampered inside the ciphertext or structurally malformed (wrong prefix or part count) still proceeds with no `classifier_eval` record |

## Tests that must pass

- A swipe with a token of the form `<prefix>.foo.<exp>.<ciphertext>` returns `400 invalid_request`; with a known other type it still returns `400`; with a tampered ciphertext, an expired token and a wrong prefix it returns the normal success and writes no eval record.
- Existing `bake_5_sealed_token_carries_both` and the swipe tests stay green.

## Edge cases and traps

- Ordering matters: the 400 applies only after the prefix and part-count checks, so garbage that is not token-shaped keeps the "proceed" behaviour.
- Do not log the token or its parts.

## Out of scope

- Re-citing which test proves the other BAKE-5 behaviours (review finding N2); T-1115b.

## Done when

- The tests above pass and every required check is green (S10 10.1); Definition of done in S10 10.4.
