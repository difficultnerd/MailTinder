# T-1108a: demo.sh: start, stop, status and an automatic check (loopback only)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 150 lines of code plus tests | T-500b, T-500c, T-1101a |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

One command, `scripts/demo.sh`, starts the e2e stack (Firestore emulator, fake-google, unsub-testbed, `api`, the Flutter web build via `scripts/e2e_host.py`), leaves it running on 127.0.0.1 and prints the URL; `stop` and `status` work; `--check` is an automatic test that starts, fetches the page and the health route, and stops cleanly.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/demo.sh` | `start` (default), `stop`, `status`, `--check` |
| Create | `scripts/e2e/lib.sh` | Shared start/stop functions moved out of `scripts/e2e.sh`; `e2e.sh` behaviour must not change |
| Change | `scripts/e2e.sh` | Use the shared functions |

## Behaviour

1. Everything binds 127.0.0.1 on free ports; `demo.sh` refuses any non-loopback host.
2. State is written to `target/demo/state.json`; `stop` kills every process group it started, even after a crash, and prints a leak census (nothing left).
3. `--check` finishes in under 3 minutes and leaves no process behind.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `demo_check_serves_page_and_health_then_cleans_up`
- `demo_refuses_non_loopback_bind`
- `e2e_sh_still_passes_unchanged` (its journey runs)

## Out of scope

Seeding (T-1108b), the phone tunnel (T-1108c), hot reload (T-1113).

## Done when

`scripts/demo.sh --check` output is in the PR body. Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
