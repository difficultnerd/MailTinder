"""Tests for `scripts/demo.sh --dev`, `scripts/demo_reload.sh` and the dev-mode
front door (T-1113).

`demo_dev_applies_access_control_like_normal_mode` and
`demo_reload_changes_the_served_app` drive `scripts/e2e_host.py` directly with a
stub dev upstream (stdlib only, no stack), so they run in the ordinary
`python3 -m unittest discover -s tools` gate (the `ac-coverage` CI job). The
stub upstream plays the part of `flutter run -d web-server` on loopback: it
writes its pid to the same `--pid-file` demo.sh points Flutter at, re-reads its
served file on SIGUSR1 - the documented Flutter hot-reload mechanism - and
serves it over HTTP, so `scripts/demo_reload.sh` and the front-door proxy are
exercised end to end without a browser or the stack.

`demo_dev_serves_app_and_api_through_one_origin` is the shell test inside
`scripts/demo.sh --check --dev`: it starts the real stack and the real
`flutter run -d web-server`, so `scripts/e2e.sh` runs it in the `e2e` CI job,
the only job with Flutter and the stack.
"""

from __future__ import annotations

import base64
import http.client
import http.server
import json
import os
import signal
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
RELOAD = ROOT / "scripts" / "demo_reload.sh"
E2E_HOST = ROOT / "scripts" / "e2e_host.py"

CODE = "Abcdefgh12345678"
STACK = os.environ.get("MT_DEMO_STACK") == "1"


def auth_header(code: str = CODE, user: str = "demo") -> str:
    token = base64.b64encode(f"{user}:{code}".encode()).decode()
    return f"Basic {token}"


def http_get(port: int, path: str, auth: str | None = None, timeout: float = 5.0):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    headers = {"Authorization": auth} if auth else {}
    conn.request("GET", path, headers=headers)
    response = conn.getresponse()
    body = response.read()
    got = {k.lower(): v for k, v in response.getheaders()}
    conn.close()
    return response.status, got, body


class StubUpstream(socketserver.ThreadingTCPServer):
    """A stand-in for `flutter run -d web-server`, recording the paths asked."""

    allow_reuse_address = True
    daemon_threads = True

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), _StubHandler)
        self.requests: list[str] = []
        self.body = b"<html><body>stub</body></html>"

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server_address[1]}"


class _StubHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        self.server.requests.append(self.path)  # type: ignore[attr-defined]
        body = self.server.body  # type: ignore[attr-defined]
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        pass


def start_stub() -> StubUpstream:
    server = StubUpstream()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class HostServer:
    """Start `scripts/e2e_host.py` on a temp root and expose its port."""

    def __init__(self, root: Path, api_url: str, extra: list[str]) -> None:
        web = root / "web"
        web.mkdir(parents=True, exist_ok=True)
        (web / "index.html").write_text(
            "<!DOCTYPE html><html><head></head><body>static release build</body></html>"
        )
        firebase = root / "firebase.json"
        firebase.write_text(
            json.dumps(
                {
                    "hosting": {
                        "headers": [
                            {
                                "source": "**",
                                "headers": [
                                    {
                                        "key": "X-Content-Type-Options",
                                        "value": "nosniff",
                                    }
                                ],
                            }
                        ]
                    }
                }
            )
        )
        port_file = root / "host.port"
        args = [
            sys.executable,
            str(E2E_HOST),
            "--root",
            str(web),
            "--firebase-json",
            str(firebase),
            "--api",
            api_url,
            "--port",
            "0",
            "--port-file",
            str(port_file),
            *extra,
        ]
        self.proc = subprocess.Popen(
            args, stdout=subprocess.PIPE, stderr=subprocess.PIPE
        )
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


class E2eHostDevTest(unittest.TestCase):
    """Dev-mode proxying, security headers, access control and reload."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_demo_dev_applies_access_control_like_normal_mode(self) -> None:
        api = start_stub()
        self.addCleanup(api.shutdown)
        self.addCleanup(api.server_close)
        api.body = b'{"status":"ok"}'

        dev = start_stub()
        self.addCleanup(dev.shutdown)
        self.addCleanup(dev.server_close)
        dev.body = b"<html><body>dev app</body></html>"

        code_file = self.tmp / "access-code"
        code_file.write_text(CODE + "\n")
        os.chmod(code_file, 0o600)

        host = HostServer(
            self.tmp,
            api.url,
            [
                "--dev-upstream",
                dev.url,
                "--access-code-file",
                str(code_file),
            ],
        )
        self.addCleanup(host.close)
        auth = auth_header()

        # Exactly like normal mode: a request without the code is refused, with
        # the code it is served - here from the dev upstream, not the static
        # release build.
        status, _, _ = http_get(host.port, "/")
        self.assertEqual(status, 401, "dev mode must keep the front door's access control")
        status, headers, body = http_get(host.port, "/", auth=auth)
        self.assertEqual(status, 200)
        self.assertEqual(body, b"<html><body>dev app</body></html>")
        self.assertNotIn(b"static release build", body, "the static root must not be served in dev mode")
        self.assertEqual(headers.get("x-content-type-options"), "nosniff", "dev mode keeps the firebase security headers")

        # /api/** still goes to the api through the same origin.
        status, _, body = http_get(host.port, "/api/v1/healthz", auth=auth)
        self.assertEqual(status, 200)
        self.assertEqual(body, b'{"status":"ok"}')

        # The dev upstream saw the app path (with its query) but never /api.
        self.assertIn("/", dev.requests)
        self.assertFalse(
            any(p.startswith("/api") for p in dev.requests),
            f"/api must never reach the dev server: {dev.requests}",
        )
        self.assertIn("/api/v1/healthz", api.requests)

    def test_demo_reload_changes_the_served_app(self) -> None:
        source = self.tmp / "copy.dart"
        source.write_text("const continueWithGoogle = 'Continue with Google';")

        dev = start_stub()
        self.addCleanup(dev.shutdown)
        self.addCleanup(dev.server_close)
        dev.body = source.read_bytes()

        # The stub dev server plays flutter run --pid-file: it writes its pid
        # where demo.sh would and re-reads its served file on SIGUSR1.
        run_dir = self.tmp / "run"
        run_dir.mkdir()
        (run_dir / "flutter.pid").write_text(f"{os.getpid()}\n")

        previous = signal.signal(
            signal.SIGUSR1,
            lambda signum, frame: setattr(dev, "body", source.read_bytes()),
        )
        self.addCleanup(signal.signal, signal.SIGUSR1, previous)

        host = HostServer(self.tmp, "http://127.0.0.1:1", ["--dev-upstream", dev.url])
        self.addCleanup(host.close)

        status, _, body = http_get(host.port, "/")
        self.assertEqual(status, 200)
        self.assertIn(b"Continue with Google", body)

        token = "ReloadedVisibleString42"
        try:
            source.write_text(f"const continueWithGoogle = 'Continue with Google {token}';")
            proc = subprocess.run(
                ["bash", str(RELOAD)],
                cwd=str(ROOT),
                env={**os.environ, "DEMO_RUN_DIR": str(run_dir)},
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertIn("hot reload", proc.stdout)

            deadline = time.monotonic() + 30
            served = False
            while time.monotonic() < deadline:
                status, _, body = http_get(host.port, "/")
                if status == 200 and token.encode() in body:
                    served = True
                    break
                time.sleep(0.5)
            self.assertTrue(
                served, "the reloaded string was not served through the front door within 30 s"
            )
        finally:
            source.write_text("const continueWithGoogle = 'Continue with Google';")

    def test_demo_reload_refuses_without_a_dev_server(self) -> None:
        run_dir = self.tmp / "empty-run"
        run_dir.mkdir()
        proc = subprocess.run(
            ["bash", str(RELOAD)],
            cwd=str(ROOT),
            env={**os.environ, "DEMO_RUN_DIR": str(run_dir)},
            capture_output=True,
            text=True,
            timeout=30,
        )
        combined = proc.stdout + proc.stderr
        self.assertNotEqual(proc.returncode, 0, combined)
        self.assertIn("no dev server pid file", combined)

    def test_demo_dev_script_wiring(self) -> None:
        """`demo.sh --dev` wires the documented Flutter mechanisms the tests rely on."""
        script = DEMO.read_text()
        self.assertIn("flutter run -d web-server", script)
        self.assertIn("--pid-file", script)
        self.assertIn("--dart-define=MT_E2E=true", script)
        self.assertIn("--dev-upstream", script)
        self.assertIn("websockets==", script)
        self.assertIn("target/demo/venv", script)
        reload_script = RELOAD.read_text()
        self.assertIn("flutter.pid", reload_script)
        self.assertIn("USR1", reload_script)


if __name__ == "__main__":
    unittest.main()
