#!/usr/bin/env python3
"""AC coverage checker.

Proves that every acceptance criterion, invariant, S5 verification ID,
classifier test ID and ASVS row named by a merged task file has a test
whose name carries that ID, and prints overall S2 coverage.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class KnownId:
    id: str  # exactly as written in source, e.g. "SW-03 AC2", "INV-5", "V3.4.3", "SES-1", "GUARD-1"
    source: str  # "S2", "S3", "S5", "S10", "ASVS"
    active: bool  # False for S2 ACs marked "Removed" or "v2."
    verify: str | None  # ASVS only: "test", "semgrep", "ci", "dast", "review", "-"
    status: str | None  # ASVS only: "Planned", "Verified", ...


def parse_s2(text: str) -> list[KnownId]:
    """Parse S2 acceptance criteria and cross-cutting IDs."""
    story_re = re.compile(r"^### ([A-Z]{2}-\d{2})\b")
    ac_re = re.compile(r"^- \*\*(AC\d+[a-z]?)\.\*\*\s*(.*)")
    xc_re = re.compile(r"^- \*\*(XC-\d+)\.\*\*\s*(.*)")

    res: list[KnownId] = []
    current_story: str | None = None

    for line in text.splitlines():
        m_story = story_re.match(line)
        if m_story:
            current_story = m_story.group(1)
            continue

        m_ac = ac_re.match(line)
        if m_ac and current_story:
            ac_num = m_ac.group(1)
            rest = m_ac.group(2)
            active = not (rest.startswith("Removed") or rest.startswith("v2."))
            res.append(
                KnownId(
                    id=f"{current_story} {ac_num}",
                    source="S2",
                    active=active,
                    verify=None,
                    status=None,
                )
            )
            continue

        m_xc = xc_re.match(line)
        if m_xc:
            xc_id = m_xc.group(1)
            rest = m_xc.group(2)
            active = not (rest.startswith("Removed") or rest.startswith("v2."))
            res.append(
                KnownId(
                    id=xc_id,
                    source="S2",
                    active=active,
                    verify=None,
                    status=None,
                )
            )
            continue

    return res


def parse_s3_invariants(text: str) -> list[KnownId]:
    """Parse S3 invariants (INV-\\d+)."""
    raw_ids = sorted(list(set(re.findall(r"\*\*(INV-\d+)\.\*\*", text))))
    return [
        KnownId(id=inv_id, source="S3", active=True, verify=None, status=None)
        for inv_id in raw_ids
    ]


def parse_s5_ids(text: str) -> list[KnownId]:
    """Parse S5 IDs (DEL, SES, CFG, JOB, RL, LOG, EXP, JEV, INV-T, NA-T)."""
    raw_ids = sorted(
        list(
            set(
                re.findall(
                    r"\b((?:DEL|SES|CFG|JOB|RL|LOG|EXP|JEV)-\d+|(?:INV|NA)-T\d+)\b",
                    text,
                )
            )
        )
    )
    return [
        KnownId(id=sid, source="S5", active=True, verify=None, status=None)
        for sid in raw_ids
    ]


def parse_s10_ids(text: str) -> list[KnownId]:
    """Parse S10 classifier and test IDs (JEV, BAKE, EXP, GUARD, STAT)."""
    raw_ids = sorted(
        list(set(re.findall(r"\b((?:JEV|BAKE|EXP|GUARD|STAT)-\d+)\b", text)))
    )
    return [
        KnownId(id=sid, source="S10", active=True, verify=None, status=None)
        for sid in raw_ids
    ]


def parse_register(text: str) -> list[KnownId]:
    """Parse ASVS register table rows."""
    row_re = re.compile(
        r"^\| (V\d+\.\d+\.\d+) \| (\d) \| .* \| ([^|]*) \| ([^|]*) \|\s*$"
    )
    res: list[KnownId] = []
    for line in text.splitlines():
        m = row_re.match(line)
        if m:
            row_id = m.group(1)
            verify_raw = m.group(3).strip()
            status_raw = m.group(4).strip()
            if verify_raw.lower().startswith("test"):
                verify = "test"
            else:
                verify = verify_raw.lower()
            res.append(
                KnownId(
                    id=row_id,
                    source="ASVS",
                    active=True,
                    verify=verify,
                    status=status_raw,
                )
            )
    return res


def parse_task_acs(text: str) -> list[str]:
    """Parse acceptance criteria IDs from a task markdown file."""
    lines = text.splitlines()
    in_ac = False
    result: list[str] = []

    for line in lines:
        if line.strip() == "## Acceptance criteria":
            in_ac = True
            continue
        if in_ac and line.startswith("## "):
            break
        if in_ac and line.startswith("|"):
            parts = [p.strip() for p in line.split("|")]
            # Table row | Cell1 | Cell2 | ... |
            # parts will be ['', 'Cell1', 'Cell2', ..., '']
            if len(parts) >= 3:
                first_cell = parts[1].replace("`", "").strip()
                if not first_cell or first_cell in ("ID", "None"):
                    continue
                if first_cell.startswith("---"):
                    continue
                result.append(first_cell)

    return result


def test_key(known_id: str) -> str:
    """Derive expected test name prefix key from an ID.

    If the ID matches ^V\\d+(\\.\\d+)+$, return 'asvs_' + id.lower().replace('.', '_').
    Otherwise lowercase, replace non-alphanumeric runs with '_', strip '_' at ends.
    """
    if re.match(r"^V\d+(\.\d+)+$", known_id):
        return "asvs_" + known_id.lower().replace(".", "_")
    norm = re.sub(r"[^a-z0-9]+", "_", known_id.lower()).strip("_")
    return norm


def is_task_local_story_ac(raw_id: str) -> bool:
    """True for an `AC` row of a story the spec sources do not define.

    A task may name its own story (T-1101g's `E2E-INFRA AC1`, whose checks are
    proved by `e2e_infra_ac1_*` tests). Such a row supplies its own test key
    from the whole ID; the named test must still exist, so a typo fails exactly
    like a missing test instead of being ignored.

    The story part must carry a word (letters, not only a number), which is what
    distinguishes `E2E-INFRA` from the spec's own `SW-03` style IDs.
    """
    return bool(re.match(r"^[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*-[A-Z][A-Z0-9]* AC\d+[a-z]?$", raw_id))


def rust_test_names(root: Path) -> set[str]:
    """Collect Rust test function names under backend/ (excluding /target/)."""
    names: set[str] = set()
    fn_re = re.compile(r"\bfn\s+([a-z0-9_]+)\s*[<(]")
    backend_dir = root / "backend"
    if not backend_dir.exists():
        return names

    for rs_path in backend_dir.rglob("*.rs"):
        if "/target/" in rs_path.as_posix():
            continue
        try:
            content = rs_path.read_text(encoding="utf-8")
        except Exception:
            continue
        for m in fn_re.finditer(content):
            names.add(m.group(1))

    return names


def dart_test_keys(root: Path) -> set[str]:
    """Collect normalised Dart test descriptions under app/test/ and app/integration_test/."""
    keys: set[str] = set()
    desc_re = re.compile(r"\b(?:test|testWidgets)\(\s*(['\"])(.+?)\1")

    for sub in ("app/test", "app/integration_test"):
        dart_dir = root / sub
        if not dart_dir.exists():
            continue
        for dart_path in dart_dir.rglob("*.dart"):
            try:
                content = dart_path.read_text(encoding="utf-8")
            except Exception:
                continue
            for m in desc_re.finditer(content):
                desc = m.group(2)
                # Normalise with step 7's second branch: lower-case, non-alphanumeric to _
                norm = re.sub(r"[^a-z0-9]+", "_", desc.lower()).strip("_")
                keys.add(norm)

    return keys


def semgrep_rule_ids(root: Path) -> set[str]:
    """Collect Semgrep rule IDs under .semgrep/."""
    rules: set[str] = set()
    semgrep_dir = root / ".semgrep"
    if not semgrep_dir.exists():
        return rules

    rule_re = re.compile(r"^\s*-\s*id:\s*([A-Za-z0-9_.-]+)", re.MULTILINE)
    for yml in semgrep_dir.rglob("*"):
        if yml.suffix in (".yml", ".yaml") and yml.is_file():
            try:
                content = yml.read_text(encoding="utf-8")
            except Exception:
                continue
            for m in rule_re.finditer(content):
                rules.add(m.group(1))

    return rules


def has_test(key: str, rust: set[str], dart: set[str]) -> bool:
    """True when some test name in rust or dart equals key or starts with key + '_'."""
    prefix = key + "_"
    for name in rust:
        if name == key or name.startswith(prefix):
            return True
    for name in dart:
        if name == key or name.startswith(prefix):
            return True
    return False


def main(argv: list[str] | None = None) -> int:
    """CLI entry point. 0 ok, 1 missing coverage, 2 input error."""
    parser = argparse.ArgumentParser(description="Check AC and ASVS test coverage.")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root path (default: parent of tools/)",
    )
    args = parser.parse_args(argv)
    root: Path = args.root.resolve()

    # Required files
    s2_file = root / "docs/specs/S2-v1-acceptance-criteria.md"
    s3_file = root / "docs/specs/S3-domain-model.md"
    s5_file = root / "docs/specs/S5-data-inventory.md"
    s10_file = root / "docs/specs/S10-test-strategy.md"
    asvs_file = root / "docs/security/asvs-l2-register.md"
    enforced_file = root / "tools/ac_coverage_enforced.txt"

    for req in (s2_file, s3_file, s5_file, s10_file, asvs_file, enforced_file):
        if not req.is_file():
            try:
                rel = req.relative_to(root)
            except ValueError:
                rel = req
            print(f"Error: missing required file {rel}", file=sys.stderr)
            return 2

    try:
        s2_list = parse_s2(s2_file.read_text(encoding="utf-8"))
        s3_list = parse_s3_invariants(s3_file.read_text(encoding="utf-8"))
        s5_list = parse_s5_ids(s5_file.read_text(encoding="utf-8"))
        s10_list = parse_s10_ids(s10_file.read_text(encoding="utf-8"))
        asvs_list = parse_register(asvs_file.read_text(encoding="utf-8"))
        enforced_text = enforced_file.read_text(encoding="utf-8")
    except Exception as e:
        print(f"Error reading source files: {e}", file=sys.stderr)
        return 2

    # Build known_ids map
    # Deduplicate keeping one entry (e.g. EXP-1 in S5 and S10)
    known_map: dict[str, KnownId] = {}

    for kid in s2_list:
        known_map[kid.id] = kid
    for kid in s3_list:
        known_map[kid.id] = kid
    for kid in s5_list:
        if kid.id not in known_map:
            known_map[kid.id] = kid
    for kid in s10_list:
        if kid.id not in known_map:
            known_map[kid.id] = kid
    for kid in asvs_list:
        known_map[kid.id] = kid

    # Scan test names and semgrep rules
    rust_names = rust_test_names(root)
    dart_keys = dart_test_keys(root)
    semgrep_rules = semgrep_rule_ids(root)

    # Read enforced tasks
    enforced_ids: list[str] = []
    for line in enforced_text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        enforced_ids.append(line)

    if not enforced_ids:
        # An empty manifest would silently disable this gate (S13 47), so it is
        # an input error, not a pass.
        print(
            "Error: tools/ac_coverage_enforced.txt enforces no tasks; "
            "the AC manifest must not be empty",
            file=sys.stderr,
        )
        return 2

    failures: list[str] = []

    # Check each enforced task
    for task_id in enforced_ids:
        pattern = f"docs/backlog/{task_id}-*.md"
        matches = list(root.glob(pattern))
        if len(matches) != 1:
            print(
                f"Error: expected exactly one match for {pattern}, found {len(matches)}",
                file=sys.stderr,
            )
            return 2

        task_path = matches[0]
        try:
            rel_task_path = task_path.relative_to(root).as_posix()
        except ValueError:
            rel_task_path = task_path.as_posix()

        task_acs = parse_task_acs(task_path.read_text(encoding="utf-8"))
        for raw_id in task_acs:
            # Check if S9 state test name
            if re.match(r"^s9_[a-z0-9_]+$", raw_id):
                # Must exist exactly as Rust name or Dart key prefix (or exact match)
                if not has_test(raw_id, rust_names, dart_keys):
                    failures.append(
                        f"{rel_task_path}: {raw_id}: missing test (expected a name starting {raw_id})"
                    )
                continue

            # Look up known_id
            target_id = raw_id
            kid = known_map.get(target_id)
            if kid is None and target_id.startswith("ASVS "):
                without_prefix = target_id[5:].strip()
                if without_prefix in known_map:
                    target_id = without_prefix
                    kid = known_map.get(target_id)

            if kid is None:
                # A story the spec sources do not define (`E2E-INFRA AC1`,
                # T-1101g): the row carries its own test key, and the test must
                # exist, so a typo fails like a missing test.
                if is_task_local_story_ac(raw_id):
                    key = test_key(raw_id)
                    if not has_test(key, rust_names, dart_keys):
                        failures.append(
                            f"{rel_task_path}: {raw_id}: missing test (expected a name starting {key})"
                        )
                    continue
                failures.append(
                    f"{rel_task_path}: {raw_id}: unknown ID"
                )
                continue

            if not kid.active:
                failures.append(
                    f"{rel_task_path}: {raw_id}: inactive ID"
                )
                continue

            # Verification check
            if kid.source == "ASVS":
                v = kid.verify or "-"
                if v == "test":
                    key = test_key(kid.id)
                    if not has_test(key, rust_names, dart_keys):
                        failures.append(
                            f"{rel_task_path}: {raw_id}: missing test (expected a name starting {key})"
                        )
                elif v == "semgrep":
                    # semgrep rule starting with asvs-<id.lower().replace('.', '-')>-
                    sem_prefix = "asvs-" + kid.id.lower().replace(".", "-") + "-"
                    if not any(r.startswith(sem_prefix) for r in semgrep_rules):
                        failures.append(
                            f"{rel_task_path}: {raw_id}: missing semgrep rule (expected rule ID starting {sem_prefix})"
                        )
                elif v in ("ci", "dast", "review", "-"):
                    pass
                else:
                    # Unknown verify method, treat as test? Or pass?
                    pass
            else:
                key = test_key(kid.id)
                if not has_test(key, rust_names, dart_keys):
                    failures.append(
                        f"{rel_task_path}: {raw_id}: missing test (expected a name starting {key})"
                    )

    # Step 14: Always enforced register rows (status starts with "Verified")
    for kid in asvs_list:
        if kid.status and kid.status.startswith("Verified"):
            v = kid.verify or "-"
            if v == "test":
                key = test_key(kid.id)
                if not has_test(key, rust_names, dart_keys):
                    failures.append(
                        f"docs/security/asvs-l2-register.md: {kid.id}: missing test (expected a name starting {key})"
                    )
            elif v == "semgrep":
                sem_prefix = "asvs-" + kid.id.lower().replace(".", "-") + "-"
                if not any(r.startswith(sem_prefix) for r in semgrep_rules):
                    failures.append(
                        f"docs/security/asvs-l2-register.md: {kid.id}: missing semgrep rule (expected rule ID starting {sem_prefix})"
                    )

    # Build report lines
    report_lines: list[str] = []

    if failures:
        report_lines.extend(failures)
        report_lines.append("")

    # S2 coverage by story
    # Group active S2 ACs by story
    s2_stories: dict[str, list[KnownId]] = {}
    for kid in s2_list:
        if not kid.active:
            continue
        if " " in kid.id:
            story = kid.id.split(" ", 1)[0]
        else:
            story = "Cross-cutting"
        s2_stories.setdefault(story, []).append(kid)

    report_lines.append("## Acceptance Criteria Coverage (S2)")
    header = f"| {'Story':<14} | {'Active ACs':<10} | {'Covered':<8} | {'Uncovered AC IDs'} |"
    sep = f"|{'-' * 16}|{'-' * 12}|{'-' * 10}|{'-' * 20}|"
    report_lines.append(header)
    report_lines.append(sep)

    total_s2_active = 0
    total_s2_covered = 0

    for story in sorted(s2_stories.keys()):
        acs = s2_stories[story]
        active_count = len(acs)
        total_s2_active += active_count
        covered_count = 0
        uncovered: list[str] = []
        for kid in acs:
            key = test_key(kid.id)
            if has_test(key, rust_names, dart_keys):
                covered_count += 1
            else:
                uncovered.append(kid.id)
        total_s2_covered += covered_count
        uncovered_str = ", ".join(uncovered) if uncovered else "-"
        report_lines.append(
            f"| {story:<14} | {active_count:<10} | {covered_count:<8} | {uncovered_str} |"
        )

    report_lines.append("")

    # Totals
    # INV covered of 7
    inv_covered = 0
    inv_total = len(s3_list)
    for kid in s3_list:
        key = test_key(kid.id)
        if has_test(key, rust_names, dart_keys):
            inv_covered += 1

    # Register 'test' rows covered of their total
    reg_test_rows = [kid for kid in asvs_list if kid.verify == "test"]
    reg_test_covered = 0
    for kid in reg_test_rows:
        key = test_key(kid.id)
        if has_test(key, rust_names, dart_keys):
            reg_test_covered += 1

    report_lines.append("## Totals")
    report_lines.append(
        f"- S2 ACs covered: {total_s2_covered} / {total_s2_active}"
    )
    report_lines.append(f"- Invariants (INV) covered: {inv_covered} / {inv_total}")
    report_lines.append(
        f"- ASVS register 'test' rows covered: {reg_test_covered} / {len(reg_test_rows)}"
    )

    report_text = "\n".join(report_lines)
    print(report_text)

    # Step summary output
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        try:
            with open(step_summary, "a", encoding="utf-8") as f:
                f.write(report_text + "\n")
        except Exception as e:
            sys.stderr.write(f"Warning: could not write to GITHUB_STEP_SUMMARY: {e}\n")

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
