# T-1122: `blockers.py`: move into the repo, make it testable, test it

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: one module, one test file | none |

**Read only these:** the reference implementation below; `scripts/mailtinder_pick.py` functions it uses are on the factory box, not in this repo, so the module takes them as injected inputs (see Files). Nothing else is needed.

## Why this task exists (verified 13 Oct 2026, AAR 3.134)
A finished task (T-907) sat on a side branch with no PR for six days and blocked four others, and the hourly watch reported "waiting for a merge" without finding out why. `blockers.py` answers "what is the single root blocker of every unfinished task" and flags three defects. A live v1 runs on the factory box. Its first run flagged a FALSE defect (it counted one running builder as zero), so the checker itself needs tests.

## Goal
`tools/factory/blockers.py` with the row logic as pure functions over plain data, plus tests that pin every row kind and every defect kind. The box script becomes a thin wrapper that gathers the data and calls the module.

## Files
| Action | Path | What |
| --- | --- | --- |
| Create | `tools/factory/blockers.py` | `classify(tasks, merged, done, pr_by_task, allow, builders, gate_for) -> list[Row]` (pure) and `main()` that gathers inputs from the box (picker, `pr_state.py`, `gh`, `pgrep`) |
| Create | `tools/test_blockers.py` | Unit tests with a fake task graph |

## Algorithm
Port the reference implementation below. `Row = (task, text, kind)`, kind in `wait | owner | ok | DEFECT`. Order of checks per unfinished task (a merged task is skipped): (1) built (in `done`) with a PR: `PR #n <stage>`; built with no PR: DEFECT "ORPHAN"; (2) unmerged dependencies: `needs T-x (PR #n <stage> | not built | ORPHAN)`; (3) gated: `gate <kind>`, kind `owner`; (4) buildable but not in ALLOW: DEFECT "NOT IN ALLOW"; (5) buildable and in ALLOW with 0 builders: DEFECT "IDLE"; with builders: `ok`. Exit status 1 if any DEFECT.

## Tests that must pass
1. Orphan: T-907 built, no PR, dependents unmerged: T-907 is DEFECT ORPHAN and each dependent says `needs T-907 (ORPHAN)`. (This is the 6 Oct replay.)
2. Not in allow: a buildable task absent from ALLOW is DEFECT NOT IN ALLOW.
3. Idle: buildable, in ALLOW, `builders = 0` is DEFECT IDLE; **`builders = 1` is `ok`** (the v1 bug: one builder was read as zero).
4. A gated buildable task is `owner`, not a defect; a task with an open PR in any stage is `wait`.
5. Exit code 1 with a DEFECT, 0 without; output lines for defects start with `!!`.
6. `main()` handles the box tools failing (missing `gh`, a `pgrep` that errors) by printing `!! MISSING <tool>` and exiting 1, never by silently producing an empty table. An *empty* `pgrep` (exit 1, no match) is not a failure: it is zero builders and must reach the table as IDLE (see test 3).

## Edge cases and traps
- Counting builders: lines of `pgrep -af run-task.sh` that contain `bash`, excluding the `timeout` wrapper and the `pgrep` line itself.
- Task ids look like `T-1120b`; sort with the same key the picker uses (injected).
- Read-only: never writes, never calls an LLM, finishes in under 20 s.

## Out of scope
`tick.sh` (T-1123), the orphan watcher and the digest column (T-1124). Installing the module on the box is a manual `cp` by the on-call engineer after merge.

## Done when
The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.

## Reference implementation (v1, live on the box 13 Oct 2026)
```python
#!/usr/bin/env python3
"""blockers.py: one row per unfinished task with its single root blocker (AAR 3.134, item 2).

Read-only. Run from anywhere; it works inside /home/ubuntu/MailTinder-factory (the picker needs a git checkout).
Exit 0 = every row is explained by something that is working or waiting on a named human/PR.
Exit 1 = a DEFECT row exists: ORPHAN (built, no PR), NOT-IN-ALLOW (buildable but the lane will skip it),
         or IDLE (buildable, in ALLOW, nothing running). Those are the stalls the T-907 incident hid.
"""
import importlib.util, json, os, re, subprocess, sys

BASE = os.path.expanduser("~/.hermes/profiles/portal-affordable")
REPO = "/home/ubuntu/MailTinder-factory"
GH = "difficultnerd/MailTinder"


def sh(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, cwd=REPO).stdout


def load_picker():
    sys.argv = ["x"]
    spec = importlib.util.spec_from_file_location("pick", f"{BASE}/scripts/mailtinder_pick.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def allow_list():
    text = open(f"{BASE}/scripts/factory/scheduled-run-lane2.sh").read()
    m = re.search(r'^ALLOW="\$\{LANE2_TASKS:-(.*)\}"$', text, re.M)
    return set(m.group(1).split()) if m else set()


def pr_stages():
    """task id -> (pr number, stage) from pr_state.py (its own labels), plus open PR numbers by branch."""
    out = sh(f"python3 {BASE}/scripts/pr_state.py 2>/dev/null")
    stages = {}
    for line in out.splitlines():
        m = re.match(r"#(\d+) (\S+) (\S+) ", line)
        if m:
            stages[m.group(2)] = (int(m.group(1)), m.group(3))
    return stages


def running_builders():
    out = subprocess.run("pgrep -af 'run-task.sh'", shell=True, capture_output=True, text=True).stdout
    return len([l for l in out.splitlines() if "pgrep" not in l and "timeout" not in l and "bash" in l])


def main():
    os.chdir(REPO)
    sh("git fetch -q 2>/dev/null")
    m = load_picker()
    tasks, merged, done = m.task_index(), m.done_set(merged_only=True), m.done_set()
    allow, stages, busy = allow_list(), pr_stages(), running_builders()
    rows, defect = [], False
    for tid in sorted(tasks, key=m.sort_key):
        if tid in merged:
            continue
        t = tasks[tid]
        gate, why = m.gate_for(tid, t)
        pr = stages.get(f"factory/{tid}")
        unmet = [d for d in t["deps"] if d not in merged]
        if tid in done:  # built somewhere, not merged
            if pr:
                rows.append((tid, f"PR #{pr[0]} {pr[1]}", "wait"))
            else:
                rows.append((tid, "ORPHAN: built on a branch, NO PR (open one)", "DEFECT")); defect = True
        elif unmet:
            parts = []
            for d in unmet:
                p = stages.get(f"factory/{d}")
                parts.append(f"{d}" + (f" (PR #{p[0]} {p[1]})" if p else " (not built)" if d not in done else " (ORPHAN)"))
            rows.append((tid, "needs " + ", ".join(parts), "wait"))
        elif gate:
            rows.append((tid, f"gate {gate}: owner must lift", "owner"))
        elif tid not in allow:
            rows.append((tid, "buildable but NOT IN ALLOW (lane skips it): allow-add.sh " + tid, "DEFECT")); defect = True
        elif busy:
            rows.append((tid, f"buildable; {busy} builder(s) running", "ok"))
        else:
            rows.append((tid, "buildable, in ALLOW, NOTHING RUNNING", "DEFECT")); defect = True
    for tid, text, kind in rows:
        print(f"{'!!' if kind == 'DEFECT' else '  '} {tid:8} {text}")
    print(f"-- {len(rows)} unfinished, merged {len([t for t in tasks if t in merged])}/{len(tasks)}; defects: {sum(1 for r in rows if r[2]=='DEFECT')}")
    return 1 if defect else 0


if __name__ == "__main__":
    sys.exit(main())
```
