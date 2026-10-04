"""Tests for check_ac_coverage.py."""

from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

from tools.check_ac_coverage import (
    KnownId,
    dart_test_keys,
    has_test,
    main,
    parse_register,
    parse_s2,
    parse_s3_invariants,
    parse_s5_ids,
    parse_s10_ids,
    parse_task_acs,
    rust_test_names,
    semgrep_rule_ids,
    test_key,
)


class TestCheckAcCoverage(unittest.TestCase):
    def test_parse_s2_story_and_ac_ids(self):
        text = """
### AU-01 Invite
- **AC1.** First AC.
- **AC2.** Second AC.

### AU-03 Sign in
- **AC4a.** Suffix letter.

## Cross-cutting
- **XC-01.** Cross cutting rule.
"""
        res = parse_s2(text)
        id_map = {item.id: item for item in res}
        self.assertIn("AU-01 AC1", id_map)
        self.assertIn("AU-03 AC4a", id_map)
        self.assertIn("XC-01", id_map)
        self.assertTrue(id_map["AU-01 AC1"].active)
        self.assertTrue(id_map["AU-03 AC4a"].active)
        self.assertTrue(id_map["XC-01"].active)

    def test_parse_s2_removed_and_v2_are_inactive(self):
        text = """
### AU-03 Sign in
- **AC4a.** Removed (passkey lock dropped).
- **AC4b.** v2. Some feature for later.
- **AC5.** Active AC.
"""
        res = parse_s2(text)
        id_map = {item.id: item for item in res}
        self.assertFalse(id_map["AU-03 AC4a"].active)
        self.assertFalse(id_map["AU-03 AC4b"].active)
        self.assertTrue(id_map["AU-03 AC5"].active)

    def test_parse_s3_invariants(self):
        text = """
- **INV-1.** Body not saved.
- **INV-5.** No delete.
- **INV-7.** Domain pure.
"""
        res = parse_s3_invariants(text)
        ids = [item.id for item in res]
        self.assertEqual(ids, ["INV-1", "INV-5", "INV-7"])
        for item in res:
            self.assertTrue(item.active)
            self.assertEqual(item.source, "S3")

    def test_parse_s5_ids_include_inv_t_and_na_t(self):
        text = """
| DEL-1 | deletion |
| SES-1 | session |
| CFG-1 | config |
| JOB-1 | jobs |
| RL-1 | rate limits |
| LOG-1 | logging |
| EXP-1 | eval |
| JEV-1 | jev |
| INV-T1 | invite retention |
| NA-T1 | needs attention retention |
"""
        res = parse_s5_ids(text)
        ids = [item.id for item in res]
        self.assertIn("INV-T1", ids)
        self.assertIn("NA-T1", ids)
        self.assertIn("DEL-1", ids)
        self.assertIn("SES-1", ids)
        self.assertEqual(len(ids), 10)

    def test_parse_s10_classifier_ids(self):
        text = """
The tests are JEV-1, BAKE-2, EXP-1, GUARD-1, and STAT-1.
"""
        res = parse_s10_ids(text)
        ids = [item.id for item in res]
        self.assertEqual(ids, ["BAKE-2", "EXP-1", "GUARD-1", "JEV-1", "STAT-1"])

    def test_parse_register_verify_and_status(self):
        text = """
| V1.1.1 | 2 | Req 1 | Control 1 | backend/ | test `asvs_v1_1_1_*` | Planned |
| V1.2.3 | 1 | Req 2 | Control 2 | backend/ | semgrep | Planned |
| V1.4.1 | 2 | Req 3 | Control 3 | backend/ | ci | Verified |
| V1.2.6 | 2 | Req 4 | Control 4 | - | - | N/A |
"""
        res = parse_register(text)
        self.assertEqual(len(res), 4)
        self.assertEqual(res[0].id, "V1.1.1")
        self.assertEqual(res[0].verify, "test")
        self.assertEqual(res[0].status, "Planned")

        self.assertEqual(res[1].id, "V1.2.3")
        self.assertEqual(res[1].verify, "semgrep")

        self.assertEqual(res[2].id, "V1.4.1")
        self.assertEqual(res[2].verify, "ci")
        self.assertEqual(res[2].status, "Verified")

        self.assertEqual(res[3].id, "V1.2.6")
        self.assertEqual(res[3].verify, "-")
        self.assertEqual(res[3].status, "N/A")

    def test_test_key_examples(self):
        self.assertEqual(test_key("SW-03 AC2"), "sw_03_ac2")
        self.assertEqual(test_key("SW-05 AC4a"), "sw_05_ac4a")
        self.assertEqual(test_key("V3.4.3"), "asvs_v3_4_3")
        self.assertEqual(test_key("INV-T1"), "inv_t1")
        self.assertEqual(test_key("GUARD-1"), "guard_1")
        self.assertEqual(test_key("XC-01"), "xc_01")

    def test_rust_names_found_in_proptest_block(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            rs_dir = tmp_root / "backend/crates/domain/tests"
            rs_dir.mkdir(parents=True)
            rs_file = rs_dir / "prop_test.rs"
            rs_file.write_text(
                """
fn regular_test_fn() {}

proptest! {
    #[test]
    fn guard_1_no_list_without_valid_header(val in 0..10) {
        // ...
    }
}
"""
            )
            # Create a fake target directory that should be ignored
            target_dir = tmp_root / "backend/target/debug"
            target_dir.mkdir(parents=True)
            (target_dir / "ignored.rs").write_text("fn ignored_fn() {}")

            names = rust_test_names(tmp_root)
            self.assertIn("regular_test_fn", names)
            self.assertIn("guard_1_no_list_without_valid_header", names)
            self.assertNotIn("ignored_fn", names)

    def test_dart_description_normalised(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            dart_dir = tmp_root / "app/test"
            dart_dir.mkdir(parents=True)
            dart_file = dart_dir / "sample_test.dart"
            dart_file.write_text(
                """
void main() {
  test('SW-03 AC2 reject toast states the delay', () {});
  testWidgets("ASVS V14.3.1 wipes state on 401", (tester) async {});
}
"""
            )
            keys = dart_test_keys(tmp_root)
            self.assertIn(
                "sw_03_ac2_reject_toast_states_the_delay",
                keys,
            )
            self.assertIn(
                "asvs_v14_3_1_wipes_state_on_401",
                keys,
            )

    def test_prefix_match_needs_underscore_boundary(self):
        rust = {"sw_03_ac21_x", "inv_51"}
        dart = set()
        # SW-03 AC2 -> sw_03_ac2; must not match sw_03_ac21_x
        self.assertFalse(has_test("sw_03_ac2", rust, dart))
        # INV-5 -> inv_5; must not match inv_51
        self.assertFalse(has_test("inv_5", rust, dart))

        # But with proper underscore boundary, it matches
        rust_valid = {"sw_03_ac2_valid", "inv_5"}
        self.assertTrue(has_test("sw_03_ac2", rust_valid, dart))
        self.assertTrue(has_test("inv_5", rust_valid, dart))

    def _setup_tree(self, tmp_root: Path):
        """Helper to scaffold a minimal valid repo tree."""
        docs = tmp_root / "docs/specs"
        docs.mkdir(parents=True)
        (docs / "S2-v1-acceptance-criteria.md").write_text(
            """### AU-01 Invite
- **AC1.** Valid invite.
- **AC2.** Removed.
"""
        )
        (docs / "S3-domain-model.md").write_text("- **INV-1.** Invariant.\n")
        (docs / "S5-data-inventory.md").write_text("| DEL-1 | deletion |\n")
        (docs / "S10-test-strategy.md").write_text("GUARD-1 test.\n")

        sec = tmp_root / "docs/security"
        sec.mkdir(parents=True)
        (sec / "asvs-l2-register.md").write_text(
            "| V1.1.1 | 2 | Req | Control | backend/ | test | Planned |\n"
        )

        tools = tmp_root / "tools"
        tools.mkdir(parents=True)
        (tools / "ac_coverage_enforced.txt").write_text("T-101\n")

        backlog = tmp_root / "docs/backlog"
        backlog.mkdir(parents=True)

    def test_enforced_task_missing_test_fails_with_exit_1(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| AU-01 AC1 | Something |
"""
            )
            # No rust/dart tests exist
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

    def test_unenforced_task_missing_test_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            # T-101 is enforced and has a test
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| AU-01 AC1 | Something |
"""
            )
            # T-102 is NOT in ac_coverage_enforced.txt, and has no test
            (tmp_root / "docs/backlog/T-102-sample.md").write_text(
                """# T-102
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| DEL-1 | Something |
"""
            )
            # Provide test for T-101
            rs_dir = tmp_root / "backend/src"
            rs_dir.mkdir(parents=True)
            (rs_dir / "lib.rs").write_text("fn au_01_ac1_test() {}\n")

            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 0)

    def test_unknown_id_in_task_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| UNKNOWN-99 | Something |
"""
            )
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

    def test_inactive_id_in_task_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| AU-01 AC2 | Something inactive |
"""
            )
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

    def test_semgrep_row_needs_rule_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            # Add semgrep row to register
            (tmp_root / "docs/security/asvs-l2-register.md").write_text(
                "| V1.2.3 | 1 | Req | Control | backend/ | semgrep | Planned |\n"
            )
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| V1.2.3 | Something |
"""
            )
            # Missing semgrep rule fails
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

            # Provide semgrep rule
            sem = tmp_root / ".semgrep"
            sem.mkdir(parents=True)
            (sem / "rules.yml").write_text(
                """rules:
  - id: asvs-v1-2-3-no-string-json
    message: ...
"""
            )
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 0)

    def test_verified_register_row_always_enforced(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            # Task T-101 has no ACs
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| None | None |
"""
            )
            # Register has a Verified row with verify=test
            (tmp_root / "docs/security/asvs-l2-register.md").write_text(
                "| V1.1.1 | 2 | Req | Control | backend/ | test | Verified |\n"
            )
            # No test exists -> fails
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

            # Add test for V1.1.1
            rs_dir = tmp_root / "backend/src"
            rs_dir.mkdir(parents=True)
            (rs_dir / "lib.rs").write_text("fn asvs_v1_1_1_test() {}\n")

            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 0)

    def test_s9_state_name_needs_exact_test(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| s9_feed_offline_banner | S9 state |
"""
            )
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 1)

            # Add test matching s9_feed_offline_banner
            rs_dir = tmp_root / "backend/src"
            rs_dir.mkdir(parents=True)
            (rs_dir / "lib.rs").write_text("fn s9_feed_offline_banner() {}\n")

            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 0)

    def test_missing_source_file_exit_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            # Empty dir -> missing S2 file
            code = main(["--root", str(tmp_root)])
            self.assertEqual(code, 2)

    def test_report_written_to_step_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            self._setup_tree(tmp_root)
            (tmp_root / "docs/backlog/T-101-sample.md").write_text(
                """# T-101
## Acceptance criteria
| ID | Behaviour |
| --- | --- |
| None | None |
"""
            )
            summary_file = tmp_root / "step_summary.md"
            import os

            old_val = os.environ.get("GITHUB_STEP_SUMMARY")
            try:
                os.environ["GITHUB_STEP_SUMMARY"] = str(summary_file)
                code = main(["--root", str(tmp_root)])
                self.assertEqual(code, 0)
                self.assertTrue(summary_file.is_file())
                content = summary_file.read_text()
                self.assertIn("## Acceptance Criteria Coverage (S2)", content)
                self.assertIn("## Totals", content)
            finally:
                if old_val is None:
                    os.environ.pop("GITHUB_STEP_SUMMARY", None)
                else:
                    os.environ["GITHUB_STEP_SUMMARY"] = old_val


if __name__ == "__main__":
    unittest.main()
