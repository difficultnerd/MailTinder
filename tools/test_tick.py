#!/usr/bin/env python3
"""Tests for tools/factory/tick.sh (T-1123).

The report runs in a sandbox: a temp dir whose bin/ holds fake tools and
`PATH` is that bin only, so "remove a tool" is simply not writing its fake -
exactly the failure the v1 watch hid, where it named tools that did not exist
where it said and the checks were skipped in silence.

Pinned here: the healthy report against the golden copy below, one dropped
component per section, every loud failure the task names (cloudflared running,
an owner-only PR missing from PENDING.md, blockers.py exiting 1, staging
healthz not 200) and the single write the script is allowed to make, the inbox
marker, which happens only after a run with no problems.

`python3 tools/test_tick.py --golden` prints the healthy report, which is how
GOLDEN below is kept honest.

The task's six required tests, and where they live here:

1. all fake tools healthy: exit 0 and the golden output -
   TickHealthyReport.
2. one component removed per section: `!! MISSING <tool>` in that section only,
   the rest of the report intact, exit 1 -
   TickMissingComponents.test_each_component_missing_in_turn_only_that_section_is_affected.
3. cloudflared count 1: `!! CLOUDFLARED RUNNING`, exit 1 -
   TickLoudFailures.test_cloudflared_running_is_loud_and_the_report_still_completes.
4. an owner-only PR missing from PENDING.md: `!! NOT IN PENDING #n` -
   TickLoudFailures.test_owner_only_pr_missing_from_pending_is_named.
5. blockers.py exiting 1: tick.sh exits 1 and its `!!` rows sit under BLOCKERS -
   TickLoudFailures.test_blockers_exiting_one_is_carried_and_its_rows_sit_under_blockers.
6. staging /api/v1/healthz returning 404: `!! FAILED staging healthz` -
   TickLoudFailures.test_staging_healthz_not_200_fails_loudly_while_root_is_fine.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TICK = ROOT / "tools" / "factory" / "tick.sh"
BASH = shutil.which("bash") or "/bin/bash"
REAL_PYTHON = sys.executable or "python3"

SECTIONS = ("CLOCK", "STAGING", "PRS", "BLOCKERS", "LANES", "INBOX", "COUNT")

GOLDEN = """\
== CLOCK ==
   date                     2026-10-13 07:00:00 UTC
   claude                   1.2.3 (Claude Code)
   balance                  12.34 USD (floor 1.00)
   disk                     43%
   cloudflared processes    0
   PAUSED crons             0
== STAGING ==
   staging healthz          200
   staging root             200
== PRS ==
#12 factory/T-1119 review 2h ago
#11 factory/T-1120a build 1h ago
== BLOCKERS ==
   T-1120   buildable; 2 builder(s) running
-- 3 unfinished, merged 118/120; defects: 0
== LANES ==
   builders                 2
   reviews                  3
   reworks                  0
   rework slot              free
== INBOX ==
   newest                   2026-10-13-0700.json
   state                    UNCHANGED
== COUNT ==
merged 118/120 [built-not-merged 2]
-- tick complete: 0 problem(s)
"""

# --- fixtures the fake box answers with ------------------------------------
DATE_OUT = "2026-10-13 07:00:00 UTC\n"
CLAUDE_OUT = "1.2.3 (Claude Code)\n"
BALANCE_OUT = "12.34\n"
DF_OUT = (
    "Filesystem     1024-blocks    Used Available Capacity Mounted on\n"
    "/dev/sda1        103080888 42000000  55800000      43% /\n"
)
CRON_OUT = (
    "ID           SCHEDULE  STATE\n"
    "lane2-build  hourly    RUNNING\n"
    "lane2-digest daily     RUNNING\n"
)
PR_STATE_OUT = "#12 factory/T-1119 review 2h ago\n#11 factory/T-1120a build 1h ago\n"
BLOCKER_ROWS = (
    "   T-1120   buildable; 2 builder(s) running\n"
    "-- 3 unfinished, merged 118/120; defects: 0\n"
)
BLOCKER_DEFECT_ROWS = (
    "!! T-907    ORPHAN: built on a branch, NO PR (open one)\n"
    "-- 4 unfinished, merged 118/120; defects: 1\n"
)
GH_OUT = "7 .github/workflows/deploy.yml\n10 app/lib/main.dart\n"
GH_ORPHAN_OUT = "7 .github/workflows/deploy.yml\n9 infra/terraform/envs/staging/main.tf\n10 app/lib/main.dart\n"
PENDING_OUT = (
    "# PENDING\n"
    "- #7 owner-only: .github/workflows/deploy.yml (owner review)\n"
    "- #12 factory/T-1119 review\n"
)
INBOX_OUT = "2026-10-13-0700.json\n2026-10-13-0600.json\n"
INBOX_NEWEST = "2026-10-13-0700.json"
INBOX_OLDER = "2026-10-13-0600.json"

GENERIC_SHIM = """#!/bin/sh
# T-1123 harness fake: prints fixtures/$tool.out, then exits fixtures/$tool.status.
tool="${0##*/}"
data="${FAKE_DATA:?fake tool ran without FAKE_DATA}"
if [ -f "$data/$tool.out" ]; then
  while IFS= read -r line; do printf '%s\\n' "$line"; done <"$data/$tool.out"
fi
if [ -f "$data/$tool.status" ]; then
  read -r status <"$data/$tool.status"
  exit "$status"
fi
exit 0
"""

PGREP_SHIM = """#!/bin/sh
# T-1123 harness fake pgrep: -af PATTERN prints N fixture lines, exit 0/1 like pgrep.
data="${FAKE_DATA:?fake pgrep ran without FAKE_DATA}"
if [ -f "$data/pgrep.status" ]; then
  read -r status <"$data/pgrep.status"
  exit "$status"
fi
pattern=""
for arg in "$@"; do
  case $arg in
    -a*|-f*|--*) ;;
    *) pattern="$arg" ;;
  esac
done
case $pattern in
  *cloudflared*)
    count="${FAKE_CLOUDFLARED:-0}"
    line="cloudflared tunnel --url http://127.0.0.1:8080"
    ;;
  *run-task.sh*)
    count="${FAKE_BUILDERS:-0}"
    line="bash run-task.sh T-1122"
    ;;
  *review-task.sh*)
    count="${FAKE_REVIEWS:-0}"
    line="bash review-task.sh T-1119"
    ;;
  *rework-task.sh*)
    count="${FAKE_REWORKS:-0}"
    line="bash rework-task.sh PR-9"
    ;;
  *)
    count="${FAKE_OTHER_PROCS:-0}"
    line="bash $pattern"
    ;;
esac
index=0
while [ "$index" -lt "$count" ]; do
  printf '%s %s\\n' "$((1000 + index))" "$line"
  index=$((index + 1))
done
[ "$count" -gt 0 ]
"""

CURL_SHIM = """#!/bin/sh
# T-1123 harness fake curl: prints the fixture http code for the URL it was given.
code="${FAKE_CODE_DEFAULT:-200}"
for arg in "$@"; do
  case $arg in
    *healthz*) code="${FAKE_CODE_HEALTHZ:-200}" ;;
    https://*|http://*) code="${FAKE_CODE_ROOT:-200}" ;;
  esac
done
printf '%s' "$code"
exit "${FAKE_CURL_EXIT:-0}"
"""

PR_STATE_SCRIPT = """#!/usr/bin/env python3
import sys

sys.stdout.write({output!r})
sys.exit({status})
"""

BLOCKERS_SCRIPT = """#!/usr/bin/env python3
import sys

sys.stdout.write({output!r})
sys.exit({status})
"""

PICKER_SCRIPT = '''"""Fake mailtinder_pick.py: the same four entry points blockers.py reads."""

TASKS = {{tid: {{"deps": []}} for tid in {tasks!r}}}
MERGED = set({merged!r})
DONE = MERGED | set({extra!r})


def task_index():
    return {{tid: dict(task) for tid, task in TASKS.items()}}


def done_set(merged_only=False):
    return set(MERGED) if merged_only else set(DONE)


def gate_for(task_id, task):
    return (None, None)
'''

# component -> the section that must report it missing
DROPS = (
    ("CLOCK", "date"),
    ("CLOCK", "claude"),
    ("CLOCK", "deepseek-balance"),
    ("CLOCK", "df"),
    ("CLOCK", "hermes"),
    ("CLOCK", "pgrep"),
    ("STAGING", "curl"),
    ("PRS", "pr_state.py"),
    ("PRS", "gh"),
    ("PRS", "PENDING.md"),
    ("BLOCKERS", "blockers.py"),
    ("LANES", "pgrep"),
    ("INBOX", "inbox"),
    ("INBOX", "ls"),
    ("COUNT", "mailtinder_pick.py"),
)

# components that are paths, not tools: dropping one points its variable at a
# path that does not exist.
PATH_DROPS = {
    "pr_state.py": "TICK_PR_STATE",
    "PENDING.md": "TICK_PENDING",
    "blockers.py": "TICK_BLOCKERS",
    "mailtinder_pick.py": "TICK_PICKER",
    "inbox": "TICK_INBOX",
}

TOOLS = ("date", "claude", "deepseek-balance", "df", "pgrep", "hermes", "curl", "gh", "ls")


class Sandbox:
    """A temp dir with fake tools in bin/ and a fake factory box in box/."""

    def __init__(
        self,
        drop=None,
        cloudflared=0,
        builders=2,
        reviews=3,
        reworks=0,
        balance=BALANCE_OUT,
        healthz="200",
        root_code="200",
        curl_exit=0,
        paused_crons=0,
        pr_state_out=PR_STATE_OUT,
        pr_state_status=0,
        blocker_rows=BLOCKER_ROWS,
        blocker_status=0,
        gh_out=GH_OUT,
        pending=PENDING_OUT,
        inbox=INBOX_OUT,
        marker=INBOX_NEWEST,
        picker_tasks=120,
        picker_merged=118,
        picker_extra=2,
        pgrep_status=None,
    ):
        self.drop = drop
        self.root = Path(tempfile.mkdtemp(prefix="tick-sandbox-"))
        self.bin = self.root / "bin"
        self.data = self.root / "fixtures"
        self.box = self.root / "box"
        self.home = self.root / "home"
        for directory in (self.bin, self.data, self.box, self.home):
            directory.mkdir(parents=True)

        for fixture in {
            "date": DATE_OUT,
            "claude": CLAUDE_OUT,
            "deepseek-balance": balance,
            "df": DF_OUT,
            "hermes": CRON_OUT + ("lane2-weekly weekly  PAUSED\n" * paused_crons),
            "gh": gh_out,
            "ls": inbox,
        }.items():
            self._fixture(*fixture)
        if pgrep_status is not None:
            (self.data / "pgrep.status").write_text(str(pgrep_status), encoding="utf-8")

        (self.box / "PENDING.md").write_text(pending, encoding="utf-8")
        (self.box / "pr_state.py").write_text(
            PR_STATE_SCRIPT.format(output=pr_state_out, status=pr_state_status),
            encoding="utf-8",
        )
        (self.box / "blockers.py").write_text(
            BLOCKERS_SCRIPT.format(output=blocker_rows, status=blocker_status),
            encoding="utf-8",
        )
        (self.box / "mailtinder_pick.py").write_text(
            PICKER_SCRIPT.format(
                tasks=[f"T-{n:04d}" for n in range(1, picker_tasks + 1)],
                merged=[f"T-{n:04d}" for n in range(1, picker_merged + 1)],
                extra=[f"T-{n:04d}" for n in range(picker_merged + 1, picker_merged + 1 + picker_extra)],
            ),
            encoding="utf-8",
        )
        (self.box / "inbox").mkdir()
        (self.box / "inbox" / "2026-10-13-0700.json").write_text("{}\n", encoding="utf-8")
        (self.box / "inbox" / "2026-10-13-0600.json").write_text("{}\n", encoding="utf-8")
        (self.box / "state").mkdir()
        self.marker_file = self.box / "state" / "inbox-marker"
        self.marker_file.write_text(marker + "\n", encoding="utf-8")

        self._write_fakes()

        self.env = {
            "PATH": str(self.bin),
            "HOME": str(self.home),
            "FAKE_DATA": str(self.data),
            "FAKE_CLOUDFLARED": str(cloudflared),
            "FAKE_BUILDERS": str(builders),
            "FAKE_REVIEWS": str(reviews),
            "FAKE_REWORKS": str(reworks),
            "FAKE_CODE_HEALTHZ": healthz,
            "FAKE_CODE_ROOT": root_code,
            "FAKE_CURL_EXIT": str(curl_exit),
            "TICK_BASE": str(self.box),
            "TICK_BOX_REPO": str(self.box),
            "TICK_PR_STATE": str(self.box / "pr_state.py"),
            "TICK_PENDING": str(self.box / "PENDING.md"),
            "TICK_BLOCKERS": str(self.box / "blockers.py"),
            "TICK_PICKER": str(self.box / "mailtinder_pick.py"),
            "TICK_INBOX": str(self.box / "inbox"),
            "TICK_INBOX_MARKER": str(self.marker_file),
            "TICK_STAGING": "https://staging.invalid",
        }
        if drop in PATH_DROPS:
            self.env[PATH_DROPS[drop]] = str(self.root / "absent" / drop)

    def _fixture(self, name, text):
        (self.data / f"{name}.out").write_text(text, encoding="utf-8")

    def _write_fakes(self):
        # python3 must be the real interpreter: pr_state.py, blockers.py and the
        # picker module are real Python the report runs.
        (self.bin / "python3").write_text(
            f'#!/bin/sh\nexec "{REAL_PYTHON}" "$@"\n', encoding="utf-8"
        )
        for name in ("pgrep", "curl"):
            if name == self.drop:
                continue
            (self.bin / name).write_text(
                PGREP_SHIM if name == "pgrep" else CURL_SHIM, encoding="utf-8"
            )
        for name in TOOLS:
            if name in ("pgrep", "curl") or name == self.drop:
                continue
            (self.bin / name).write_text(GENERIC_SHIM, encoding="utf-8")
        for path in self.bin.iterdir():
            path.chmod(0o755)

    def run(self, timeout=60):
        return subprocess.run(
            [BASH, str(TICK)],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(ROOT),
        )

    def marker(self):
        return self.marker_file.read_text(encoding="utf-8")

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def tree(self):
        return {
            str(path.relative_to(self.box)): path.read_bytes()
            for path in sorted(self.box.rglob("*"))
            if path.is_file()
        }


def sandbox(test, **kwargs):
    box = Sandbox(**kwargs)
    test.addCleanup(box.cleanup)
    return box


def sections_of(lines, line):
    """Every section header whose block contains `line`."""
    found = []
    heading = None
    for current in lines:
        if current.startswith("== ") and current.endswith(" =="):
            heading = current.strip("= ").strip()
        elif current == line and heading:
            found.append(heading)
    return found


def section_of(lines, line):
    """The first section header whose block contains `line` (None when absent)."""
    found = sections_of(lines, line)
    return found[0] if found else None


class TickHealthyReport(unittest.TestCase):
    def test_all_fake_tools_healthy_exit_zero_and_output_equals_the_golden_file(self):
        box = sandbox(self)
        done = box.run()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(done.stdout, GOLDEN)

    def test_sections_appear_in_the_fixed_order(self):
        box = sandbox(self)
        lines = box.run().stdout.splitlines()
        headers = [line for line in lines if line.startswith("== ")]
        self.assertEqual(headers, [f"== {name} ==" for name in SECTIONS])

    def test_count_reports_merged_only_and_the_built_not_merged_tasks_separately(self):
        # The v1 watch folded built-but-unmerged tasks into the merged total and
        # reported 120/120 while two tasks sat on branches with no PR.
        box = sandbox(self)
        lines = box.run().stdout.splitlines()
        self.assertIn("merged 118/120 [built-not-merged 2]", lines)
        self.assertNotIn("merged 120/120 [built-not-merged 2]", lines)

    def test_count_with_no_orphan_branches_brackets_zero(self):
        box = sandbox(self, picker_merged=120, picker_extra=0)
        self.assertIn("merged 120/120 [built-not-merged 0]", box.run().stdout)


class TickMissingComponents(unittest.TestCase):
    def test_each_component_missing_in_turn_only_that_section_is_affected(self):
        for section, component in DROPS:
            with self.subTest(section=section, component=component):
                box = sandbox(self, drop=component)
                done = box.run()
                lines = done.stdout.splitlines()
                self.assertEqual(done.returncode, 1, component)
                self.assertEqual(
                    [line for line in lines if line.startswith("== ")],
                    [f"== {name} ==" for name in SECTIONS],
                )
                reports = sections_of(lines, f"!! MISSING {component}")
                self.assertIn(f"!! MISSING {component}", lines, component)
                if component == "pgrep":
                    # CLOCK counts cloudflared with pgrep too, so both sections
                    # say so; pgrep is in the lanes section as well.
                    self.assertIn(section, reports, f"{component}: {done.stdout}")
                else:
                    self.assertEqual(reports, [section], f"{component}: {done.stdout}")
                # every other section still printed its payload
                for other, expected in (
                    ("CLOCK", "   cloudflared processes    0"),
                    ("STAGING", "   staging healthz          200"),
                    ("PRS", "#12 factory/T-1119 review 2h ago"),
                    ("BLOCKERS", "   T-1120   buildable; 2 builder(s) running"),
                    ("LANES", "   builders                 2"),
                    ("INBOX", "   newest                   2026-10-13-0700.json"),
                    ("COUNT", "merged 118/120 [built-not-merged 2]"),
                ):
                    if other in reports:
                        continue
                    self.assertIn(expected, lines, f"{other} went missing with {component}")

    def test_pgrep_missing_is_reported_in_clock_and_lanes(self):
        box = sandbox(self, drop="pgrep")
        lines = box.run().stdout.splitlines()
        self.assertEqual(sections_of(lines, "!! MISSING pgrep"), ["CLOCK", "LANES"])
        self.assertEqual(lines.count("!! MISSING pgrep"), 2)

    def test_a_tool_that_exists_but_exits_non_zero_prints_failed(self):
        box = sandbox(self, pgrep_status=2)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertEqual(sections_of(lines, "!! FAILED pgrep"), ["CLOCK", "LANES", "LANES", "LANES"])
        self.assertNotIn("!! MISSING pgrep", lines)

    def test_curl_that_cannot_run_prints_failed_for_both_staging_urls(self):
        box = sandbox(self, curl_exit=7)
        done = box.run()
        self.assertEqual(done.returncode, 1)
        self.assertIn("!! FAILED staging healthz", done.stdout)
        self.assertIn("!! FAILED staging root", done.stdout)


class TickLoudFailures(unittest.TestCase):
    def test_cloudflared_running_is_loud_and_the_report_still_completes(self):
        box = sandbox(self, cloudflared=1)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertEqual(section_of(lines, "!! CLOUDFLARED RUNNING"), "CLOCK")
        self.assertIn("   cloudflared processes    1", lines)
        self.assertIn("-- tick complete: 1 problem(s)", lines)
        self.assertEqual(len([line for line in lines if line.startswith("== ")]), 7)

    def test_owner_only_pr_missing_from_pending_is_named(self):
        box = sandbox(self, gh_out=GH_ORPHAN_OUT)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertIn("!! NOT IN PENDING #9", lines)
        self.assertEqual(section_of(lines, "!! NOT IN PENDING #9"), "PRS")
        # #7 is owner-only and listed in PENDING.md; #10 is not owner-only
        self.assertNotIn("!! NOT IN PENDING #7", lines)
        self.assertNotIn("!! NOT IN PENDING #10", lines)

    def test_one_line_per_orphan_pr_even_with_several_owner_only_files(self):
        box = sandbox(
            self,
            gh_out="9 .github/workflows/ci.yml\n9 infra/terraform/envs/prod/main.tf\n",
        )
        lines = box.run().stdout.splitlines()
        self.assertEqual(lines.count("!! NOT IN PENDING #9"), 1)

    def test_blockers_exiting_one_is_carried_and_its_rows_sit_under_blockers(self):
        box = sandbox(self, blocker_rows=BLOCKER_DEFECT_ROWS, blocker_status=1)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        for row in BLOCKER_DEFECT_ROWS.splitlines():
            self.assertEqual(section_of(lines, row), "BLOCKERS", row)
        self.assertNotIn("!! MISSING blockers.py", lines)
        self.assertNotIn("!! FAILED blockers.py", lines)

    def test_blockers_crashing_is_reported_as_failed(self):
        box = sandbox(self, blocker_rows="", blocker_status=2)
        done = box.run()
        self.assertEqual(done.returncode, 1)
        self.assertEqual(section_of(done.stdout.splitlines(), "!! FAILED blockers.py"), "BLOCKERS")

    def test_staging_healthz_not_200_fails_loudly_while_root_is_fine(self):
        box = sandbox(self, healthz="404")
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertIn("   staging healthz          404", lines)
        self.assertEqual(section_of(lines, "!! FAILED staging healthz"), "STAGING")
        self.assertIn("   staging root             200", lines)
        self.assertNotIn("!! FAILED staging root", lines)

    def test_staging_root_not_200_fails_loudly(self):
        box = sandbox(self, root_code="503")
        done = box.run()
        self.assertEqual(done.returncode, 1)
        self.assertIn("!! FAILED staging root", done.stdout)
        self.assertNotIn("!! FAILED staging healthz", done.stdout)

    def test_balance_below_the_dollar_floor_is_loud(self):
        box = sandbox(self, balance="0.50\n")
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertIn("   balance                  0.50 USD (floor 1.00)", lines)
        self.assertEqual(section_of(lines, "!! BALANCE LOW 0.50 < 1.00"), "CLOCK")

    def test_balance_at_the_floor_is_not_loud(self):
        box = sandbox(self, balance="1.00\n")
        done = box.run()
        self.assertEqual(done.returncode, 0, done.stdout)
        self.assertNotIn("!! BALANCE LOW", done.stdout)

    def test_a_paused_cron_is_loud(self):
        box = sandbox(self, paused_crons=1)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertIn("   PAUSED crons             1", lines)
        self.assertEqual(section_of(lines, "!! CRON PAUSED 1"), "CLOCK")

    def test_pr_state_exiting_non_zero_is_reported_as_failed(self):
        box = sandbox(self, pr_state_status=3)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertEqual(section_of(lines, "!! FAILED pr_state.py"), "PRS")
        self.assertIn("#12 factory/T-1119 review 2h ago", lines)

    def test_lanes_report_the_three_counts_and_the_rework_slot(self):
        box = sandbox(self, builders=2, reviews=1, reworks=1)
        lines = box.run().stdout.splitlines()
        for expected in (
            "   builders                 2",
            "   reviews                  1",
            "   reworks                  1",
            "   rework slot              busy",
        ):
            self.assertIn(expected, lines)


class TickInboxMarker(unittest.TestCase):
    def test_new_inbox_file_is_reported_new(self):
        box = sandbox(self, marker=INBOX_OLDER)
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertIn("   newest                   2026-10-13-0700.json", lines)
        self.assertIn("   state                    NEW", lines)
        self.assertEqual(done.returncode, 0, done.stdout)

    def test_marker_is_updated_after_a_clean_run(self):
        box = sandbox(self, marker=INBOX_OLDER)
        box.run()
        self.assertEqual(box.marker(), INBOX_NEWEST + "\n")

    def test_marker_is_updated_only_after_a_successful_run(self):
        box = sandbox(self, marker=INBOX_OLDER, cloudflared=1)
        before = box.tree()
        done = box.run()
        self.assertEqual(done.returncode, 1)
        self.assertIn("   state                    NEW", done.stdout)
        self.assertEqual(box.marker(), INBOX_OLDER + "\n")
        self.assertEqual(box.tree(), before, "a failing run must write nothing")

    def test_second_run_of_the_same_inbox_is_unchanged_and_clean(self):
        box = sandbox(self, marker=INBOX_NEWEST)
        first = box.run()
        second = box.run()
        self.assertEqual(first.stdout, second.stdout)
        self.assertIn("   state                    UNCHANGED", second.stdout)
        self.assertEqual(second.returncode, 0)

    def test_a_failing_run_writes_nothing_at_all(self):
        box = sandbox(self, cloudflared=1)
        before = box.tree()
        box.run()
        self.assertEqual(box.tree(), before)

    def test_empty_inbox_is_loud(self):
        box = sandbox(self, inbox="")
        done = box.run()
        lines = done.stdout.splitlines()
        self.assertEqual(done.returncode, 1)
        self.assertIn("   newest                   (none)", lines)
        self.assertEqual(section_of(lines, "!! INBOX EMPTY"), "INBOX")


def main(argv):
    if "--golden" in argv:
        box = Sandbox()
        try:
            sys.stdout.write(box.run().stdout)
        finally:
            box.cleanup()
        return 0
    unittest.main(argv=[argv[0]] + [arg for arg in argv[1:] if arg != "--golden"])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
