# T-1201: Complete independent code review before release

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M12 | strong | about 8 reviewer batches plus a findings register | every other task |

**Read only these spec sections:** `docs/security/asvs-l2-register.md` (the whole register — it is the standard being tested), the S-series specs named per batch below, and `docs/backlog/CONVENTIONS.md`. Nothing else is needed.

## Why this exists

The per-pull-request security review loop started on 2026-10-06 at 05:32. Measured against the repository at that time:

- 157 commits on `main`
- **148 of them landed BEFORE any review loop existed**
- 9 have had an independent review

So roughly 94% of the product — M0-M3 (domain, ports, KMS and sealed envelopes), the M4 Gmail adapter, all of M5/M6, and the entire Flutter app — has never been looked at by anything except the model that wrote it.

This task exists so that a *release* claim is backed by a whole-codebase review rather than by an accident of when the review loop happened to be switched on.

## The standard: different models review each other's code

The owner's objective is explicit — **the reviewer must not be the model that wrote the code.** The current split already satisfies this:

- Builders: `deepseek/deepseek-v4.1-flash` (all five build tracks, pinned)
- Reviewer: Claude Code CLI, `--model sonnet`

So DeepSeek's code is already reviewed cross-vendor. The exception is code **Claude itself wrote** — the Flutter app range (T-1006a..T-1010, `claude-app-driver.sh`) and the S11 spec (`claude-s11-driver.sh`). That slice is identifiable, because the Claude CLI writes `Co-Authored-By: Claude … <noreply@anthropic.com>` and `Claude-Session:` trailers. It must be reviewed by something that is not Claude.

**Attribution note, stated plainly:** historical authorship is NOT reliably recoverable. 134 commits are authored as `difficultnerd <jnewburrie@gmail.com>` with no model trailer, the branch structure that recorded the track is merged away, and builder job models were changed during the build (the main driver ran ChatGPT-family models before being repinned to DeepSeek). Do not attempt archaeology. The workable rule is: review everything with a non-author model, and use the Claude trailers only to identify the slice that must NOT be reviewed by Claude.

## Method — review by spec area, not by commit

A per-commit re-run of the existing reviewer would produce ~148 overlapping reports and would structurally miss cross-cutting defects: the same retry logic implemented five different ways, a control correct in each file but absent at the seams, or a register row marked `Verified` on the strength of an unrelated test.

Instead, freeze a tag at code complete, then run **one review batch per area**, each given the relevant spec as its standard:

| Batch | Area | Spec |
| --- | --- | --- |
| 1 | Domain model and invariants | S3 |
| 2 | Ports, traits and error types | S3, CONVENTIONS |
| 3 | Crypto: KMS, sealed envelopes, `data_key` handling | S6 |
| 4 | Authentication, sessions, step-up, invites | S6, S7 |
| 5 | API surface and contract conformance | S7 |
| 6 | Provider adapters (Gmail, AppFolderStore) and egress/unsubscribe SSRF | S6 T3, S8 |
| 7 | Flutter app (Claude-authored — needs a NON-Claude reviewer) | S9 |
| 8 | CI, workflow and infrastructure scripts | S10, S11 |

Each batch must:

1. **Challenge every ASVS register row in its scope** — especially `Planned` and `unverified` ones — and either cite evidence (file:line plus the test that exercises it) or downgrade the claim.
2. Report findings as a table: ID, severity, the defect, `file:line`, why it matters, and a concrete suggested fix. (Same shape as the existing PR reviews, which is proven to be actionable.)
3. **Not** assert verification of its own fixes.

## Deliverables

1. One deduplicated **findings register** with severities and owners — not eight separate reports.
2. An updated `docs/security/asvs-l2-register.md` where every row traces to evidence a reviewer checked.
3. Fixes landed for High and Medium, with the Low/Info set explicitly accepted or deferred (recorded, not silently dropped).
4. A **re-review pass that verifies the fixes** — an independent check of each fix, because a fix nobody re-checks is a self-report. This closes the one real gap in the existing loop: `claude-pr-review.sh` skips any PR that already carries its marker, so a fix is currently never independently confirmed.

## Acceptance criteria

- Every batch above has a review artefact citing file:line evidence.
- No file is claimed as reviewed without a named reviewer and the standard it was reviewed against.
- Every ASVS row is `Verified` with a cited test, or honestly `Planned`/`unverified` — no row may be `Verified` on an unstated basis.
- The Claude-authored slice (app, S11) was reviewed by a non-Claude model or by a human.

## Notes

- **This is owner-initiated, not buildable-by-the-fleet.** The picker surfaces it as a pause rather than handing it to a track — see `PROJECT_END_TASKS` in `scripts/mailtinder_pick.py`.
- Cost is small: ~8 reviewer batches, parallelisable across the five build tracks.
- Cost of NOT doing it: the release claim rests on 9 reviewed commits out of 157.