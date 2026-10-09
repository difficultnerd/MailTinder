# ADR 0002: e2e journeys do not assert `swipe` / `undo` metric events

Status: accepted (factory decision, 2026-10-09; owner asleep, recorded for review)
Date: 9 October 2026

## Context

T-1101b step 6 asked journeys 4 to 6 to assert `swipe` and `undo` metric events. The coder blocked twice (14:08 UTC): the services emit only `unsub_outcome` and `delivery_check_outcome`, so those assertions cannot pass without api changes that T-1101b declares out of scope.

## Options (buy / borrow / build)

| Option | Verdict |
| --- | --- |
| Borrow: assert only the events that exist (`unsub_outcome`); swipe/undo effects are already proven end to end by the testbed and fake-google checks | **Chosen.** Smallest change, unblocks T-1101b and T-1101c, no production code touched |
| Build: add `swipe`/`undo` metric emission to the api, then keep the assertions | Deferred. Worth doing only if S10 section 8 telemetry is wanted for product reasons; separate task, own review |
| Buy: none applicable | n/a |

## Consequence

T-1101b step 6 is amended. Revisit if swipe/undo telemetry is added later (then restore the assertions in the same change).
