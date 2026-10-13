"""Tests for tools/factory/blockers.py (T-1122).

The row logic is pure, so every row kind (wait / owner / ok / DEFECT) and every
defect kind (ORPHAN / NOT IN ALLOW / IDLE) is pinned against a fake task graph,
and main() is driven through injected box tools so a missing `gh` or an empty
`pgrep` is tested without touching a real box.
"""

from __future__ import annotations

import io
from types import SimpleNamespace
import unittest
from unittest import mock

from tools.factory.blockers import (
    DEFECT,
    OK,
    OWNER,
    WAIT,
    Row,
    classify,
    default_sort_key,
    exit_code,
    format_row,
    has_defect,
    main,
    report_lines,
    running_builders,
)


def fake_picker(tasks, done=(), merged=(), gates=None):
    """A stand-in for the box's mailtinder_pick.py, with the same four entry points."""
    gates = gates or {}
    done = set(done)
    merged = set(merged)
    return SimpleNamespace(
        task_index=lambda: {tid: dict(task) for tid, task in tasks.items()},
        done_set=lambda merged_only=False: set(merged) if merged_only else set(done),
        gate_for=lambda tid, task: gates.get(tid, (None, None)),
        sort_key=default_sort_key,
    )


def kinds(rows):
    return [(row.task, row.kind) for row in rows]


def by_task(rows):
    return {row.task: row for row in rows}


class TestClassifyRows(unittest.TestCase):
    def test_orphan_task_and_dependents_report_the_t907_replay(self):
        # 6 Oct replay: T-907 was built on a branch with no PR; the four tasks
        # depending on it said "waiting for a merge" and named nobody.
        tasks = {
            "T-907": {"deps": []},
            "T-908": {"deps": ["T-907"]},
            "T-909": {"deps": []},
        }
        rows = classify(
            tasks,
            merged=set(),
            done={"T-907"},
            pr_by_task={},
            allow={"T-907", "T-908", "T-909"},
            builders=1,
            gate_for=lambda tid, task: (None, None),
        )
        self.assertEqual(
            kinds(rows), [("T-907", DEFECT), ("T-908", WAIT), ("T-909", OK)]
        )
        self.assertEqual(by_task(rows)["T-907"].text, "ORPHAN: built on a branch, NO PR (open one)")
        self.assertEqual(by_task(rows)["T-908"].text, "needs T-907 (ORPHAN)")

    def test_buildable_task_absent_from_allow_is_defect_not_in_allow(self):
        tasks = {"T-1120c": {"deps": []}}
        rows = classify(
            tasks,
            merged=set(),
            done=set(),
            pr_by_task={},
            allow={"T-1120a"},
            builders=3,
            gate_for=lambda tid, task: (None, None),
        )
        self.assertEqual(kinds(rows), [("T-1120c", DEFECT)])
        self.assertIn("NOT IN ALLOW", rows[0].text)
        self.assertIn("allow-add.sh T-1120c", rows[0].text)

    def test_zero_builders_is_idle_and_one_builder_is_ok(self):
        # The v1 bug: one running builder was counted as zero and reported IDLE.
        tasks = {"T-1120b": {"deps": []}}
        kwargs = dict(
            merged=set(),
            done=set(),
            pr_by_task={},
            allow={"T-1120b"},
            gate_for=lambda tid, task: (None, None),
        )
        zero = classify(tasks, builders=0, **kwargs)
        self.assertEqual(kinds(zero), [("T-1120b", DEFECT)])
        self.assertEqual(zero[0].text, "buildable, in ALLOW, NOTHING RUNNING")

        one = classify(tasks, builders=1, **kwargs)
        self.assertEqual(kinds(one), [("T-1120b", OK)])
        self.assertEqual(one[0].text, "buildable; 1 builder(s) running")

        many = classify(tasks, builders=5, **kwargs)
        self.assertEqual(kinds(many), [("T-1120b", OK)])

    def test_gated_buildable_task_is_owner(self):
        tasks = {"T-1120b": {"deps": []}}
        rows = classify(
            tasks,
            merged=set(),
            done=set(),
            pr_by_task={},
            allow={"T-1120b"},
            builders=0,
            gate_for=lambda tid, task: ("owner", "waiting on the security review"),
        )
        self.assertEqual(kinds(rows), [("T-1120b", OWNER)])
        self.assertEqual(rows[0].text, "gate owner: owner must lift")

    def test_open_pr_in_any_stage_is_wait_not_a_defect(self):
        tasks = {"T-1": {"deps": []}, "T-2": {"deps": []}, "T-3": {"deps": []}}
        rows = classify(
            tasks,
            merged=set(),
            done={"T-1", "T-2", "T-3"},
            pr_by_task={"T-1": (12, "review"), "T-2": (13, "queued"), "T-3": (14, "CI red")},
            allow=set(),
            builders=0,
            gate_for=lambda tid, task: (None, None),
        )
        self.assertEqual(
            [(row.task, row.kind, row.text) for row in rows],
            [
                ("T-1", WAIT, "PR #12 review"),
                ("T-2", WAIT, "PR #13 queued"),
                ("T-3", WAIT, "PR #14 CI red"),
            ],
        )

    def test_merged_tasks_are_skipped(self):
        tasks = {"T-1": {"deps": []}, "T-2": {"deps": ["T-1"]}}
        rows = classify(
            tasks,
            merged={"T-1"},
            done={"T-1"},
            pr_by_task={},
            allow={"T-2"},
            builders=0,
            gate_for=lambda tid, task: (None, None),
        )
        self.assertEqual(kinds(rows), [("T-2", DEFECT)])  # T-1 gone; T-2 idle

    def test_dependency_text_names_pr_not_built_and_orphan(self):
        tasks = {
            "T-1": {"deps": ["T-2", "T-3", "T-4"]},
            "T-2": {"deps": []},
            "T-3": {"deps": []},
            "T-4": {"deps": []},
        }
        rows = classify(
            tasks,
            merged=set(),
            done={"T-4"},
            pr_by_task={"T-2": (7, "queued")},
            allow=set(),
            builders=0,
            gate_for=lambda tid, task: (None, None),
        )
        self.assertEqual(
            by_task(rows)["T-1"].text,
            "needs T-2 (PR #7 queued), T-3 (not built), T-4 (ORPHAN)",
        )
        self.assertEqual(by_task(rows)["T-1"].kind, WAIT)

    def test_ids_sort_with_the_injected_picker_key(self):
        kwargs = dict(
            merged=set(),
            done=set(),
            pr_by_task={},
            allow={"T-1120", "T-1120a", "T-1120b"},
            builders=1,
            gate_for=lambda tid, task: (None, None),
        )
        tasks = {"T-1120b": {"deps": []}, "T-1120": {"deps": []}, "T-1120a": {"deps": []}}
        self.assertEqual(
            [row.task for row in classify(tasks, **kwargs)],
            ["T-1120", "T-1120a", "T-1120b"],
        )
        # The picker's own key wins when injected: a plain string sort puts
        # T-10 before T-2, the natural key does not.
        kwargs["allow"] = {"T-2", "T-10"}
        numbered = {"T-10": {"deps": []}, "T-2": {"deps": []}}
        self.assertEqual(
            [row.task for row in classify(numbered, sort_key=lambda tid: tid, **kwargs)],
            ["T-10", "T-2"],
        )
        self.assertEqual(
            [row.task for row in classify(numbered, **kwargs)], ["T-2", "T-10"]
        )


class TestExitCodeAndOutput(unittest.TestCase):
    def test_exit_code_is_one_only_with_a_defect(self):
        defect = Row("T-907", "ORPHAN: built on a branch, NO PR (open one)", DEFECT)
        fine = [
            Row("T-1", "PR #12 review", WAIT),
            Row("T-2", "gate owner: owner must lift", OWNER),
            Row("T-3", "buildable; 1 builder(s) running", OK),
        ]
        self.assertTrue(has_defect([defect]))
        self.assertEqual(exit_code([defect]), 1)
        self.assertFalse(has_defect(fine))
        self.assertEqual(exit_code(fine), 0)
        self.assertEqual(exit_code([]), 0)

    def test_defect_lines_start_with_double_bang(self):
        defect = Row("T-907", "ORPHAN: built on a branch, NO PR (open one)", DEFECT)
        fine = Row("T-1", "PR #12 review", WAIT)
        self.assertTrue(format_row(defect).startswith("!!"))
        self.assertFalse(format_row(fine).startswith("!!"))
        self.assertEqual(format_row(defect), "!! T-907    ORPHAN: built on a branch, NO PR (open one)")

    def test_summary_counts_unfinished_merged_and_defects(self):
        tasks = {"T-1": {"deps": []}, "T-2": {"deps": []}}
        rows = [Row("T-2", "ORPHAN: built on a branch, NO PR (open one)", DEFECT)]
        lines = report_lines(rows, tasks, merged={"T-1"})
        self.assertEqual(lines[-1], "-- 1 unfinished, merged 1/2; defects: 1")
        self.assertEqual(lines[0], "!! T-2      ORPHAN: built on a branch, NO PR (open one)")


class TestMain(unittest.TestCase):
    def run_main(self, picker, *, builders: int | None = 1, allow=(), stages=None, probe=None):
        out = io.StringIO()
        code = main(
            out=out,
            probe=(lambda: None) if probe is None else probe,
            sync=lambda: None,
            picker=lambda: picker,
            allow=lambda: set(allow),
            stages=lambda: dict(stages or {}),
            builders=lambda: builders,
        )
        return code, out.getvalue().splitlines()

    def test_main_reports_the_orphan_and_exits_one(self):
        picker = fake_picker(
            {"T-907": {"deps": []}, "T-908": {"deps": ["T-907"]}},
            done={"T-907"},
            merged=(),
        )
        code, lines = self.run_main(picker, allow={"T-907", "T-908"})
        self.assertEqual(code, 1)
        self.assertEqual(lines[0], "!! T-907    ORPHAN: built on a branch, NO PR (open one)")
        self.assertEqual(lines[1], "   T-908    needs T-907 (ORPHAN)")
        self.assertEqual(lines[-1], "-- 2 unfinished, merged 0/2; defects: 1")

    def test_main_exits_zero_when_every_row_is_explained(self):
        picker = fake_picker({"T-1": {"deps": []}, "T-2": {"deps": []}}, done={"T-2"})
        code, lines = self.run_main(
            picker, allow={"T-1"}, stages={"T-2": (21, "review")}
        )
        self.assertEqual(code, 0)
        self.assertFalse([line for line in lines if line.startswith("!!")])

    def test_main_reports_a_missing_gh_instead_of_an_empty_table(self):
        picker = fake_picker({"T-1": {"deps": []}})
        code, lines = self.run_main(picker, probe=lambda: "gh")
        self.assertEqual(code, 1)
        self.assertEqual(lines, ["!! MISSING gh"])

    def test_main_reports_an_unknown_builder_count_instead_of_a_clean_table(self):
        # Empty pgrep output is a tool failure, not "zero builders": the run
        # must say so rather than print a table that reads as defect-free.
        picker = fake_picker({"T-1": {"deps": []}})
        code, lines = self.run_main(picker, builders=None)
        self.assertEqual(code, 1)
        self.assertEqual(lines, ["!! MISSING pgrep"])

    def test_main_says_missing_pgrep_when_the_tool_is_absent(self):
        picker = fake_picker({"T-1": {"deps": []}})
        code, lines = self.run_main(picker, probe=lambda: "pgrep")
        self.assertEqual(code, 1)
        self.assertEqual(lines, ["!! MISSING pgrep"])

    def test_main_reports_idle_when_no_builder_is_running(self):
        # A genuinely idle box reads as zero builders (pgrep exits 1, no
        # output), so the IDLE row must reach classify instead of main()
        # aborting with `!! MISSING pgrep` before the table is built.
        picker = fake_picker({"T-1": {"deps": []}})
        code, lines = self.run_main(picker, builders=0, allow={"T-1"})
        self.assertEqual(code, 1)
        self.assertEqual(lines[0], "!! T-1      buildable, in ALLOW, NOTHING RUNNING")
        self.assertEqual(lines[-1], "-- 1 unfinished, merged 0/1; defects: 1")


class TestRunningBuilders(unittest.TestCase):
    """The real running_builders(), driven through a fake pgrep (no box needed)."""

    def _pgrep(self, returncode, stdout=""):
        return mock.patch(
            "tools.factory.blockers.subprocess.run",
            return_value=SimpleNamespace(returncode=returncode, stdout=stdout),
        )

    def test_no_match_is_zero_builders_not_a_failure(self):
        # pgrep exits 1 with no output when nothing matches: a real answer.
        with self._pgrep(1, ""):
            self.assertEqual(running_builders(), 0)

    def test_counts_only_bash_run_task_lines(self):
        out = "\n".join(
            [
                "1234 bash run-task.sh T-1",
                "1235 timeout 1200 bash run-task.sh T-2",
                "1236 pgrep -af run-task.sh",
                "1237 python3 something-else.py",
            ]
        )
        with self._pgrep(0, out):
            self.assertEqual(running_builders(), 1)

    def test_pgrep_failure_is_none_not_zero(self):
        for code in (2, 3, -9):
            with self._pgrep(code, ""):
                self.assertIsNone(running_builders())

    def test_missing_binary_is_none(self):
        with mock.patch("tools.factory.blockers.subprocess.run", side_effect=OSError):
            self.assertIsNone(running_builders())


if __name__ == "__main__":
    unittest.main()
