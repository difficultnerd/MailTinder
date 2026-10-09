# T-1101c: End-to-end journeys: Needs Attention, disconnect, delete account; leak, storage and CSP scans

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 400 lines of code plus tests | T-006, T-1005, T-1101b, T-1101d, T-1101e, T-1101f |

**Read only these spec sections:** S10 sections 3.3, 6.3 (rows "Disconnect cancels", "Account deletion order"), 7.2 (row "CSP compatibility") and 7.3 (`docs/specs/S10-test-strategy.md`); S5 "Logs and telemetry", "Browser" and the LOG-1 row of "Deletion tests" (`docs/specs/S5-data-inventory.md`); S2 NA-01 AC2, AU-05 AC1, AU-06 AC1, FD-01 AC3, XC-01; S3 INV-1. Nothing else is needed.

## Goal

The last three journeys (resolve a Needs Attention item, disconnect a mailbox, delete the account) and the scans that run across every journey: logs, Firestore and browser storage hold no corpus canary, and the app runs under the shipped CSP with no violation.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/needs_attention.rs` | Journey 7 |
| Create | `backend/crates/e2e/tests/disconnect.rs` | Journey 8 |
| Create | `backend/crates/e2e/tests/delete_account.rs` | Journey 9 |
| Create | `backend/crates/e2e/tests/zz_leak_scan.rs` | Scans, run in a second `cargo test` call after all journeys |
| Change | `backend/crates/e2e/src/lib.rs` | `Canaries`, `FirestoreDump`, `BrowserDumps`, `assert_no_csp_violation` |
| Change | `scripts/e2e.sh` | After journeys, run `cargo test -p e2e --test zz_leak_scan -- --ignored` |

## Types and signatures

```rust
pub struct Canaries { pub strings: Vec<String> } // every CANARY-*, corpus address and URL from the T-204 manifest
impl Canaries { pub fn load() -> Result<Self, E2eError>; pub fn find_in(&self, text: &str) -> Vec<String>; }

pub struct FirestoreDump;  // lists every document of every S5 collection through the emulator REST API
impl FirestoreDump { pub async fn all_documents(emulator: &Url, project: &str) -> Result<Vec<(String, String)>, E2eError>; } // (path, json)

/// Each journey writes its browser storage dump to target/e2e-logs/browser-<journey>.json before closing.
pub async fn save_browser_dump(ui: &Ui, journey: &str) -> Result<(), E2eError>;
/// Fails if Chrome's log has any "Content Security Policy" or "TrustedHTML"/"TrustedScript" message.
pub async fn assert_no_csp_violation(ui: &Ui) -> Result<(), E2eError>;
```

## Algorithm

1. **Journey 7, Needs Attention** (NA-01 AC2): seed an https-only list message (DKIM covers `List-Unsubscribe`, no one-click). Reject it; open the Needs Attention tab; the item shows "This sender needs you to unsubscribe on their website."; tap "Done"; the item disappears and the Firestore emulator has no `needs_attention` document for the user. Assert the testbed received nothing (v1 never fetches the link).
2. **Journey 8, disconnect** (AU-05 AC1): user with two mailboxes (reuse the journey 3 helper) and one queued unsubscribe job for mailbox B (reject a one-click card from B, do not advance the clock). Settings, Connected accounts, Disconnect B, confirm, Confirm it's you through the popup. Then: fake-google recorded a revoke for B's token; no `jobs` document for B remains; after `advance_clock(6 minutes)` the testbed received nothing; B's cards are gone from the Feed.
3. **Journey 9, delete account** (AU-06 AC1): Settings, Account, delete with both confirmations and the popup step-up. Then fake-google's recorded call order is: app folder file delete, then token revoke; `jobs` hold nothing for the user; the browser is back on Sign-in; after `advance_clock(25 hours)` and a sweep (`TestControl`), no Firestore document references the user ID.
4. Every journey in T-1101b, T-1101d, T-1101e and T-1101f ends with `finish_journey` (T-1101b's helper that calls `save_browser_dump` and `assert_no_csp_violation`); check that all of them do, and add the call to any journey that lacks it. Journeys 7 to 9 call it too.
5. **Leak scan** (`zz_leak_scan.rs`, after all journeys):
   - Logs (XC-01, LOG-1): every line of `target/e2e-logs/*.jsonl` has no canary, corpus address or unsubscribe URL.
   - Firestore (INV-1, FD-01 AC3): every document from `FirestoreDump` has no canary in plaintext. Encrypted fields hold ciphertext, so a plaintext match is a failure wherever it appears.
   - Browser (ASVS V14.3.3): every `browser-*.json` dump has no canary and no key other than ones Flutter itself writes with no card data (list the allowed keys explicitly; start with none).
6. **CSP** (ASVS V3.4.3): the host serves the CSP from `firebase.json` (T-006); `assert_no_csp_violation` after each journey proves the app loads and completes a swipe under it with Trusted Types.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| NA-01 AC2 | Marking an item done deletes it |
| AU-05 AC1 | Disconnect revokes the token, cancels queued jobs and removes the mailbox's cards |
| AU-06 AC1 | Deletion runs app folder delete then token revoke, and every server record is swept within 24 hours |
| XC-01 | No canary, address or URL appears in service logs |
| FD-01 AC3 | No message content reaches the server database while building cards |
| INV-1 | No Firestore document holds a body, subject or snippet |
| LOG-1 | Integration logs contain no value from the fixture corpus |
| ASVS V14.3.3 | Browser storage holds no card data after every journey |
| ASVS V3.4.3 | The app completes a swipe under the shipped CSP with Trusted Types and no violation |

## Tests that must pass

- `na_01_ac2_e2e_done_deletes_item` (end to end)
- `au_05_ac1_e2e_disconnect_revokes_and_cancels_jobs` (end to end)
- `au_06_ac1_e2e_delete_account_order_and_sweep` (end to end)
- `xc_01_e2e_logs_hold_no_canary` (end to end scan)
- `log_1_e2e_logs_hold_no_corpus_value` (end to end scan)
- `fd_01_ac3_e2e_firestore_holds_no_card_content` (end to end scan)
- `inv_1_e2e_no_firestore_document_holds_mail_content` (end to end scan)
- `asvs_v14_3_3_e2e_browser_storage_holds_no_card_data` (end to end scan)
- `asvs_v3_4_3_e2e_swipe_completes_under_shipped_csp` (end to end)

## Edge cases and traps

- The scan must fail loudly if a log file or dump is missing (a missing file is not a pass).
- Scan the plaintext canaries only; do not try to decrypt encrypted fields.
- `zz_leak_scan.rs` runs in its own `cargo test` call after the journeys; do not rely on test ordering within one call.
- Do not add an allowlist entry to make a scan pass; fix the leak.
- The account deletion check uses the virtual clock and a sweep call; never wait 24 real hours.

## Out of scope

- Bake-off journeys and the EXP-1 `classifier_eval` scan (follow-up after T-906, recommended as its own task).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The full `scripts/e2e.sh` run stays under 10 minutes on a GitHub-hosted runner, or the PR records the time and asks James.
