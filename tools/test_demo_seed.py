"""Tests for scripts/demo_seed.py (T-1108b; security-review fix).

`demo_seed.py` POSTs to a local fake stack, so it must refuse any URL that is
not loopback http before the request is built. These tests run in the ordinary
`python3 -m unittest discover -s tools` gate (the `ac-coverage` CI job) and need
no stack.
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_demo_seed():
    """Load scripts/demo_seed.py, which is a script, not an importable module."""
    spec = importlib.util.spec_from_file_location(
        "demo_seed", ROOT / "scripts" / "demo_seed.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


demo_seed = load_demo_seed()


class DemoSeedUrlValidationTest(unittest.TestCase):
    def test_demo_seed_refuses_non_loopback_url(self):
        # Exercised through post_json, the single choke point every request goes
        # through: removing the validate_url call there makes these calls raise
        # ConnectionError instead of ValueError, and the test fails.
        refused = (
            # A file URL is the classic urlopen footgun (semgrep
            # python.lang.security.audit.dynamic-urllib-use-detected).
            "file:///etc/passwd",
            "ftp://127.0.0.1/",
            # Remote hosts, by name and by address.
            "http://example.com/",
            "https://example.com/",
            "http://10.0.0.1/",
            "http://192.168.1.5/",
            "http://[::2]/",
            # A name that merely starts with `127.` is not loopback.
            "http://127.evil.example/",
        )
        for url in refused:
            with self.subTest(url=url):
                with self.assertRaises(ValueError):
                    demo_seed.post_json(url, {})

    def test_demo_seed_accepts_loopback_url(self):
        for url in (
            "http://127.0.0.1:8086/__fake/reset",
            "http://localhost:8080",
            "http://[::1]:9000/api",
        ):
            with self.subTest(url=url):
                self.assertEqual(demo_seed.validate_url(url), url)


if __name__ == "__main__":
    unittest.main()
