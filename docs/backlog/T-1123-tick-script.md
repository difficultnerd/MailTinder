# T-1123: `tick.sh`: the hourly watch as one tested script

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: one shell script, one Python test harness | T-1122 |

**Read only these:** `docs/TICK-AND-BLOCKERS-SPEC.md` is on the factory box; its content is reproduced under "Sections" below. Nothing else is needed.

## Why this task exists (verified 13 Oct 2026, AAR 3.134)
The hourly watch is an English prompt. It named tools that do not exist where it said (`scripts/factory/lane-health`, `logs/rework-dispatch.log`), so those checks were silently skipped for hours, and it never proved why a lane was idle. A watch that can silently skip a check cannot be trusted to find a stall.

## Goal
`tools/factory/tick.sh` prints one fixed-layout report and **fails loudly**: any component it cannot find prints `!! MISSING <component>` and makes the exit status non-zero. No section can be skipped silently.

## Files
| Action | Path | What |
| --- | --- | --- |
| Create | `tools/factory/tick.sh` | The report; every external tool and path is a variable at the top |
| Create | `tools/test_tick.py` | Harness: runs `tick.sh` in a sandbox with fake tools on `PATH` |

## Sections (fixed order, fixed headers)
1. `CLOCK`: `date -u`; `claude --version`; DeepSeek balance against the `$1.00` floor; disk percent; count of `cloudflared` processes (must be 0); count of PAUSED crons.
2. `STAGING`: `GET <staging>/api/v1/healthz` and `GET <staging>/` must both be 200 (URL from a variable).
3. `PRS`: the output of `pr_state.py`, and a line `!! NOT IN PENDING #n` for every open owner-only PR (paths under `.github/`, `infra/terraform/`) missing from `PENDING.md`.
4. `BLOCKERS`: the output of `tools/factory/blockers.py` (T-1122); its exit status is carried into `tick.sh`'s.
5. `LANES`: builders, reviews and reworks running (`pgrep`), and the rework slot.
6. `INBOX`: newest file name, and `NEW` if it differs from the stored marker (the marker is updated only after a successful run).
7. `COUNT`: `merged N/M` (MERGED only); `built-not-merged K` shown separately in brackets.

## Behaviours that must hold
- A missing tool, an unreadable path, a failed HTTP call or a tool exiting non-zero prints `!! MISSING <name>` or `!! FAILED <name>` in that section; the script still runs the other sections, then exits 1.
- Output lines are stable (golden file), so a diff between ticks is meaningful.
- Read-only except the inbox marker; no network calls other than the two staging GETs and `gh`; no secrets read.

## Tests that must pass
1. All fake tools healthy: exit 0, output equals the golden file.
2. For each section, remove its tool (one at a time): that section prints `!! MISSING <tool>`, the others still print, exit 1.
3. `cloudflared` count 1: `!! CLOUDFLARED RUNNING` and exit 1.
4. An owner-only PR missing from `PENDING.md`: `!! NOT IN PENDING #n`.
5. `blockers.py` exiting 1 makes `tick.sh` exit 1 and its `!!` lines appear under `BLOCKERS`.
6. Staging `/api/v1/healthz` returning 404: `!! FAILED staging healthz`.

## Out of scope
Auto-creating PENDING stubs and opening PRs for orphans (T-1124); changing any cron (the on-call engineer swaps the cron prompt for "run tick.sh" after merge).

## Done when
The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
