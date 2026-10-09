"""Mutation-testing report for the T-1109 pilot.

Reads the cargo-mutants outcomes written under ``target/mutants`` (one
``outcomes.json`` per run, or several when the run was sharded), groups the
mutants by the areas named in ``tools/mutation_scope.txt`` and prints, per
area, the counts and the mutation score, then the list of **missed** mutants
with their file and line. The exit code is non-zero when an area scores below
the floor recorded in ``tools/mutation_baseline.json``.

Scoring (T-1109): ``score = (caught + timeout) / (caught + missed + timeout)``.
A mutant that times out is counted as caught (behaviour 3), so timeouts are
folded into the numerator instead of the denominator. Unviable mutants never
compile, so they are reported but kept out of the score. An area with no
scored mutants at all has no score and cannot be below its floor.

cargo-mutants artifacts live at ``<--output>/mutants.out/outcomes.json``; a
sharded run nests one such directory per shard, which is why ``--input`` may
be either a single ``outcomes.json`` or a directory searched recursively.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import sys
from typing import Any, Iterable, Sequence

_SCOPE_DEFAULT = Path(__file__).resolve().parent / "mutation_scope.txt"
_BASELINE_DEFAULT = Path(__file__).resolve().parent / "mutation_baseline.json"

CAUGHT_SUMMARIES = frozenset({"CaughtMutant", "Caught", "caught", "killed"})
MISSED_SUMMARIES = frozenset({"MissedMutant", "Missed", "missed", "survived"})
TIMEOUT_SUMMARIES = frozenset({"Timeout", "timeout"})
UNVIABLE_SUMMARIES = frozenset({"Unviable", "unviable"})

_NAME_LINE_RE = re.compile(r":(\d+):")


@dataclass(frozen=True)
class MissedMutant:
    """A surviving mutant: the file it lives in, the line and its name."""

    file: str
    line: int | None
    name: str


@dataclass
class AreaStats:
    """Outcome counts for one mutation-testing area."""

    caught: int = 0
    missed: int = 0
    timeout: int = 0
    unviable: int = 0
    missed_mutants: list[MissedMutant] = field(default_factory=list)

    def scored(self) -> int:
        """Number of mutants that contribute to the score."""
        return self.caught + self.missed + self.timeout

    def score(self) -> float | None:
        """Percent of scored mutants killed; None when nothing was scored."""
        denom = self.scored()
        if denom == 0:
            return None
        return (self.caught + self.timeout) / denom * 100.0


def parse_scope(text: str) -> tuple[dict[str, list[str]], list[str]]:
    """Parse ``tools/mutation_scope.txt`` into (area -> paths, area order).

    Lines are ``area: path, path``; ``#`` starts a comment and blank lines are
    ignored. Order is preserved so the report lists areas the way the file
    does.
    """
    areas: dict[str, list[str]] = {}
    order: list[str] = []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line or ":" not in line:
            continue
        area, rest = line.split(":", 1)
        area = area.strip()
        paths = [p.strip() for p in rest.split(",") if p.strip()]
        if not area or not paths:
            continue
        if area not in areas:
            order.append(area)
        areas[area] = paths
    return areas, order


def _anchor(entry: str) -> str:
    """Reduce a scope entry to the cargo-workspace-relative tail.

    Scope paths are repository-root relative (``backend/crates/domain/src/rules.rs``)
    while cargo-mutants reports them relative to the cargo workspace root
    (``crates/domain/src/rules.rs``). Anchoring on this one form, rather than
    accepting a boundary suffix in either direction, keeps attribution to a
    single root so an unrelated path cannot be folded into the wrong area.
    """
    entry_norm = os.path.normpath(entry).replace(os.sep, "/")
    prefix = "backend/"
    if entry_norm.startswith(prefix):
        return entry_norm[len(prefix) :]
    return entry_norm


def area_for_file(path: str, areas: dict[str, list[str]]) -> str | None:
    """Return the area whose scope entry matches ``path``, else None.

    ``path`` is matched against each scope entry's cargo-workspace-relative
    anchor (see ``_anchor``): equality, or a path-boundary suffix so an
    absolute path (``/repo/backend/crates/domain/src/rules.rs``) still lands in
    the right area. Matching happens in one direction only.
    """
    norm = os.path.normpath(path).replace(os.sep, "/")
    for area, paths in areas.items():
        for entry in paths:
            anchor = _anchor(entry)
            if norm == anchor or norm.endswith("/" + anchor):
                return area
    return None


def find_outcomes(input_path: Path) -> list[Path]:
    """Return outcome files for a single file or a directory tree."""
    if input_path.is_file():
        return [input_path]
    if input_path.is_dir():
        return sorted(input_path.rglob("outcomes.json"))
    return []


def _load_outcomes(path: Path) -> list[dict[str, Any]]:
    """Load one outcomes.json, accepting a bare list or an ``outcomes`` key."""
    data = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(data, dict):
        data = data.get("outcomes", [])
    if not isinstance(data, list):
        return []
    return [o for o in data if isinstance(o, dict)]


def _mutant_object(outcome: dict[str, Any]) -> dict[str, Any] | None:
    scenario = outcome.get("scenario")
    if isinstance(scenario, dict):
        mutant = scenario.get("Mutant")
        if isinstance(mutant, dict):
            return mutant
    return None


def _summary_of(outcome: dict[str, Any]) -> str:
    value = outcome.get("summary")
    if isinstance(value, str):
        return value
    if isinstance(value, dict) and value:
        # Defensive: some enum encodings wrap the variant in an object tag.
        return str(next(iter(value)))
    status = outcome.get("status")
    return status if isinstance(status, str) else ""


def _file_of(mutant: dict[str, Any]) -> str:
    value = mutant.get("file")
    if isinstance(value, str) and value:
        return value
    function = mutant.get("function")
    if isinstance(function, dict) and isinstance(function.get("file"), str):
        return function["file"]
    name = mutant.get("name")
    if isinstance(name, str) and ":" in name:
        return name.split(":", 1)[0]
    return ""


def _line_of(mutant: dict[str, Any]) -> int | None:
    value = mutant.get("line")
    if isinstance(value, int):
        return value
    # cargo-mutants 27.x serialises the location as a `span` with a `start`
    # LineColumn; older/other shapes put `line` on the mutant or its function.
    span = mutant.get("span")
    if isinstance(span, dict):
        start = span.get("start")
        if isinstance(start, dict) and isinstance(start.get("line"), int):
            return start["line"]
    function = mutant.get("function")
    if isinstance(function, dict):
        if isinstance(function.get("line"), int):
            return function["line"]
        fn_span = function.get("span")
        if isinstance(fn_span, dict):
            start = fn_span.get("start")
            if isinstance(start, dict) and isinstance(start.get("line"), int):
                return start["line"]
    name = mutant.get("name")
    if isinstance(name, str):
        match = _NAME_LINE_RE.search(name)
        if match:
            return int(match.group(1))
    return None


def summarize(
    outcomes: Iterable[dict[str, Any]], areas: dict[str, list[str]]
) -> dict[str, AreaStats]:
    """Fold cargo-mutants outcomes into per-area counts.

    Baseline / Success / Failure entries carry no mutant and are ignored.
    """
    stats: dict[str, AreaStats] = {area: AreaStats() for area in areas}
    for outcome in outcomes:
        mutant = _mutant_object(outcome)
        if mutant is None:
            continue
        area = area_for_file(_file_of(mutant), areas)
        if area is None:
            continue
        summary = _summary_of(outcome)
        bucket = stats[area]
        if summary in CAUGHT_SUMMARIES:
            bucket.caught += 1
        elif summary in MISSED_SUMMARIES:
            bucket.missed += 1
            bucket.missed_mutants.append(
                MissedMutant(
                    file=_file_of(mutant),
                    line=_line_of(mutant),
                    name=str(mutant.get("name", "")),
                )
            )
        elif summary in TIMEOUT_SUMMARIES:
            bucket.timeout += 1
        elif summary in UNVIABLE_SUMMARIES:
            bucket.unviable += 1
    return stats


def load_floors(path: Path) -> dict[str, float]:
    """Load ``mutation_baseline.json``: area -> floor percent.

    Accepts ``{"area": {"floor": 80}}`` (the documented shape) and the plain
    ``{"area": 80}`` shorthand.
    """
    data = json.loads(path.read_text(encoding="utf-8"))
    floors: dict[str, float] = {}
    if not isinstance(data, dict):
        return floors
    for area, entry in data.items():
        if area.startswith("#"):
            continue
        if isinstance(entry, dict) and "floor" in entry:
            floors[area] = float(entry["floor"])
        elif isinstance(entry, (int, float)):
            floors[area] = float(entry)
    return floors


def _floor_of(floors: dict[str, float], area: str) -> float | None:
    return floors.get(area)


def _status_for(stats: AreaStats, floor: float | None) -> tuple[str, float | None]:
    score = stats.score()
    if score is None:
        return "no mutants", None
    if floor is not None and score < floor:
        return "BELOW", score
    return "ok", score


def _fmt_score(score: float | None) -> str:
    return "no mutants" if score is None else f"{score:.1f}%"


def _fmt_floor(floor: float | None) -> str:
    return "-" if floor is None else f"{floor:.1f}%"


def _missed_location(mutant: MissedMutant) -> str:
    line = mutant.line if mutant.line is not None else "?"
    return f"{mutant.file}:{line}"


def format_table(rows: list[tuple[str, int, int, int, int, str, str, str]]) -> str:
    """Aligned plain-text table: Area, Caught, Missed, Timeout, Unviable, Score, Floor, Status."""
    headers = [
        "Area",
        "Caught",
        "Missed",
        "Timeout",
        "Unviable",
        "Score",
        "Floor",
        "Status",
    ]
    table: list[list[str]] = [headers]
    for area, caught, missed, timeout, unviable, score_s, floor_s, status in rows:
        table.append(
            [
                area,
                str(caught),
                str(missed),
                str(timeout),
                str(unviable),
                score_s,
                floor_s,
                status,
            ]
        )

    widths = [max(len(r[i]) for r in table) for i in range(len(headers))]
    out = [
        " | ".join(v.ljust(widths[i]) for i, v in enumerate(table[0])),
        "-+-".join("-" * widths[i] for i in range(len(headers))),
    ]
    for row in table[1:]:
        out.append(" | ".join(v.ljust(widths[i]) for i, v in enumerate(row)))
    return "\n".join(out)


def format_markdown(rows: list[tuple[str, int, int, int, int, str, str, str]]) -> str:
    """Markdown table for pasting into an issue (--markdown)."""
    lines = [
        "| Area | Caught | Missed | Timeout | Unviable | Score | Floor | Status |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for area, caught, missed, timeout, unviable, score_s, floor_s, status in rows:
        lines.append(
            f"| {area} | {caught} | {missed} | {timeout} | {unviable} | "
            f"{score_s} | {floor_s} | {status} |"
        )
    return "\n".join(lines)


def _render_missed(stats_by_area: dict[str, AreaStats], order: Sequence[str]) -> list[str]:
    """Bullet lines for every missed mutant, grouped by area."""
    lines: list[str] = []
    for area in order:
        for mutant in stats_by_area[area].missed_mutants:
            lines.append(f"- {area}: {_missed_location(mutant)}  {mutant.name}")
    return lines


def build_report(
    stats_by_area: dict[str, AreaStats],
    order: Sequence[str],
    floors: dict[str, float],
) -> tuple[str, str, dict[str, Any], bool]:
    """Build (text, markdown, json, failed) for the given stats."""
    rows: list[tuple[str, int, int, int, int, str, str, str]] = []
    failed = False
    json_areas: dict[str, Any] = {}

    for area in order:
        stats = stats_by_area[area]
        floor = _floor_of(floors, area)
        status, score = _status_for(stats, floor)
        if status == "BELOW":
            failed = True
        rows.append(
            (
                area,
                stats.caught,
                stats.missed,
                stats.timeout,
                stats.unviable,
                _fmt_score(score),
                _fmt_floor(floor),
                status,
            )
        )
        json_areas[area] = {
            "caught": stats.caught,
            "missed": stats.missed,
            "timeout": stats.timeout,
            "unviable": stats.unviable,
            "score": None if score is None else round(score, 2),
            "floor": floor,
            "status": status,
            "missed_mutants": [
                {"file": m.file, "line": m.line, "name": m.name}
                for m in stats.missed_mutants
            ],
        }

    text_lines = [format_table(rows), "", "Missed mutants:"]
    missed_lines = _render_missed(stats_by_area, order)
    text_lines.extend(missed_lines if missed_lines else ["- (none)"])
    text = "\n".join(text_lines)

    md_lines = [format_markdown(rows), "", "### Missed mutants", ""]
    md_lines.extend(missed_lines if missed_lines else ["- (none)"])
    markdown = "\n".join(md_lines)

    summary = {"areas": json_areas, "failed": failed}
    return text, markdown, summary, failed


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Report cargo-mutants outcomes by area.")
    parser.add_argument(
        "--input",
        required=True,
        help="outcomes.json, or the target/mutants directory to search recursively",
    )
    parser.add_argument("--scope", default=str(_SCOPE_DEFAULT), help="mutation scope file")
    parser.add_argument(
        "--baseline", default=str(_BASELINE_DEFAULT), help="mutation baseline/floors JSON"
    )
    parser.add_argument("--markdown", action="store_true", help="print a markdown table")
    parser.add_argument("--json-out", default=None, help="write the machine summary here")
    args = parser.parse_args(argv)

    try:
        scope_text = Path(args.scope).read_text(encoding="utf-8")
    except OSError as exc:
        sys.stderr.write(f"Error reading scope from {args.scope}: {exc}\n")
        return 2

    areas, order = parse_scope(scope_text)
    if not areas:
        sys.stderr.write(f"Error: no areas found in {args.scope}\n")
        return 2

    outcomes_path = Path(args.input)
    files = find_outcomes(outcomes_path)
    if not files:
        sys.stderr.write(f"Error: no outcomes.json found under {args.input}\n")
        return 2

    try:
        floors = load_floors(Path(args.baseline))
    except OSError as exc:
        sys.stderr.write(f"Error reading baseline from {args.baseline}: {exc}\n")
        return 2
    except (json.JSONDecodeError, ValueError) as exc:
        sys.stderr.write(f"Error parsing baseline {args.baseline}: {exc}\n")
        return 2

    outcomes: list[dict[str, Any]] = []
    for path in files:
        try:
            outcomes.extend(_load_outcomes(path))
        except (OSError, json.JSONDecodeError) as exc:
            sys.stderr.write(f"Error reading outcomes from {path}: {exc}\n")
            return 2

    stats_by_area = summarize(outcomes, areas)
    text, markdown, summary, failed = build_report(stats_by_area, order, floors)

    print(markdown if args.markdown else text)

    if args.json_out:
        out_path = Path(args.json_out)
        try:
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        except OSError as exc:
            sys.stderr.write(f"Error writing {args.json_out}: {exc}\n")
            return 2

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
