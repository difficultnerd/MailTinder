# T-1101e: End-to-end journeys: reject then undo, reject and let the unsubscribe run

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 100 to 150 lines plus tests | T-1101b |

**Read only these spec sections:** S10 sections 3.3 (journey list), 6.2 (one-click route group), 6.3 (rows "Reject queues job", "Undo before due time", "Every outcome recorded") and 8 (events per journey) (`docs/specs/S10-test-strategy.md`); S2 AU-07 AC1, FD-02 AC1 and AC2, SW-04 AC2, SW-05 AC2, UN-01 AC1 and AC3, UN-02 AC1. Nothing else is needed.

**Split from T-1101b (2026-10-09):** the original task bundled five journeys and failed repeatedly on the per-run budget; each journey is now its own task so they can be built in parallel. Helpers in `backend/crates/e2e/src/lib.rs` come from T-1101b.

## Goal

Two browser journeys: rejecting a one-click list message and undoing before the due time sends nothing; rejecting and waiting sends exactly one correct one-click POST, collected into History.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/reject_undo.rs` | Journey 4 |
| Create | `backend/crates/e2e/tests/reject_unsubscribe.rs` | Journey 5 |

## Types and signatures

Use the helpers from T-1101b (`Testbed`, `EventLog`, `signed_in_user`); add a helper to `lib.rs` only if the journey needs one, keeping existing signatures.

## Algorithm

All tests are `#[ignore = "run by scripts/e2e.sh"]`, use `example.com` accounts and corpus fixtures from T-204 only.

3. **Journey 4, reject then undo** (SW-05 AC2): seed one one-click list message (DKIM covers both headers) pointing at the testbed's one-click 200 route. Tap "Reject"; wait for "Trashed. Unsubscribing in 5 minutes."; tap "Undo"; wait for the card again. `advance_clock(6 minutes)`; then the testbed has received zero requests on that route, and fake-google shows the message back in INBOX with its exact label set.
4. **Journey 5, reject and let it run** (UN-01 AC1, UN-02 AC1, UN-01 AC3): same seed; tap "Reject"; `advance_clock(6 minutes)`; poll the testbed up to 10 s: exactly one POST, body exactly `List-Unsubscribe=One-Click`, no `Cookie`, no `Authorization`. Pull to refresh the Feed (collects the outcome), open Settings, History, filter "Unsubscribes": one entry with outcome "Sent". Advance the clock again and refresh: still exactly one request (runs once).
6. **Events** (S10 8): each journey takes `EventLog::mark()` at start and asserts the events the services actually emit: journey 5 exactly one `unsub_outcome` with outcome `sent`; journey 4 no `unsub_outcome` (read the log only after the 6-minute clock advance and the testbed zero-request check, so the negative assertion cannot pass vacuously). Do NOT assert `swipe` or `undo` events: the services do not emit them (ADR 0002); the swipe, undo and filing effects are already proven by the testbed and fake-google assertions above. Event field names come from T-307; if they differ from `event_type` and `outcome`, use T-307's.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |


## Tests that must pass

- `sw_05_ac2_e2e_undo_before_due_sends_nothing` (end to end)
- `un_01_ac1_e2e_unsubscribe_runs_once` (end to end)
- `un_02_ac1_e2e_one_click_post_is_exact` (end to end)
- `un_01_ac3_e2e_outcome_in_history_after_feed_load` (end to end)

## Edge cases and traps

- Advance the virtual clock through `TestControl`; never `sleep` for minutes. Poll with a short timeout only for asynchronous delivery.
- The testbed listens on loopback; `unsub` runs with the test egress policy that allows exactly the testbed socket (S10 6.2). Do not loosen the production policy.
- Each journey seeds its own accounts with unique `sub` values so tests do not see each other's mail.
- Read events only after the mark, so earlier journeys' lines do not count.
- No real hosts, no real mail.
- First check that the `unsub` and `worker` e2e configurations (T-500b follow-up) let the unsubscribe reach the testbed over the test egress policy (loopback listener and any certificate trust it needs). If a missing piece is small and inside `backend/crates/e2e` and test configuration, add it; if it needs production code or a policy loosening, stop and report BLOCKED with the exact missing piece.

## Out of scope

- Journey 6 (T-1101f); `swipe`/`undo` event assertions (T-1114, ADR 0002).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
