# T-1124: orphan-branch watcher, PENDING stubs, and blockers in the digest

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: two small scripts, one digest change, tests | T-1122 |

## Why this task exists (verified 13 Oct 2026, AAR 3.134)
T-907 was built on 6 Oct on a side branch and never got a PR; nobody noticed for six days. The owner-facing digest printed the four tasks it blocked as a bare hourglass. The owner's pending list is typed by hand and the hourly checker only complains when it drifts (four failures on 12 to 13 Oct).

## Goal
(a) A branch that holds finished task work but has no PR gets one automatically. (b) The owner's pending list never drifts from the open owner-only PRs. (c) Every waiting task in the digest shows its root blocker.

## Files
| Action | Path | What |
| --- | --- | --- |
| Create | `tools/factory/orphan_branch_pr.py` | Find and fix orphans (see Algorithm) |
| Create | `tools/factory/pending_sync.py` | Keep `PENDING.md` stubs in step with open owner-only PRs |
| Change | digest script (on the box; this task delivers the function `blocker_text(task)` and a patch file `tools/factory/owner-digest-blockers.patch`) | Replace bare hourglasses with `blockers.py` text |
| Create | `tools/test_orphan_branch_pr.py`, `tools/test_pending_sync.py` | Tests |

## Algorithm
1. **Orphan watcher.** List origin branches that (a) are named `factory/*` or `agent/*` (an allow-list of prefixes held in one variable), (b) have a newest commit whose author is on a short allow-list of identities (a variable), and (c) have a subject opening with a task id (`^T-\d+[a-z]?:`), are ahead of `main`, and have no open or merged PR for that branch. If the commit is older than 4 hours, open a PR **from that branch as it is** (no cherry-pick, no checkout of its contents, no execution of any of its code on the factory box) whose body says "recovered orphan; normal review and CI apply". **The watcher never runs the branch's tests or scripts**; CI on the PR does that in the sandbox. Print `!! ORPHAN OPENED <task>`. **Never merge.** Idempotent: a second run does nothing if the PR exists. Any branch outside the allow-lists, or whose commit touches `.github/`, `scripts/` or any file named in a protected-paths variable, is only REPORTED (`!! ORPHAN HELD <branch>: needs the owner`), never opened.
2. **Pending sync.** For each open PR touching `.github/` or `infra/terraform/` with no line mentioning `#<n>` in `PENDING.md`, append `A<next id> [soon] PR #<n> <title>`; for each line whose PR is merged or closed, remove it. Never edit any other line. Writes via `.new` plus rename; keeps a dated `.bak`.
3. **Digest.** `blocker_text(task)` returns the `blockers.py` text for a task; the digest prints it after the hourglass.

## Tests that must pass
1. A fake repo with a 5-hour-old `T-907:` commit by an allow-listed identity on `agent/parallel-2`, no PR: the watcher opens exactly one PR from that branch; a second run opens none.
2. A 1-hour-old orphan: nothing happens.
3. A branch named `evil/x` or a commit by a non-listed identity: held and reported, no PR.
4. A commit touching `.github/workflows/x.yml` or `scripts/x.sh`: held and reported, no PR.
5. The watcher never executes any file from the branch (a canary script in the fake branch must not run) and never calls a merge.
6. `pending_sync`: an unlisted owner-only PR gains one stub; a merged PR's line is removed; unrelated lines are byte-identical.
7. `blocker_text("T-906b")` with a fake graph returns the dependency text.

## Edge cases and traps
- Subjects that open with a task id on a documentation-only commit are not orphans (the picker has the same rule).
- A branch that already has a PR, open or merged, is never touched.
- Do not touch `.github/` or any `.tf` file (owner-only).
- Found by the independent review of PR #110 (N-01, High): the first draft cherry-picked any branch whose subject began with a task id and ran its tests on the factory box. A branch name and a commit subject are attacker-controlled text; never execute branch content outside CI.

## Out of scope
Making the picker treat "done" as "merged" (a picker change that needs the owner's decision, AAR 3.134 item 1); merging anything.

## Done when
The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
