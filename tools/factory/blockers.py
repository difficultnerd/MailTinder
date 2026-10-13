#!/usr/bin/env python3
"""blockers.py: one row per unfinished task with its single root blocker (AAR 3.134, item 2).

Read-only. Runs from anywhere; on the factory box it works inside
/home/ubuntu/MailTinder-factory (the picker needs a git checkout).

Exit 0 = every row is explained by something that is working or waiting on a
named human/PR.
Exit 1 = a DEFECT row exists: ORPHAN (built, no PR), NOT-IN-ALLOW (buildable but
the lane will skip it), or IDLE (buildable, in ALLOW, nothing running). Those
are the stalls the T-907 incident hid.

The decision table lives in `classify`, a pure function over plain data, so
tools/test_blockers.py pins every row kind and every defect kind without a git
checkout, a pull request or a running builder. `main` is the thin wrapper that
gathers the box inputs (the picker module, `pr_state.py`, `gh`, `pgrep`) and
calls it. Nothing here writes to the repo or to GitHub, calls a model, or takes
more than a few seconds.
"""

from __future__ import annotations

import importlib.util
import os
import re
import shutil
import subprocess
import sys
from typing import Any, Callable, NamedTuple

BASE = os.path.expanduser("~/.hermes/profiles/portal-affordable")
REPO = "/home/ubuntu/MailTinder-factory"

# Row kinds. DEFECT is the only one that makes the script exit 1.
WAIT = "wait"
OWNER = "owner"
OK = "ok"
DEFECT = "DEFECT"

ORPHAN_TEXT = "ORPHAN: built on a branch, NO PR (open one)"
NOT_IN_ALLOW_TEXT = "buildable but NOT IN ALLOW (lane skips it): allow-add.sh "
IDLE_TEXT = "buildable, in ALLOW, NOTHING RUNNING"


class Row(NamedTuple):
    """One unfinished task and its single root blocker."""

    task: str
    text: str
    kind: str  # wait | owner | ok | DEFECT


_TASK_ID_RE = re.compile(r"^([A-Za-z]+-)(\d+)([a-z]*)$")


def default_sort_key(task_id: str):
    """Fallback ordering for `T-1120b`-style ids (the picker's own key is injected when present).

    Numeric-aware so T-1120 < T-1120a < T-1120b < T-1121, which is the order the
    picker works in. Both branches return (str, int, str) so the tuples compare.
    """
    m = _TASK_ID_RE.match(task_id)
    if not m:
        return (task_id, 0, "")
    return (m.group(1), int(m.group(2)), m.group(3))


def _dep_text(dep: str, done: set[str], pr_by_task: dict) -> str:
    """`T-907 (PR #12 review)`, `T-907 (not built)` or `T-907 (ORPHAN)`."""
    pr = pr_by_task.get(dep)
    if pr:
        return f"{dep} (PR #{pr[0]} {pr[1]})"
    if dep not in done:
        return f"{dep} (not built)"
    return f"{dep} (ORPHAN)"


def classify(
    tasks: dict,
    merged: set[str],
    done: set[str],
    pr_by_task: dict,
    allow: set[str],
    builders: int,
    gate_for: Callable,
    sort_key: Callable | None = None,
) -> list[Row]:
    """Return one Row per unfinished task, in the picker's order (pure).

    tasks: task id -> {"deps": [task id, ...], ...}; merged tasks are skipped.
    done: task ids built somewhere (a built task may still be unmerged).
    pr_by_task: task id -> (pr number, stage) for every open PR; absent otherwise.
    allow: task ids the lane's ALLOW list permits.
    builders: running `run-task.sh` builders; the caller never passes an unknown
        count (an unknown count is a tool failure, not zero - see main).
    gate_for: (task id, task) -> (gate, why); a truthy gate is an owner gate.
    """
    key = sort_key or default_sort_key
    rows: list[Row] = []
    for tid in sorted(tasks, key=key):
        if tid in merged:
            continue
        task = tasks[tid]
        gate, _why = gate_for(tid, task)
        pr = pr_by_task.get(tid)
        unmet = [d for d in task.get("deps", []) if d not in merged]
        if tid in done:
            # Built somewhere but not merged: the single blocker is the PR.
            if pr:
                rows.append(Row(tid, f"PR #{pr[0]} {pr[1]}", WAIT))
            else:
                rows.append(Row(tid, ORPHAN_TEXT, DEFECT))
        elif unmet:
            needs = ", ".join(_dep_text(d, done, pr_by_task) for d in unmet)
            rows.append(Row(tid, f"needs {needs}", WAIT))
        elif gate:
            rows.append(Row(tid, f"gate {gate}: owner must lift", OWNER))
        elif tid not in allow:
            rows.append(Row(tid, NOT_IN_ALLOW_TEXT + tid, DEFECT))
        elif builders:
            rows.append(Row(tid, f"buildable; {builders} builder(s) running", OK))
        else:
            rows.append(Row(tid, IDLE_TEXT, DEFECT))
    return rows


def has_defect(rows: list[Row]) -> bool:
    """True when any row is a DEFECT (the thing the T-907 incident hid)."""
    return any(row.kind == DEFECT for row in rows)


def exit_code(rows: list[Row]) -> int:
    """1 when a DEFECT row exists, 0 otherwise."""
    return 1 if has_defect(rows) else 0


def format_row(row: Row) -> str:
    """One output line; defect lines start with `!!` so they are greppable."""
    return f"{'!!' if row.kind == DEFECT else '  '} {row.task:8} {row.text}"


def report_lines(rows: list[Row], tasks: dict, merged: set[str]) -> list[str]:
    """The formatted rows followed by the one-line summary."""
    lines = [format_row(row) for row in rows]
    merged_count = sum(1 for tid in tasks if tid in merged)
    defects = sum(1 for row in rows if row.kind == DEFECT)
    lines.append(
        f"-- {len(rows)} unfinished, merged {merged_count}/{len(tasks)}; defects: {defects}"
    )
    return lines


# --- box inputs -------------------------------------------------------------
# Every function below touches the box and is injectable in main(), so the row
# logic is tested off-box while main() still refuses to print a table it cannot
# stand behind.


def probe_tools() -> str | None:
    """Name of the first required box tool that is not on PATH, else None.

    `gh` is required because pr_state.py shells out to it; `pgrep` because the
    builder count and the running_builders() call both need it.
    """
    for tool in ("gh", "pgrep"):
        if shutil.which(tool) is None:
            return tool
    return None


def sync_refs() -> None:
    """Best-effort `git fetch` so the picker sees the box's latest refs."""
    subprocess.run(["git", "fetch", "-q"], cwd=REPO, capture_output=True, text=True)


def load_picker() -> Any:
    """Import the box's mailtinder_pick.py module (task_index/done_set/gate_for)."""
    spec = importlib.util.spec_from_file_location("pick", f"{BASE}/scripts/mailtinder_pick.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load the picker from {BASE}/scripts/mailtinder_pick.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def allow_list() -> set[str]:
    """The lane's ALLOW list, read out of the scheduled-run-lane2.sh wrapper."""
    with open(f"{BASE}/scripts/factory/scheduled-run-lane2.sh", encoding="utf-8") as handle:
        text = handle.read()
    m = re.search(r'^ALLOW="\$\{LANE2_TASKS:-(.*)\}"$', text, re.M)
    return set(m.group(1).split()) if m else set()


def pr_stages() -> dict[str, tuple[int, str]]:
    """task id -> (pr number, stage) from pr_state.py's own labels."""
    out = subprocess.run(
        ["python3", f"{BASE}/scripts/pr_state.py"],
        capture_output=True,
        text=True,
        cwd=REPO,
    ).stdout
    stages: dict[str, tuple[int, str]] = {}
    for line in out.splitlines():
        m = re.match(r"#(\d+) (\S+) (\S+) ", line)
        if m and m.group(2).startswith("factory/"):
            stages[m.group(2)[len("factory/") :]] = (int(m.group(1)), m.group(3))
    return stages


def running_builders() -> int | None:
    """Count running builders, or None when pgrep itself failed.

    pgrep's exit status is the answer, not noise: 0 = at least one process
    matched, 1 = nothing matched. Exit 1 with no output is a real "zero
    builders" and must reach classify, otherwise the IDLE defect - the very
    stall T-907 hid - is unreachable through main(). Any other status (2
    syntax, 3 fatal, a signal) or a missing binary is a tool failure and
    returns None, so main() prints `!! MISSING pgrep` instead of a table that
    reads as defect-free. `probe_tools` already covers an absent pgrep before
    this runs; this None is the "pgrep is there but unusable" case.

    Counted lines of `pgrep -af run-task.sh` contain `bash`, excluding the
    `timeout` wrapper and the `pgrep` line itself.
    """
    try:
        done = subprocess.run(
            ["pgrep", "-af", "run-task.sh"], capture_output=True, text=True
        )
    except OSError:
        return None
    if done.returncode not in (0, 1):  # 0 = match, 1 = no match; else it failed
        return None
    lines = done.stdout.splitlines()
    return len([line for line in lines if "pgrep" not in line and "timeout" not in line and "bash" in line])


def main(
    *,
    out=sys.stdout,
    probe: Callable[[], str | None] = probe_tools,
    sync: Callable[[], None] = sync_refs,
    picker: Callable[[], Any] = load_picker,
    allow: Callable[[], set[str]] = allow_list,
    stages: Callable[[], dict] = pr_stages,
    builders: Callable[[], int | None] = running_builders,
) -> int:
    """Gather the box inputs, print the table, return the exit status.

    A box tool that is missing or unusable prints `!! MISSING <tool>` and exits
    1. It never prints a table it cannot stand behind: an empty table reads as
    "nothing is wrong", which is exactly how T-907 sat for six days.
    """
    missing = probe()
    if missing:
        print(f"!! MISSING {missing}", file=out)
        return 1
    sync()
    module = picker()
    tasks = module.task_index()
    merged = module.done_set(merged_only=True)
    done = module.done_set()
    busy = builders()
    if busy is None:
        print("!! MISSING pgrep", file=out)
        return 1
    rows = classify(
        tasks,
        merged,
        done,
        stages(),
        allow(),
        busy,
        module.gate_for,
        sort_key=getattr(module, "sort_key", None),
    )
    for line in report_lines(rows, tasks, merged):
        print(line, file=out)
    return exit_code(rows)


if __name__ == "__main__":
    sys.exit(main())
