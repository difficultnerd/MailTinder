"""Coverage floor checker for Rust and Flutter.

Reads two LCOV files (Rust workspace and Flutter app) and a floors JSON file,
prints an aligned summary table, and exits non-zero if any area is below floor
or if floors are set below the minimums defined in S10 10.2.

Note on excluded files:
Binaries keep `main.rs` thin (parse config, call a function in `lib.rs`),
which is why `src/main.rs` is excluded from coverage. Similarly, Flutter's
`lib/main.dart` is excluded.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
import re
import sys
from typing import Sequence

MINIMUMS = {"domain": 90, "rust_default": 75, "flutter_lib": 75}


@dataclass
class Totals:
    found: int = 0  # LF
    hit: int = 0  # LH

    def percent(self) -> float | None:
        if self.found == 0:
            return None
        return (self.hit / self.found) * 100.0


def parse_lcov(text: str) -> dict[str, Totals]:
    """Parse LCOV string into per-source-file Totals.

    Records are delimited by 'end_of_record'.
    Uses LF and LH if present. If LF/LH are missing but DA lines exist,
    counts DA lines as found and DA lines with non-zero execution count as hit.
    """
    files: dict[str, Totals] = {}
    current_sf: str | None = None
    lf: int | None = None
    lh: int | None = None
    da_lines: list[tuple[int, int]] = []

    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("SF:"):
            current_sf = line[3:].strip()
            lf = None
            lh = None
            da_lines = []
        elif line.startswith("LF:"):
            try:
                lf = int(line[3:].strip())
            except ValueError:
                pass
        elif line.startswith("LH:"):
            try:
                lh = int(line[3:].strip())
            except ValueError:
                pass
        elif line.startswith("DA:"):
            parts = line[3:].split(",")
            if len(parts) >= 2:
                try:
                    line_no = int(parts[0].strip())
                    count = int(parts[1].strip())
                    da_lines.append((line_no, count))
                except ValueError:
                    pass
        elif line == "end_of_record":
            if current_sf is not None:
                if lf is not None and lh is not None:
                    totals = Totals(found=lf, hit=lh)
                elif da_lines:
                    found = len(da_lines)
                    hit = sum(1 for _, cnt in da_lines if cnt > 0)
                    totals = Totals(found=found, hit=hit)
                else:
                    totals = Totals(found=0, hit=0)
                files[current_sf] = totals
            current_sf = None
            lf = None
            lh = None
            da_lines = []

    # Handle file ending without end_of_record if SF was active
    if current_sf is not None:
        if lf is not None and lh is not None:
            totals = Totals(found=lf, hit=lh)
        elif da_lines:
            found = len(da_lines)
            hit = sum(1 for _, cnt in da_lines if cnt > 0)
            totals = Totals(found=found, hit=hit)
        else:
            totals = Totals(found=0, hit=0)
        files[current_sf] = totals

    return files


_RUST_CRATE_RE = re.compile(r"(?:^|[/\\])crates[/\\]([^/\\]+)[/\\]src[/\\]")


def rust_crate_of(path: str) -> str | None:
    """Find the segment 'crates/<name>/src/' in path and return '<name>', else None."""
    m = _RUST_CRATE_RE.search(path)
    if m:
        return m.group(1)
    return None


def rust_area_totals(files: dict[str, Totals], cfg: dict) -> dict[str, Totals]:
    """Aggregate totals for each rust crate, applying exclusions.

    cfg expects:
      exclude_crates: list[str]
      exclude_files: list[str] (path endings)
    """
    rust_cfg = cfg.get("rust", {})
    exclude_crates = set(rust_cfg.get("exclude_crates", []))
    exclude_files = tuple(rust_cfg.get("exclude_files", []))

    crate_totals: dict[str, Totals] = {}

    for path, totals in files.items():
        crate = rust_crate_of(path)
        if crate is None:
            continue
        if crate in exclude_crates:
            continue
        if exclude_files and any(path.endswith(suffix) for suffix in exclude_files):
            continue

        if crate not in crate_totals:
            crate_totals[crate] = Totals(found=0, hit=0)
        crate_totals[crate].found += totals.found
        crate_totals[crate].hit += totals.hit

    return crate_totals


def flutter_totals(files: dict[str, Totals], cfg: dict) -> Totals:
    """Aggregate totals for flutter lib/ files, applying exclusions.

    cfg expects:
      flutter.exclude_files: list[str] (path endings)
    """
    flutter_cfg = cfg.get("flutter", {})
    exclude_files = tuple(flutter_cfg.get("exclude_files", []))

    total = Totals(found=0, hit=0)
    for path, totals in files.items():
        if "lib/" not in path and not path.startswith("lib/"):
            continue
        if exclude_files and any(path.endswith(suffix) for suffix in exclude_files):
            continue
        total.found += totals.found
        total.hit += totals.hit

    return total


def check_minimums(cfg: dict) -> list[str]:
    """Return error strings if any floor in cfg is below MINIMUMS."""
    errors: list[str] = []
    rust_cfg = cfg.get("rust", {})
    flutter_cfg = cfg.get("flutter", {})

    domain_floor = rust_cfg.get("domain")
    if domain_floor is not None and domain_floor < MINIMUMS["domain"]:
        errors.append(
            f"Floor for rust.domain ({domain_floor}%) is below minimum of {MINIMUMS['domain']}%"
        )

    rust_default = rust_cfg.get("default")
    if rust_default is not None and rust_default < MINIMUMS["rust_default"]:
        errors.append(
            f"Floor for rust.default ({rust_default}%) is below minimum of {MINIMUMS['rust_default']}%"
        )

    for k, v in rust_cfg.items():
        if k in ("domain", "default", "exclude_crates", "exclude_files"):
            continue
        if isinstance(v, (int, float)) and v < MINIMUMS["rust_default"]:
            errors.append(
                f"Floor for rust crate '{k}' ({v}%) is below minimum of {MINIMUMS['rust_default']}%"
            )

    flutter_lib = flutter_cfg.get("lib")
    if flutter_lib is not None and flutter_lib < MINIMUMS["flutter_lib"]:
        errors.append(
            f"Floor for flutter.lib ({flutter_lib}%) is below minimum of {MINIMUMS['flutter_lib']}%"
        )

    return errors


def format_table(rows: list[tuple[str, int, int, str, float, str]]) -> str:
    """Format table with headers: Area, Lines, Hit, Percent, Floor, Status."""
    headers = ["Area", "Lines", "Hit", "Percent", "Floor", "Status"]
    table_rows = [headers]
    for area, found, hit, pct_str, floor, status in rows:
        table_rows.append([area, str(found), str(hit), pct_str, f"{floor:.1f}%", status])

    col_widths = [max(len(row[i]) for row in table_rows) for i in range(len(headers))]

    lines: list[str] = []
    header_line = " | ".join(val.ljust(col_widths[i]) for i, val in enumerate(table_rows[0]))
    separator_line = "-+-".join("-" * col_widths[i] for i in range(len(headers)))
    lines.append(header_line)
    lines.append(separator_line)

    for row in table_rows[1:]:
        row_line = " | ".join(val.ljust(col_widths[i]) for i, val in enumerate(row))
        lines.append(row_line)

    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check coverage against floors.")
    parser.add_argument("--rust", required=True, help="Path to Rust LCOV file")
    parser.add_argument("--flutter", required=True, help="Path to Flutter LCOV file")
    parser.add_argument(
        "--floors",
        default=os.path.join(os.path.dirname(__file__), "coverage_floors.json"),
        help="Path to coverage floors JSON file",
    )

    args = parser.parse_args(argv)

    # 1. Read files (Exit code 2 if missing or unreadable)
    try:
        with open(args.floors, "r", encoding="utf-8") as f:
            cfg = json.load(f)
    except Exception as e:
        sys.stderr.write(f"Error reading floors configuration from {args.floors}: {e}\n")
        return 2

    try:
        with open(args.rust, "r", encoding="utf-8") as f:
            rust_lcov_text = f.read()
    except Exception as e:
        sys.stderr.write(f"Error reading Rust LCOV file from {args.rust}: {e}\n")
        return 2

    try:
        with open(args.flutter, "r", encoding="utf-8") as f:
            flutter_lcov_text = f.read()
    except Exception as e:
        sys.stderr.write(f"Error reading Flutter LCOV file from {args.flutter}: {e}\n")
        return 2

    # Check minimums
    min_errors = check_minimums(cfg)
    if min_errors:
        for err in min_errors:
            sys.stderr.write(f"Error: {err}\n")
        return 1

    rust_files = parse_lcov(rust_lcov_text)
    flutter_files = parse_lcov(flutter_lcov_text)

    rust_totals = rust_area_totals(rust_files, cfg)
    flutter_tot = flutter_totals(flutter_files, cfg)

    rust_cfg = cfg.get("rust", {})
    rust_default_floor = float(rust_cfg.get("default", 75))
    flutter_floor = float(cfg.get("flutter", {}).get("lib", 75))

    failed = False
    table_data: list[tuple[str, int, int, str, float, str]] = []

    # Sort Rust crates alphabetically
    for crate in sorted(rust_totals.keys()):
        tot = rust_totals[crate]
        floor = float(rust_cfg.get(crate, rust_default_floor))
        pct = tot.percent()
        if pct is None:
            pct_str = "no lines"
            status = "ok"
        else:
            pct_str = f"{pct:.1f}%"
            if pct < floor:
                status = "BELOW"
                failed = True
            else:
                status = "ok"
        table_data.append((crate, tot.found, tot.hit, pct_str, floor, status))

    # Flutter row
    f_pct = flutter_tot.percent()
    if f_pct is None:
        f_pct_str = "no lines"
        f_status = "ok"
    else:
        f_pct_str = f"{f_pct:.1f}%"
        if f_pct < flutter_floor:
            f_status = "BELOW"
            failed = True
        else:
            f_status = "ok"
    table_data.append(("flutter lib", flutter_tot.found, flutter_tot.hit, f_pct_str, flutter_floor, f_status))

    table_text = format_table(table_data)
    print(table_text)

    # Append to $GITHUB_STEP_SUMMARY when set
    step_summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary_path:
        try:
            with open(step_summary_path, "a", encoding="utf-8") as f:
                f.write("### Coverage Report\n\n```\n")
                f.write(table_text)
                f.write("\n```\n")
        except Exception as e:
            sys.stderr.write(f"Warning: could not write to GITHUB_STEP_SUMMARY: {e}\n")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
