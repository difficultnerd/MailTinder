# T-1101f: End-to-end journey: file a message under a new category

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 100 to 150 lines plus tests | T-1101b |

**Read only these spec sections:** S10 sections 3.3 (journey list), 6.2 (one-click route group), 6.3 (rows "Reject queues job", "Undo before due time", "Every outcome recorded") and 8 (events per journey) (`docs/specs/S10-test-strategy.md`); S2 AU-07 AC1, FD-02 AC1 and AC2, SW-04 AC2, SW-05 AC2, UN-01 AC1 and AC3, UN-02 AC1. Nothing else is needed.

**Split from T-1101b (2026-10-09):** the original task bundled five journeys and failed repeatedly on the per-run budget; each journey is now its own task so they can be built in parallel. Helpers in `backend/crates/e2e/src/lib.rs` come from T-1101b.

## Goal

One browser journey: tapping File, creating the category Receipts and confirming applies the label and removes the message from the inbox.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/file.rs` | Journey 6 |

## Types and signatures

Use the helpers from T-1101b (`Testbed`, `EventLog`, `signed_in_user`); add a helper to `lib.rs` only if the journey needs one, keeping existing signatures.

## Algorithm

All tests are `#[ignore = "run by scripts/e2e.sh"]`, use `example.com` accounts and corpus fixtures from T-204 only.

1. **Journey 6, file** (SW-04 AC2): seed one message; tap "File"; tap "New category", type "Receipts", confirm; wait for "Filed under Receipts."; fake-google shows a label named "Receipts" on the message and no INBOX label.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-04 AC2 | Filing applies the label and the message leaves the inbox |

## Tests that must pass

- `sw_04_ac2_e2e_file_applies_label_and_leaves_inbox` (end to end)

## Edge cases and traps

- Advance the virtual clock through `TestControl`; never `sleep` for minutes. Poll with a short timeout only for asynchronous delivery.
- The testbed listens on loopback; `unsub` runs with the test egress policy that allows exactly the testbed socket (S10 6.2). Do not loosen the production policy.
- Each journey seeds its own accounts with unique `sub` values so tests do not see each other's mail.
- Read events only after the mark, so earlier journeys' lines do not count.
- No real hosts, no real mail.
- Wait for the confirmation text "Filed under Receipts."; the field is a web text input, type into it through the harness's native input helper.

## Out of scope

- Other journeys (T-1101b, T-1101d, T-1101e); `swipe` event assertion (T-1114).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
