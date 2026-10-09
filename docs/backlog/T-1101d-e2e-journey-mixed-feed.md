# T-1101d: End-to-end journey: mixed Feed across two mailboxes

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 100 to 150 lines plus tests | T-1101b |

**Read only these spec sections:** S10 sections 3.3 (journey list), 6.2 (one-click route group), 6.3 (rows "Reject queues job", "Undo before due time", "Every outcome recorded") and 8 (events per journey) (`docs/specs/S10-test-strategy.md`); S2 AU-07 AC1, FD-02 AC1 and AC2, SW-04 AC2, SW-05 AC2, UN-01 AC1 and AC3, UN-02 AC1. Nothing else is needed.

**Split from T-1101b (2026-10-09):** the original task bundled five journeys and failed repeatedly on the per-run budget; each journey is now its own task so they can be built in parallel. Helpers in `backend/crates/e2e/src/lib.rs` come from T-1101b.

## Goal

One browser journey on the T-1101a harness: a Feed that interleaves two connected mailboxes by received time, each card showing its mailbox.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/mixed_feed.rs` | Journey 3 |

## Types and signatures

Use the helpers from T-1101b (`Testbed`, `EventLog`, `signed_in_user`); add a helper to `lib.rs` only if the journey needs one, keeping existing signatures.

## Algorithm

All tests are `#[ignore = "run by scripts/e2e.sh"]`, use `example.com` accounts and corpus fixtures from T-204 only.

1. **Journey 3, mixed Feed** (FD-02 AC1, AC2): seed mailbox A with fixtures at times t1 and t3 and mailbox B at t2 (fake-google control). Sign in with A; Settings, Connected accounts, "Add Gmail"; Confirm it's you appears: switch to the popup, fake-google re-authenticates A; back in the main window the link starts; fake-google selects B; return to Connected accounts. Go to the Feed and read three cards in turn by tapping "Keep": order t3 (A), t2 (B), t1 (A), each showing its mailbox address.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-02 AC1 | The Feed interleaves two mailboxes by received time, newest first |
| FD-02 AC2 | Each card shows its mailbox |

## Tests that must pass

- `fd_02_ac1_e2e_two_mailboxes_interleaved_newest_first` (end to end; also asserts FD-02 AC2 badges, name `fd_02_ac2_e2e_cards_show_mailbox` as a second test in the same file)

## Edge cases and traps

- Advance the virtual clock through `TestControl`; never `sleep` for minutes. Poll with a short timeout only for asynchronous delivery.
- The testbed listens on loopback; `unsub` runs with the test egress policy that allows exactly the testbed socket (S10 6.2). Do not loosen the production policy.
- Each journey seeds its own accounts with unique `sub` values so tests do not see each other's mail.
- Read events only after the mark, so earlier journeys' lines do not count.
- No real hosts, no real mail.
- The Add Gmail flow uses a popup window; switch to it, finish the fake-google re-authentication, switch back; allow the app time to start the link.

## Out of scope

- All other journeys (T-1101b, T-1101e, T-1101f).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
