# T-1114: Emit the `swipe`, `undo`, `undo_failed`, `unsub_after_undo` and `history_missing` metric events

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 200 lines of code plus tests | T-307, T-1101b |

**Read only these spec sections:** S10 section 8 (`docs/specs/S10-test-strategy.md`), S11 alert A1 (`docs/specs/S11-operations.md`), ADR 0002 (`docs/decisions/0002-e2e-swipe-undo-events.md`), `backend/crates/obs/src/registry.rs`. Nothing else is needed.

## Goal

The five event types already in the obs allowlist are actually emitted, so the undo-rate and "unrecoverable actions" measures (S10 8) and the S11 A1 alert and T-1107 queries have data. Today only `unsub_outcome` and `delivery_check_outcome` are emitted.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/services/swipe.rs` (or the module that handles `/api/v1/swipes`) | emit `swipe` with the action only |
| Change | `backend/crates/api/src/services/undo.rs` | emit `undo` (action only); `undo_failed` when the exact previous state was not restored |
| Change | `backend/crates/unsub/src/runner.rs` and `backend/crates/worker/src/sweep.rs` | `unsub_after_undo` if a send follows a successful undo; `history_missing` if an automated trash has no History entry |
| Change | `backend/crates/e2e/tests/reject_undo.rs`, `reject_unsubscribe.rs`, `file.rs` | restore the `swipe` / `undo` event assertions dropped by ADR 0002 |

## Algorithm

1. Use the existing metric-event API from T-307; fields carry no mail content, only event type and action or outcome.
2. Emit at the point the state change commits, not before.
3. Unit-test each emitter; extend the three e2e journeys with the exact events (journey 4: one `swipe` reject, one `undo`, no `unsub_outcome`; journey 5: one `swipe`, one `unsub_outcome` `sent`; journey 6: one `swipe`).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| OBS-EV AC1 | A swipe emits exactly one `swipe` event carrying the action and no content |
| OBS-EV AC2 | An undo emits exactly one `undo`; a failed restore emits `undo_failed` |
| OBS-EV AC3 | `unsub_after_undo` and `history_missing` are emitted in their defined situations |

## Tests that must pass

- `obs_ev_ac1_swipe_emits_one_event` (unit, `api`)
- `obs_ev_ac2_undo_emits_event_and_failure_emits_undo_failed` (unit, `api`)
- `obs_ev_ac3_unsub_after_undo_and_history_missing_emitted` (unit, `unsub` and `worker`)
- the three restored e2e journeys (end to end)

## Edge cases and traps

- Events must never carry subject, sender, body or addresses (redaction tests in `obs` must stay green).
- Do not emit before the state change commits.

## Out of scope

- Dashboards and alert wiring (T-1107).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
