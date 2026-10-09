# ADR 0002: e2e journeys do not assert `swipe` / `undo` metric events

Status: proposed (factory decision 2026-10-09 while the owner was asleep; needs the owner to ratify)
Date: 9 October 2026

## Context

T-1101b step 6 asked journeys 4 to 6 to assert `swipe` and `undo` metric events. The coder blocked twice (14:08 UTC): the services emit only `unsub_outcome` and `delivery_check_outcome`, so those assertions cannot pass without api changes that T-1101b declares out of scope.

## Options (buy / borrow / build)

| Option | Verdict |
| --- | --- |
| Borrow: assert only the events that exist (`unsub_outcome`); swipe/undo effects are already proven end to end by the testbed and fake-google checks | **Chosen.** Smallest change, unblocks T-1101b and T-1101c, no production code touched |
| Build: add `swipe`/`undo` metric emission to the api, then keep the assertions | **Required follow-up: T-1114.** Until it merges, the undo-rate measure and the S11 alert A1 (`undo_failed`, `unsub_after_undo`, `history_missing`) have no data, so A1 can never fire. T-1114 restores the dropped assertions |
| Buy: none applicable | n/a |

## Consequence

The allowlist in `backend/crates/obs/src/registry.rs` names events that nothing emits yet; that gap is real and is owned by T-1114. Ratification: the owner confirms this ADR (and the order T-1101b, then T-1114) before the trial relies on A1.

T-1101b step 6 is amended. Revisit if swipe/undo telemetry is added later (then restore the assertions in the same change).
