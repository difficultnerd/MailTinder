"""Tests for tools/check_no_binaries.py (binary policy). Each test fails if the rule it names is removed."""
import os
import subprocess
import sys
import tempfile
import unittest

CHECK = os.path.join(os.path.dirname(__file__), "check_no_binaries.py")


class BinaryPolicyTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = self.tmp.name
        self.git("init", "-q")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "t")

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *a):
        subprocess.run(["git", "-C", self.repo, *a], check=True, capture_output=True)

    def put(self, rel, data):
        p = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
        self.git("add", "-f", rel)

    def run_check(self, mode):
        r = subprocess.run([sys.executable, CHECK, mode, "--repo", self.repo], capture_output=True, text=True)
        return r.returncode, r.stdout

    def test_clean_repo_passes(self):
        self.put("src/main.rs", b"fn main() {}\n")
        self.put("app/web/icon.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
        for mode in ("--tree", "--staged"):
            self.assertEqual(self.run_check(mode)[0], 0, mode)

    def test_elf_executable_is_rejected(self):
        self.put("tools/bin/tool", b"\x7fELF" + b"\0" * 64)
        for mode in ("--tree", "--staged"):
            rc, out = self.run_check(mode)
            self.assertEqual(rc, 1, mode)
            self.assertIn("compiled executable", out)

    def test_unknown_binary_type_is_rejected(self):
        self.put("data/blob.dat", b"abc\0def")
        rc, out = self.run_check("--tree")
        self.assertEqual(rc, 1)
        self.assertIn("not an allowed asset type", out)

    def test_hidden_tool_directory_is_rejected_even_for_text(self):
        self.put("tools/.cargo-mutants/.crates.toml", b"[v1]\n")
        rc, out = self.run_check("--tree")
        self.assertEqual(rc, 1)
        self.assertIn("tool/install/cache directory", out)

    def test_over_one_megabyte_is_rejected_but_lock_files_are_not(self):
        self.put("docs/huge.txt", b"a" * (1024 * 1024 + 1))
        rc, out = self.run_check("--tree")
        self.assertEqual(rc, 1)
        self.assertIn("over 1 MB", out)
        self.git("rm", "-q", "-f", "docs/huge.txt")
        self.put("backend/Cargo.lock", b"a" * (1024 * 1024 + 1))
        self.assertEqual(self.run_check("--tree")[0], 0)

    def test_allowlist_exempts_a_reviewed_path(self):
        self.put("vendor/tool", b"\x7fELF" + b"\0" * 8)
        self.assertEqual(self.run_check("--tree")[0], 1)
        self.put("tools/binary_allowlist.txt", b"# reviewed exception\nvendor/tool\n")
        self.assertEqual(self.run_check("--tree")[0], 0)

    def test_staged_mode_only_looks_at_staged_files(self):
        self.put("ok.txt", b"hello\n")
        self.git("commit", "-q", "-m", "base")
        self.put("tools/bin/tool", b"\x7fELF" + b"\0" * 8)
        self.assertEqual(self.run_check("--staged")[0], 1)
        self.git("reset", "-q", "tools/bin/tool")
        self.assertEqual(self.run_check("--staged")[0], 0)


if __name__ == "__main__":
    unittest.main()
