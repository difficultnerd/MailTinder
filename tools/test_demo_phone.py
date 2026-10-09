"""Tests for `scripts/demo.sh --phone` and the phone-mode front door (T-1108c).

The access-control, fake-google-proxy and viewport tests drive
`scripts/e2e_host.py` directly (stdlib only, no stack), so they run in the
ordinary `python3 -m unittest discover -s tools` gate (the `ac-coverage` CI
job). The `demo.sh` guard tests run the script's phone pre-flight only: a bad
cloudflared or a non-loopback tunnel target is refused before anything is
built or started, so they are fast too.

`DemoPhoneStackTest` starts the whole local stack and a STUB cloudflared, so it
runs only under `MT_DEMO_STACK=1` (like the T-1108a stack tests); `scripts/e2e.sh`
runs it directly in the `e2e` CI job, the only job with the stack, so the stub
tunnel is exercised on every pull request without a real tunnel.
"""

from __future__ import annotations

import base64
import hashlib
import http.client
import http.server
import json
import os
import platform
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEMO = ROOT / "scripts" / "demo.sh"
E2E_HOST = ROOT / "scripts" / "e2e_host.py"
STACK = os.environ.get("MT_DEMO_STACK") == "1"

CODE = "Abcdefgh12345678"
# 8.8.8.8 is a non-loopback literal; the tun/example hosts are not loopback.
NON_LOOPBACK_TARGETS = ("http://192.168.1.9:1234", "https://evil.example/")

_ARCHES = {
    "x86_64": "linux-amd64",
    "amd64": "linux-amd64",
    "aarch64": "linux-arm64",
    "arm64": "linux-arm64",
}
ARCH = _ARCHES.get(platform.machine(), "")


def auth_header(code: str, user: str = "demo") -> str:
    token = base64.b64encode(f"{user}:{code}".encode()).decode()
    return f"Basic {token}"


def http_get(port: int, path: str, auth: str | None = None, timeout: float = 5.0):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    headers = {"Authorization": auth} if auth else {}
    conn.request("GET", path, headers=headers)
    response = conn.getresponse()
    body = response.read()
    conn.close()
    return response.status, body


class FakeGoogle(socketserver.ThreadingTCPServer):
    """A minimal fake-google that records the paths it is asked for."""

    allow_reuse_address = True

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), _FakeGoogleHandler)
        self.paths: list[str] = []

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server_address[1]}"


class _FakeGoogleHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        self.server.paths.append(self.path)  # type: ignore[attr-defined]
        body = b"authorise-ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        pass


class _HostServer:
    """Start scripts/e2e_host.py on a temp root and expose its port."""

    def __init__(self, root: Path, extra: list[str], index: str | None = None) -> None:
        web = root / "web"
        web.mkdir(parents=True, exist_ok=True)
        (web / "index.html").write_text(
            index or "<!DOCTYPE html><html><head></head><body>hi</body></html>"
        )
        firebase = root / "firebase.json"
        firebase.write_text(json.dumps({"hosting": {"headers": []}}))
        port_file = root / "host.port"
        args = [
            sys.executable,
            str(E2E_HOST),
            "--root",
            str(web),
            "--firebase-json",
            str(firebase),
            "--api",
            "http://127.0.0.1:1",
            "--port",
            "0",
            "--port-file",
            str(port_file),
            *extra,
        ]
        self.proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.port = 0
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if port_file.is_file() and port_file.read_text().strip():
                self.port = int(port_file.read_text().strip())
                break
            time.sleep(0.05)
        if not self.port:
            self.proc.kill()
            raise RuntimeError("e2e_host.py did not report a port")

    def close(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=5)
        for stream in (self.proc.stdout, self.proc.stderr):
            if stream is not None:
                stream.close()


def write_code_file(root: Path, code: str = CODE) -> Path:
    path = root / "access-code"
    path.write_text(code + "\n")
    os.chmod(path, 0o600)
    return path


class E2eHostPhoneTest(unittest.TestCase):
    """Behaviour 2/3 and 0d, driven straight at the front door."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_demo_phone_requires_access_code(self) -> None:
        code_file = write_code_file(self.tmp)
        host = _HostServer(self.tmp, ["--access-code-file", str(code_file)])
        self.addCleanup(host.close)

        status, _ = http_get(host.port, "/")
        self.assertEqual(status, 401, "a request without the code must be refused")
        status, body = http_get(host.port, "/", auth=auth_header(CODE))
        self.assertEqual(status, 200, "with the code the page is served")
        self.assertIn(b"<html", body.lower())

    def test_demo_page_has_viewport_meta(self) -> None:
        # The temp index.html deliberately has no viewport meta; the front door
        # must add it so the demo is usable on a phone (behaviour 3).
        host = _HostServer(self.tmp, [])
        self.addCleanup(host.close)
        status, body = http_get(host.port, "/")
        self.assertEqual(status, 200)
        self.assertIn(b'name="viewport"', body)

    def test_demo_phone_fake_google_proxy_allows_only_authorise(self) -> None:
        fake = FakeGoogle()
        self.addCleanup(fake.shutdown)
        self.addCleanup(fake.server_close)
        threading.Thread(target=fake.serve_forever, daemon=True).start()
        code_file = write_code_file(self.tmp)
        host = _HostServer(
            self.tmp,
            ["--access-code-file", str(code_file), "--fake-google", fake.base_url],
        )
        self.addCleanup(host.close)
        auth = auth_header(CODE)

        status, _ = http_get(host.port, "/fake-google/o/oauth2/v2/auth")
        self.assertEqual(status, 401, "the authorise path is behind the code too")

        status, body = http_get(
            host.port, "/fake-google/o/oauth2/v2/auth?client_id=x&state=y", auth=auth
        )
        self.assertEqual(status, 200, "the one authorise path reaches fake-google")
        self.assertEqual(body, b"authorise-ok")
        self.assertEqual(
            fake.paths, ["/o/oauth2/v2/auth?client_id=x&state=y"], "query passed through"
        )

        for refused in (
            "/fake-google/reset",
            "/fake-google/tokens",
            "/fake-google/o/oauth2/v2/auth/../../reset",
            "/fake-google/%2e%2e/o/oauth2/v2/auth",
        ):
            with self.subTest(path=refused):
                status, _ = http_get(host.port, refused, auth=auth)
                self.assertEqual(status, 404, f"{refused} must be refused even with the code")

    def test_demo_phone_signin_goes_through_the_front_door(self) -> None:
        # The api builds the browser-facing authorisation URL as
        # `${MT_PUBLIC_BASE_URL}/fake-google/...` (behaviour 0c; the Rust test
        # demo_phone_signin_goes_through_the_front_door proves the URL itself).
        # Here: that path through the front door reaches loopback fake-google,
        # and is refused without the code.
        fake = FakeGoogle()
        self.addCleanup(fake.shutdown)
        self.addCleanup(fake.server_close)
        threading.Thread(target=fake.serve_forever, daemon=True).start()
        code_file = write_code_file(self.tmp)
        host = _HostServer(
            self.tmp,
            ["--access-code-file", str(code_file), "--fake-google", fake.base_url],
        )
        self.addCleanup(host.close)

        status, _ = http_get(host.port, "/fake-google/o/oauth2/v2/auth")
        self.assertEqual(status, 401)
        status, _ = http_get(host.port, "/fake-google/o/oauth2/v2/auth", auth=auth_header(CODE))
        self.assertEqual(status, 200)
        self.assertTrue(fake.paths, "the request reached fake-google")

    def test_demo_access_code_is_rate_limited_and_not_in_argv(self) -> None:
        code_file = write_code_file(self.tmp)
        host = _HostServer(self.tmp, ["--access-code-file", str(code_file)])
        self.addCleanup(host.close)

        # More than five failures within 60 s, counted globally, answer 429.
        for attempt in range(1, 6):
            status, _ = http_get(host.port, "/", auth=auth_header("WrongWrong1234567"))
            self.assertEqual(status, 401, f"attempt {attempt} is a plain refusal")
        status, _ = http_get(host.port, "/", auth=auth_header("WrongWrong1234567"))
        self.assertEqual(status, 429, "the sixth failure is rate limited")
        # The block is global: even the right code is refused while it holds.
        status, _ = http_get(host.port, "/", auth=auth_header(CODE))
        self.assertEqual(status, 429, "the block applies to every source address")

        # The code is handed to e2e_host.py through a 600 file, never as an
        # argument, so it cannot appear in the process list (behaviour 2).
        script = DEMO.read_text()
        self.assertIn("--access-code-file", script)
        self.assertIn("chmod 600", script)
        self.assertNotIn('--access-code "$ACCESS_CODE"', script)


class DemoPhoneGuardTest(unittest.TestCase):
    """`demo.sh --phone` refusals, before anything is built or started."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def _stub(self, version: str = "2026.10.0") -> Path:
        self._stub_n = getattr(self, "_stub_n", 0) + 1
        stub = self.tmp / f"cloudflared-{self._stub_n}"
        stub.write_text(
            "#!/usr/bin/env bash\n"
            'if [[ "${1:-}" == "--version" ]]; then\n'
            f'  echo "cloudflared version {version} (stub)"\n'
            "  exit 0\n"
            "fi\n"
            "exit 0\n"
        )
        os.chmod(stub, 0o755)
        return stub

    def _pin(self, stub: Path, version: str = "2026.10.0", sha: str | None = None) -> Path:
        hex_digest = sha if sha is not None else hashlib.sha256(stub.read_bytes()).hexdigest()
        self._pin_n = getattr(self, "_pin_n", 0) + 1
        pin = self.tmp / f"cloudflared-{self._pin_n}.sha256"
        pin.write_text(
            f"# test pin\nversion {version}\n"
            f"sha256 linux-amd64 {hex_digest}\n"
            f"sha256 linux-arm64 {hex_digest}\n"
        )
        return pin

    def _run(self, env: dict[str, str]):
        environment = dict(os.environ)
        environment.update(env)
        return subprocess.run(
            ["bash", str(DEMO), "--phone"],
            cwd=str(ROOT),
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
        )

    @unittest.skipUnless(ARCH, f"unsupported test architecture {platform.machine()}")
    def test_demo_refuses_unpinned_or_wrong_cloudflared(self) -> None:
        cases = []
        # Not found at all.
        cases.append(
            (
                {"DEMO_CLOUDFLARED": str(self.tmp / "missing")},
                "needs cloudflared",
            )
        )
        # Present but the pin file has no entry ("unpinned").
        cases.append(
            (
                {
                    "DEMO_CLOUDFLARED": str(self._stub()),
                    "DEMO_CLOUDFLARED_SHA256": str(self.tmp / "absent.sha256"),
                },
                "no pin",
            )
        )
        # Wrong version.
        wrong_version_stub = self._stub(version="1.2.3")
        cases.append(
            (
                {
                    "DEMO_CLOUDFLARED": str(wrong_version_stub),
                    "DEMO_CLOUDFLARED_SHA256": str(self._pin(wrong_version_stub)),
                },
                "expected version",
            )
        )
        # Right version, wrong hash.
        tampered = self._stub()
        cases.append(
            (
                {
                    "DEMO_CLOUDFLARED": str(tampered),
                    "DEMO_CLOUDFLARED_SHA256": str(self._pin(tampered, sha="0" * 64)),
                },
                "SHA-256 mismatch",
            )
        )
        for env, needle in cases:
            with self.subTest(needle=needle):
                proc = self._run(env)
                combined = (proc.stdout + proc.stderr).lower()
                self.assertNotEqual(proc.returncode, 0, combined)
                self.assertIn(needle.lower(), combined)

    @unittest.skipUnless(ARCH, f"unsupported test architecture {platform.machine()}")
    def test_demo_refuses_non_loopback_tunnel_target(self) -> None:
        stub = self._stub()
        pin = self._pin(stub)
        for target in NON_LOOPBACK_TARGETS:
            with self.subTest(target=target):
                proc = self._run(
                    {
                        "DEMO_CLOUDFLARED": str(stub),
                        "DEMO_CLOUDFLARED_SHA256": str(pin),
                        "DEMO_TUNNEL_TARGET": target,
                    }
                )
                combined = (proc.stdout + proc.stderr).lower()
                self.assertNotEqual(proc.returncode, 0, combined)
                self.assertIn("non-loopback", combined)


@unittest.skipUnless(STACK, "needs the full demo stack (MT_DEMO_STACK=1)")
class DemoPhoneStackTest(unittest.TestCase):
    """The tunnel is exercised with a stub cloudflared: no real tunnel in CI."""

    PUBLIC_URL = "https://demo-phone-test.trycloudflare.com"

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._stop_demo)
        self.addCleanup(self._tmp.cleanup)

    def _stop_demo(self) -> None:
        subprocess.run(
            ["bash", str(DEMO), "stop"],
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            timeout=60,
        )

    def _stub(self) -> Path:
        stub = self.tmp / "cloudflared"
        stub.write_text(
            "#!/usr/bin/env bash\n"
            'if [[ "${1:-}" == "--version" ]]; then\n'
            '  echo "cloudflared version 2026.10.0 (stub)"\n'
            "  exit 0\n"
            "fi\n"
            'if [[ -n "${DEMO_STUB_ARGV:-}" ]]; then\n'
            '  printf "%s\\n" "$@" > "$DEMO_STUB_ARGV"\n'
            "fi\n"
            'echo "INF Your quick Tunnel has been created! Visit it at:"\n'
            f'echo "INF {self.PUBLIC_URL}"\n'
            "while true; do sleep 1; done\n"
        )
        os.chmod(stub, 0o755)
        return stub

    def test_demo_phone_exposes_only_the_front_door(self) -> None:
        stub = self._stub()
        pin = self.tmp / "cloudflared.sha256"
        digest = hashlib.sha256(stub.read_bytes()).hexdigest()
        pin.write_text(
            f"version 2026.10.0\nsha256 linux-amd64 {digest}\nsha256 linux-arm64 {digest}\n"
        )
        argv_file = self.tmp / "argv"
        env = dict(os.environ)
        env.update(
            {
                "DEMO_CLOUDFLARED": str(stub),
                "DEMO_CLOUDFLARED_SHA256": str(pin),
                "DEMO_STUB_ARGV": str(argv_file),
            }
        )
        proc = subprocess.run(
            ["bash", str(DEMO), "--phone"],
            cwd=str(ROOT),
            env=env,
            capture_output=True,
            text=True,
            timeout=180,
        )
        combined = proc.stdout + proc.stderr
        self.assertEqual(proc.returncode, 0, combined)
        self.assertIn(f"phone URL: {self.PUBLIC_URL}", proc.stdout)

        # Exactly one --url, and it is the loopback front door, never another
        # service's port.
        argv = argv_file.read_text().splitlines()
        self.assertIn("--url", argv)
        self.assertEqual(argv.count("--url"), 1, argv)
        target = argv[argv.index("--url") + 1]
        local = [line for line in proc.stdout.splitlines() if line.startswith("demo: running at ")]
        self.assertEqual(len(local), 1, proc.stdout)
        local_url = local[0].split(" ", 3)[3]
        self.assertEqual(target, local_url, "the tunnel targets the front door")
        self.assertTrue(
            target.startswith("http://127.0.0.1:"),
            f"the tunnel target must be loopback, got {target}",
        )

        # The code lives in a mode-600 file, is well formed, and appears in no
        # process's argv.
        code_file = ROOT / "target/demo/run/access-code"
        self.assertTrue(code_file.is_file(), "the access-code file is missing")
        self.assertEqual(code_file.stat().st_mode & 0o777, 0o600)
        code = code_file.read_text().strip()
        self.assertRegex(code, r"^[A-Za-z0-9]{16}$")
        self.assertNotIn(code, argv_file.read_text())
        for cmdline in Path("/proc").glob("[0-9]*/cmdline"):
            try:
                text = cmdline.read_bytes().decode("utf-8", "replace")
            except OSError:
                continue
            self.assertNotIn(code, text, f"the access code leaked into {cmdline}")


if __name__ == "__main__":
    unittest.main()
