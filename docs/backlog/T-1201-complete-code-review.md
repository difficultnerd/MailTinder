# T-1201: Complete independent code review before release

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M12 | strong | about 9 reviewer batches plus a findings register | None (owner-initiated) |

**Read only these spec sections:** `docs/security/asvs-l2-register.md` (the whole register — it is the standard being tested), the S-series specs named per batch below, and `docs/backlog/CONVENTIONS.md`. Nothing else is needed.

## Why this exists

The per-pull-request security review loop started on 2026-10-06 at 05:32. Measured at that time:

- 157 commits on `main`
- **148 of them landed BEFORE any review loop existed**
- 9 had had an independent review

So roughly 94% of the product — M0-M3 (domain, ports, KMS and sealed envelopes), the M4 Gmail adapter, all of M5/M6, and the entire Flutter app — has never been looked at by anything except the model that wrote it.

This task exists so that a *release* claim is backed by a whole-codebase review rather than by an accident of when the review loop happened to be switched on.

## The standard: different models review each other's code

The owner's objective is explicit — **the reviewer must not be the model that wrote the code.**

| Who | Model | Evidence |
| --- | --- | --- |
| Builders (5 tracks) | `deepseek/deepseek-v4.1-flash` | pinned in the cron job definitions |
| PR reviewer | Claude Code CLI, `--model sonnet` | `scripts/claude-pr-review.sh` |

DeepSeek's code is therefore already reviewed cross-vendor. **The problem is the Claude-authored slice, and it is wider than the app.**

Two distinct sets of Claude-authored material exist:

1. **Product code** — 15 commits carry a `Co-Authored-By: Claude` trailer: the Flutter app (T-1006a..T-1010) and the S11 operations spec.
2. **The standards themselves** — 4 commits are authored as `Claude` and they add **the planning specs, the ASVS register, the backlog and CONVENTIONS** (`04aa7f6`, `29863ff`, `b90d755`, `1c3d9ea`), plus an S7 roles sync.

Set 2 is the more dangerous one. Those documents are the *yardstick* every batch is measured against. A missing or wrong control in S6, or a register row that overstates coverage, is invisible to every batch that uses it as its standard — and a Claude reviewer reading a Claude-written control is not a check at all.

**So both sets must be reviewed by something that is not Claude** — a different model family, or a human.

### Attribution rule

**Key on the author name `Claude`, OR a `Co-Authored-By: Claude` trailer.** Do not key on `Claude-Session:`: it appears on only 4 commits and none of the app or S11 commits carry it, so it would miss almost everything.

**Historical authorship is otherwise NOT reliably recoverable.** 134 commits on `main` are authored as the repository owner with no model trailer, the branch structure that recorded the track has been merged away, and builder job models were changed during the build (the main driver ran ChatGPT-family models before being repinned to DeepSeek). Do not attempt archaeology. The workable rule is: review everything with a non-author model, and use the author/trailer rule above only to identify the slice that must NOT be reviewed by Claude.

## Method — review by spec area, not by commit

A per-commit re-run of the existing reviewer would produce ~148 overlapping reports and would structurally miss cross-cutting defects: the same retry logic implemented five different ways, a control correct in each file but absent at the seams, or a register row marked `Verified` on the strength of an unrelated test.

Instead, freeze a tag at code complete, then run **one review batch per area**, each given the relevant spec as its standard.

**The reviewer for each batch must be named, and must not be the author of the artefact under review.** A batch whose subject is DeepSeek-written code may be reviewed by Claude. A batch whose subject is Claude-written (the app, S11, and the standards) must be reviewed by a third model or a human.

| Batch | Area | Spec | Reviewer |
| --- | --- | --- | --- |
| 1 | Domain model and invariants | S3 | Claude |
| 2 | Ports, traits and error types | S3, CONVENTIONS | Claude |
| 3 | Crypto: KMS, sealed envelopes, `data_key` handling | S6 | Claude |
| 4 | Authentication, sessions, step-up, invites | S6, S7 | Claude |
| 5 | API surface and contract conformance | S7 | Claude |
| 6 | Provider adapters (Gmail, AppFolderStore) and egress/unsubscribe SSRF | S6 T3, S8 | Claude |
| 7 | Flutter app | S9 | **NOT Claude** (Claude-authored) |
| 8 | CI, workflow and infrastructure scripts | S10, S11 | **NOT Claude** for S11; Claude may take S10 |
| 9 | **The standards: S3/S6/S7/S10 specs, the ASVS register, CONVENTIONS** | itself | **NOT Claude** (Claude-authored) |

Each batch must:

1. **Challenge every ASVS register row in its scope** — especially `Planned` and `unverified` ones — and either cite evidence (file:line plus the test that exercises it) or downgrade the claim.
2. Report findings as a table: ID, severity, the defect, `file:line`, why it matters, and a concrete suggested fix. (Same shape as the existing PR reviews, which is proven to be actionable.)
3. **Not** assert verification of its own fixes.

## Deliverables

1. One deduplicated **findings register** with severities and owners — not nine separate reports. **It must record, for every batch, the reviewer model ID and the prompt or commit hash used**, so the cross-model claim is checkable from an artefact rather than being a self-report.
2. An updated `docs/security/asvs-l2-register.md` where every row traces to evidence a reviewer checked.
3. Fixes landed for High and Medium, with the Low/Info set explicitly accepted or deferred (recorded, not silently dropped).
4. A **re-review pass that verifies the fixes** — an independent check of each fix, because a fix nobody re-checks is a self-report. This closes the one real gap in the existing loop: `claude-pr-review.sh` skips any PR that already carries its marker, so a fix is currently never independently confirmed.

## Release gate

This task is not "complete" while the register still contains unverified L2 rows that the release depends on. The gate is:

- **zero open Critical/High/Medium findings**, and
- every deferred Low/Info finding **recorded with a named owner**, and
- **no register row marked `Verified` on an unstated basis** — each is either backed by a cited test or honestly downgraded, and
- the Claude-authored slice (app, S11, and the standards) reviewed by a non-Claude model or a human.

## Acceptance criteria

- Every batch above has a review artefact citing file:line evidence, with the reviewer model ID recorded.
- No file is claimed as reviewed without a named reviewer and the standard it was reviewed against.
- The release gate above is satisfied.

## Notes

- **This is owner-initiated, not buildable-by-the-fleet.** The picker surfaces it as a pause rather than handing it to a track — see `PROJECT_END_TASKS` in `scripts/mailtinder_pick.py`.
- Cost is small: ~9 reviewer batches.
- **Batches are NOT all runnable on the build fleet.** The build tracks are DeepSeek, so a DeepSeek track cannot review DeepSeek-written code. Batches 7 and 9, and the S11 part of batch 8, need a different model or a human.
- Cost of NOT doing it: the release claim rests on 9 reviewed commits out of 157.