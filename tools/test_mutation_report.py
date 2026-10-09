"""Tests for mutation_report.py and the mutation.sh gate (T-1109).

Covers the pilot acceptance criteria:
  * mutation_report_scores_and_floors
  * mutation_report_lists_missed_mutants_with_lines
  * mutation_script_refuses_unlisted_paths
"""

from __future__ import annotations

import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from tools.mutation_report import main, summarize

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools" / "mutation.sh"
RULES = "backend/crates/domain/src/rules.rs"


def _outcome(summary: str, line: int, name: str, *, with_span: bool = True) -> dict:
    """A cargo-mutants 27.x shaped outcome (location in ``span``, not ``line``)."""
    mutant: dict = {
        "name": name,
        "package": "domain",
        "file": "crates/domain/src/rules.rs",
        "function": {"function_name": "domain::rules::check"},
        "replacement": "(mutated)",
        "genre": "FnValue",
    }
    if with_span:
        mutant["span"] = {
            "start": {"line": line, "column": 5},
            "end": {"line": line, "column": 9},
        }
    return {"scenario": {"Mutant": mutant}, "summary": summary}


def _rules_outcomes() -> list[dict]:
    outcomes = [{"scenario": "Baseline", "summary": "Success"}]
    for i in range(8):
        outcomes.append(
            _outcome("CaughtMutant", 10 + i, f"crates/domain/src/rules.rs:{10 + i}:1: caught")
        )
    outcomes.append(
        _outcome("MissedMutant", 42, "crates/domain/src/rules.rs:42:5: replace guard with true")
    )
    # One missed mutant with no span: the line must come from the name.
    outcomes.append(
        _outcome(
            "MissedMutant",
            77,
            "crates/domain/src/rules.rs:77:5: replace value with 0",
            with_span=False,
        )
    )
    outcomes.append(_outcome("Timeout", 55, "crates/domain/src/rules.rs:55:1: timeout"))
    outcomes.append(_outcome("Unviable", 60, "crates/domain/src/rules.rs:60:1: unviable"))
    return outcomes


def _write_json(path: Path, data: object) -> None:
    path.write_text(json.dumps(data), encoding="utf-8")


class TestMutationReport(unittest.TestCase):
    def test_mutation_report_scores_and_floors(self):
        outcomes = _rules_outcomes()
        areas = {"rules": [RULES]}

        stats = summarize(outcomes, areas)
        self.assertEqual(stats["rules"].caught, 8)
        self.assertEqual(stats["rules"].missed, 2)
        self.assertEqual(stats["rules"].timeout, 1)
        self.assertEqual(stats["rules"].unviable, 1)
        # score = (caught + timeout) / (caught + missed + timeout) = 9 / 11
        self.assertAlmostEqual(stats["rules"].score(), 9 / 11 * 100.0, places=4)

        # An area with nothing scored has no score (it cannot be below floor).
        empty = summarize([{"scenario": "Baseline", "summary": "Success"}], areas)
        self.assertIsNone(empty["rules"].score())

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            scope = tmp_path / "scope.txt"
            scope.write_text(f"rules: {RULES}\n", encoding="utf-8")
            outcomes_path = tmp_path / "outcomes.json"
            _write_json(outcomes_path, outcomes)

            below = tmp_path / "below.json"
            _write_json(below, {"rules": {"floor": 90.0}})
            above = tmp_path / "above.json"
            _write_json(above, {"rules": {"floor": 80.0}})

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                above_rc = main(
                    [
                        "--input",
                        str(outcomes_path),
                        "--scope",
                        str(scope),
                        "--baseline",
                        str(above),
                    ]
                )
            self.assertEqual(above_rc, 0)
            self.assertIn("81.8%", buf.getvalue())

            with contextlib.redirect_stdout(io.StringIO()):
                below_rc = main(
                    [
                        "--input",
                        str(outcomes_path),
                        "--scope",
                        str(scope),
                        "--baseline",
                        str(below),
                    ]
                )
            self.assertEqual(below_rc, 1)

    def test_mutation_report_lists_missed_mutants_with_lines(self):
        outcomes = _rules_outcomes()
        areas = {"rules": [RULES]}
        stats = summarize(outcomes, areas)
        lines = [m.line for m in stats["rules"].missed_mutants]
        self.assertIn(42, lines)
        self.assertIn(77, lines)

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            scope = tmp_path / "scope.txt"
            scope.write_text(f"rules: {RULES}\n", encoding="utf-8")
            baseline = tmp_path / "baseline.json"
            _write_json(baseline, {"rules": {"floor": 80.0}})
            outcomes_path = tmp_path / "outcomes.json"
            _write_json(outcomes_path, outcomes)
            json_out = tmp_path / "summary.json"

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = main(
                    [
                        "--input",
                        str(outcomes_path),
                        "--scope",
                        str(scope),
                        "--baseline",
                        str(baseline),
                        "--json-out",
                        str(json_out),
                    ]
                )
            self.assertEqual(rc, 0)
            out = buf.getvalue()
            self.assertIn("crates/domain/src/rules.rs:42", out)
            self.assertIn("crates/domain/src/rules.rs:77", out)
            self.assertIn("replace guard with true", out)

            md = io.StringIO()
            with contextlib.redirect_stdout(md):
                main(
                    [
                        "--input",
                        str(outcomes_path),
                        "--scope",
                        str(scope),
                        "--baseline",
                        str(baseline),
                        "--markdown",
                    ]
                )
            md_text = md.getvalue()
            self.assertIn("| rules |", md_text)
            self.assertIn("crates/domain/src/rules.rs:42", md_text)

            summary = json.loads(json_out.read_text(encoding="utf-8"))
            self.assertEqual(summary["areas"]["rules"]["missed"], 2)
            missed_lines = [m["line"] for m in summary["areas"]["rules"]["missed_mutants"]]
            self.assertEqual(sorted(missed_lines), [42, 77])
            self.assertEqual(summary["areas"]["rules"]["status"], "ok")


class TestMutationScript(unittest.TestCase):
    def test_mutation_script_refuses_unlisted_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            scope = Path(tmp) / "scope.txt"
            scope.write_text(f"rules: {RULES}\n", encoding="utf-8")
            base = ["bash", str(SCRIPT), "--scope", str(scope), "--list"]

            listed = subprocess.run(
                base + ["--file", RULES], capture_output=True, text=True, check=False
            )
            self.assertEqual(listed.returncode, 0, listed.stderr)
            self.assertIn(RULES, listed.stdout)

            unlisted = subprocess.run(
                base + ["--file", "backend/crates/api/src/lib.rs"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(unlisted.returncode, 0)
            self.assertIn("not in mutation_scope.txt", unlisted.stderr)

            unknown_area = subprocess.run(
                ["bash", str(SCRIPT), "--scope", str(scope), "--area", "nope", "--list"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(unknown_area.returncode, 0)
            self.assertIn("mutation_scope.txt", unknown_area.stderr)


if __name__ == "__main__":
    unittest.main()
