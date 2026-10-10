"""Tests for scripts/demo.sh and the shared scripts/e2e/lib.sh (T-1108a).

`demo_refuses_non_loopback_bind` needs nothing but bash and the script, so it
runs in the ordinary `python3 -m unittest discover -s tools` gate (the
`ac-coverage` CI job).

The other two start the whole local e2e stack (Firestore emulator, fake-google,
unsub-testbed, `api`, the Flutter web build served by scripts/e2e_host.py), so
they run under `MT_DEMO_STACK=1`, the same way the e2e crate's journeys are
`#[ignore]`d. `scripts/e2e.sh` - the `e2e` CI job, the only job with the stack -
runs `demo_check_serves_page_and_health_then_cleans_up` directly (security review
F5), so that AC is exercised on every pull request. `e2e_sh_still_passes_unchanged`
is proven by that job itself: it *is* a run of `scripts/e2e.sh` whose journeys
must pass and which ends with "e2e passed"; the named test here is for running it
locally (`MT_DEMO_STACK=1`), and it cannot be run from inside e2e.sh without
recursing.
"""

from __future__ import annotations

import os
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEMO = ROOT / "scripts" / "demo.sh"
E2E = ROOT / "scripts" / "e2e.sh"

STACK = os.environ.get("MT_DEMO_STACK") == "1"


def run_demo(args, env=None, timeout=180):
    """Run scripts/demo.sh and return the CompletedProcess."""
    environment = dict(os.environ)
    if env:
        environment.update(env)
    return subprocess.run(
        ["bash", str(DEMO), *args],
        cwd=str(ROOT),
        env=environment,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


class DemoScriptTest(unittest.TestCase):
    def test_demo_refuses_non_loopback_bind(self):
        hosts = (
            "0.0.0.0",
            "192.168.1.5",
            "10.0.0.1",
            "example.com",
            # Names that merely start with `127.` must not slip past the guard
            # (T-1108a F1): the old `127.*` glob accepted these.
            "127.evil.example",
            "127.0.0.1.evil.example",
            # A dotted-quad with an out-of-range octet is not loopback either.
            "127.0.0.256",
        )
        for host in hosts:
            with self.subTest(host=host):
                proc = run_demo(["start", "--host", host])
                combined = (proc.stdout + proc.stderr).lower()
                self.assertNotEqual(proc.returncode, 0, combined)
                self.assertIn("non-loopback", combined)
        # The environment override is refused the same way.
        proc = run_demo(["start"], env={"DEMO_HOST": "0.0.0.0"})
        combined = (proc.stdout + proc.stderr).lower()
        self.assertNotEqual(proc.returncode, 0, combined)
        self.assertIn("non-loopback", combined)

    @unittest.skipUnless(STACK, "needs the full demo stack (MT_DEMO_STACK=1)")
    def test_demo_check_serves_page_and_health_then_cleans_up(self):
        proc = run_demo(["--check"])
        combined = proc.stdout + proc.stderr
        self.assertEqual(proc.returncode, 0, combined)
        self.assertIn("leak census: nothing left", combined)
        self.assertFalse((ROOT / "target/demo/state.json").exists())
        # status agrees the stack is down afterwards.
        status = run_demo(["status"])
        self.assertNotEqual(status.returncode, 0)

    @unittest.skipUnless(STACK, "needs the full demo stack (MT_DEMO_STACK=1)")
    def test_e2e_sh_still_passes_unchanged(self):
        # The start/stop helpers moved into scripts/e2e/lib.sh; this proves
        # e2e.sh still brings up the stack and runs a journey.
        proc = subprocess.run(
            ["bash", str(E2E), "--journey", "sign_in"],
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            timeout=1800,
        )
        self.assertEqual(
            proc.returncode, 0, proc.stdout[-4000:] + proc.stderr[-4000:]
        )
        self.assertIn("e2e passed", proc.stdout)


if __name__ == "__main__":
    unittest.main()
