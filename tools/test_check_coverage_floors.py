"""Tests for check_coverage_floors.py."""

from __future__ import annotations

import json
import os
import tempfile
import unittest

from tools.check_coverage_floors import (
    Totals,
    check_minimums,
    flutter_totals,
    main,
    parse_lcov,
    rust_area_totals,
    rust_crate_of,
)


class TestCheckCoverageFloors(unittest.TestCase):
    def test_parse_lcov_lf_lh(self):
        sample = """SF:/path/to/file.rs
DA:1,1
DA:2,0
LF:10
LH:8
end_of_record
"""
        res = parse_lcov(sample)
        self.assertIn("/path/to/file.rs", res)
        self.assertEqual(res["/path/to/file.rs"].found, 10)
        self.assertEqual(res["/path/to/file.rs"].hit, 8)
        self.assertEqual(res["/path/to/file.rs"].percent(), 80.0)

    def test_parse_lcov_counts_da_when_lf_missing(self):
        sample = """SF:/path/to/file.rs
DA:1,2
DA:2,0
DA:3,1
end_of_record
"""
        res = parse_lcov(sample)
        self.assertIn("/path/to/file.rs", res)
        self.assertEqual(res["/path/to/file.rs"].found, 3)
        self.assertEqual(res["/path/to/file.rs"].hit, 2)
        self.assertAlmostEqual(res["/path/to/file.rs"].percent(), 66.6666667, places=4)

    def test_rust_crate_of_absolute_path(self):
        path1 = "/home/ubuntu/MailTinder/backend/crates/domain/src/lib.rs"
        self.assertEqual(rust_crate_of(path1), "domain")

        path2 = "crates/adapters-gmail/src/sub/mod.rs"
        self.assertEqual(rust_crate_of(path2), "adapters-gmail")

        path3 = "/rustc/xyz/library/std/src/io/mod.rs"
        self.assertIsNone(rust_crate_of(path3))

    def test_domain_below_90_fails(self):
        cfg = {
            "rust": {
                "domain": 90,
                "default": 75,
                "exclude_crates": ["testkit"],
                "exclude_files": ["src/main.rs"],
            },
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        files = {
            "/backend/crates/domain/src/lib.rs": Totals(found=100, hit=89),  # 89% < 90%
        }
        res = rust_area_totals(files, cfg)
        self.assertEqual(res["domain"].found, 100)
        self.assertEqual(res["domain"].hit, 89)
        self.assertLess(res["domain"].percent(), 90.0)

        # Also test via main
        with tempfile.TemporaryDirectory() as tmp:
            cfg_path = os.path.join(tmp, "floors.json")
            with open(cfg_path, "w") as f:
                json.dump(cfg, f)

            rust_lcov = os.path.join(tmp, "rust.info")
            with open(rust_lcov, "w") as f:
                f.write(
                    "SF:/backend/crates/domain/src/lib.rs\nLF:100\nLH:89\nend_of_record\n"
                )

            flutter_lcov = os.path.join(tmp, "flutter.info")
            with open(flutter_lcov, "w") as f:
                f.write("SF:lib/app.dart\nLF:100\nLH:80\nend_of_record\n")

            code = main(
                ["--rust", rust_lcov, "--flutter", flutter_lcov, "--floors", cfg_path]
            )
            self.assertEqual(code, 1)

    def test_other_crate_at_75_passes(self):
        cfg = {
            "rust": {
                "domain": 90,
                "default": 75,
                "exclude_crates": ["testkit"],
                "exclude_files": ["src/main.rs"],
            },
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        with tempfile.TemporaryDirectory() as tmp:
            cfg_path = os.path.join(tmp, "floors.json")
            with open(cfg_path, "w") as f:
                json.dump(cfg, f)

            rust_lcov = os.path.join(tmp, "rust.info")
            with open(rust_lcov, "w") as f:
                f.write(
                    "SF:/backend/crates/ports/src/lib.rs\nLF:100\nLH:75\nend_of_record\n"
                    "SF:/backend/crates/domain/src/lib.rs\nLF:100\nLH:90\nend_of_record\n"
                )

            flutter_lcov = os.path.join(tmp, "flutter.info")
            with open(flutter_lcov, "w") as f:
                f.write("SF:lib/app.dart\nLF:100\nLH:75\nend_of_record\n")

            code = main(
                ["--rust", rust_lcov, "--flutter", flutter_lcov, "--floors", cfg_path]
            )
            self.assertEqual(code, 0)

    def test_testkit_and_fakes_excluded(self):
        cfg = {
            "rust": {
                "domain": 90,
                "default": 75,
                "exclude_crates": ["testkit", "fake-google", "unsub-testbed"],
                "exclude_files": ["src/main.rs"],
            },
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        files = {
            "/backend/crates/testkit/src/lib.rs": Totals(found=50, hit=10),
            "/backend/crates/fake-google/src/lib.rs": Totals(found=50, hit=10),
            "/backend/crates/unsub-testbed/src/lib.rs": Totals(found=50, hit=10),
            "/backend/crates/domain/src/lib.rs": Totals(found=10, hit=10),
        }
        res = rust_area_totals(files, cfg)
        self.assertNotIn("testkit", res)
        self.assertNotIn("fake-google", res)
        self.assertNotIn("unsub-testbed", res)
        self.assertIn("domain", res)

    def test_main_rs_excluded(self):
        cfg = {
            "rust": {
                "domain": 90,
                "default": 75,
                "exclude_crates": ["testkit"],
                "exclude_files": ["src/main.rs"],
            },
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        files = {
            "/backend/crates/api/src/main.rs": Totals(found=100, hit=0),
            "/backend/crates/api/src/lib.rs": Totals(found=10, hit=8),
        }
        res = rust_area_totals(files, cfg)
        self.assertEqual(res["api"].found, 10)
        self.assertEqual(res["api"].hit, 8)

    def test_crate_with_no_lines_passes(self):
        cfg = {
            "rust": {
                "domain": 90,
                "default": 75,
                "exclude_crates": ["testkit"],
                "exclude_files": ["src/main.rs"],
            },
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        files = {
            "/backend/crates/domain/src/empty.rs": Totals(found=0, hit=0),
        }
        res = rust_area_totals(files, cfg)
        self.assertIn("domain", res)
        self.assertEqual(res["domain"].found, 0)
        self.assertIsNone(res["domain"].percent())

        # Main returns 0 when area has 0 found lines
        with tempfile.TemporaryDirectory() as tmp:
            cfg_path = os.path.join(tmp, "floors.json")
            with open(cfg_path, "w") as f:
                json.dump(cfg, f)

            rust_lcov = os.path.join(tmp, "rust.info")
            with open(rust_lcov, "w") as f:
                f.write(
                    "SF:/backend/crates/domain/src/empty.rs\nLF:0\nLH:0\nend_of_record\n"
                )

            flutter_lcov = os.path.join(tmp, "flutter.info")
            with open(flutter_lcov, "w") as f:
                f.write("SF:lib/empty.dart\nLF:0\nLH:0\nend_of_record\n")

            code = main(
                ["--rust", rust_lcov, "--flutter", flutter_lcov, "--floors", cfg_path]
            )
            self.assertEqual(code, 0)

    def test_flutter_main_dart_excluded(self):
        cfg = {
            "rust": {"domain": 90, "default": 75, "exclude_crates": [], "exclude_files": []},
            "flutter": {"lib": 75, "exclude_files": ["lib/main.dart"]},
        }
        files = {
            "lib/main.dart": Totals(found=50, hit=0),
            "lib/widget.dart": Totals(found=20, hit=18),
            "test/widget_test.dart": Totals(found=100, hit=100),  # Not in lib/
        }
        tot = flutter_totals(files, cfg)
        self.assertEqual(tot.found, 20)
        self.assertEqual(tot.hit, 18)
        self.assertEqual(tot.percent(), 90.0)

    def test_floor_below_minimum_is_error(self):
        bad_cfg1 = {
            "rust": {"domain": 89, "default": 75},
            "flutter": {"lib": 75},
        }
        errs1 = check_minimums(bad_cfg1)
        self.assertTrue(any("domain" in e for e in errs1))

        bad_cfg2 = {
            "rust": {"domain": 90, "default": 74},
            "flutter": {"lib": 75},
        }
        errs2 = check_minimums(bad_cfg2)
        self.assertTrue(any("default" in e for e in errs2))

        bad_cfg3 = {
            "rust": {"domain": 90, "default": 75},
            "flutter": {"lib": 70},
        }
        errs3 = check_minimums(bad_cfg3)
        self.assertTrue(any("flutter" in e for e in errs3))

        with tempfile.TemporaryDirectory() as tmp:
            cfg_path = os.path.join(tmp, "floors.json")
            with open(cfg_path, "w") as f:
                json.dump(bad_cfg1, f)

            rust_lcov = os.path.join(tmp, "rust.info")
            with open(rust_lcov, "w") as f:
                f.write("SF:/backend/crates/domain/src/lib.rs\nLF:10\nLH:10\nend_of_record\n")

            flutter_lcov = os.path.join(tmp, "flutter.info")
            with open(flutter_lcov, "w") as f:
                f.write("SF:lib/app.dart\nLF:10\nLH:10\nend_of_record\n")

            code = main(
                ["--rust", rust_lcov, "--flutter", flutter_lcov, "--floors", cfg_path]
            )
            self.assertEqual(code, 1)

    def test_missing_lcov_exit_2(self):
        code = main(
            [
                "--rust",
                "/nonexistent/rust.info",
                "--flutter",
                "/nonexistent/flutter.info",
            ]
        )
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
