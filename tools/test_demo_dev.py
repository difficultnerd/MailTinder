"""Tests for `scripts/demo.sh --dev` and the hot-reload front door (T-1113).

The AC1/AC3 tests drive `scripts/e2e_host.py` directly against stub upstreams
(no stack, stdlib only), so they run in the ordinary
`python3 -m unittest discover -s tools` gate (the `ac-coverage` CI job): the
front door is asked to serve the app and the api from one origin, and to apply
the same access control as normal mode.

`demo_reload_changes_the_served_app` needs the whole dev stack (the Flutter dev
server behind `demo.sh --check --dev`), so it runs only under `MT_DEMO_STACK=1`;
`scripts/e2e.sh` runs it directly in the `e2e` CI job, the only job with
Flutter and the stack.

The websocket relay is exercised only when the pinned `websockets` package is
installed (by `demo.sh --dev` into target/demo/venv); the ordinary gate has no
such virtualenv, so that test skips there.
"""

from __future__ import annotations

import base64
import http.client
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEMO = ROOT / "scripts" / "demo.sh"
E2E_HOST = ROOT / "scripts" / "e2e_host.py"
VENV_PYTHON = ROOT / "target" / "demo" / "venv" / "bin" / "python"
STACK = os.environ.get("MT_DEMO_STACK") == "1"

CODE = "Abcdefgh12345678"

# Self-contained relay check, run with the venv interpreter (the pinned
# `websockets` package): one loopback port serves HTTP for non-upgrade paths and
# a websocket that echoes, and a request to the front door's ws path must reach
# it. Keeping it all in the subprocess means the test process itself never
# imports `websockets`, so the ordinary gate can run the test when the venv
# exists and skip it otherwise.
WS_RELAY_CHECK = (
    "import json, subprocess, sys, tempfile, threading, time\n"
    "from pathlib import Path\n"
    "from websockets.datastructures import Headers\n"
    "from websockets.http11 import Response\n"
    "from websockets.sync.client import connect\n"
    "from websockets.sync.server import serve\n"
    "\n"
    "root, e2e_host = Path(sys.argv[1]), Path(sys.argv[2])\n"
    "tmp = Path(tempfile.mkdtemp())\n"
    "seen = []\n"
    "\n"
    "def process_request(connection, request):\n"
    "    if request.headers.get('Upgrade', '').lower() == 'websocket':\n"
    "        seen.append(request.path)\n"
    "        return None\n"
    "    body = b'<html>ws-stub</html>'\n"
    "    return Response(\n"
    "        200, 'OK',\n"
    "        Headers([('Content-Type', 'text/html'), ('Content-Length', str(len(body)))]),\n"
    "        body,\n"
    "    )\n"
    "\n"
    "def handler(connection):\n"
    "    for message in connection:\n"
    "        text = message if isinstance(message, str) else message.decode('utf-8')\n"
    "        connection.send('echo:' + text)\n"
    "\n"
    "server = serve(handler, '127.0.0.1', 0, process_request=process_request)\n"
    "stub_port = server.socket.getsockname()[1]\n"
    "threading.Thread(target=server.serve_forever, daemon=True).start()\n"
    "\n"
    "web = tmp / 'web'\n"
    "web.mkdir()\n"
    "(web / 'index.html').write_text('<!DOCTYPE html><html></html>')\n"
    "firebase = tmp / 'firebase.json'\n"
    "firebase.write_text(json.dumps({'hosting': {'headers': []}}))\n"
    "port_file = tmp / 'host.port'\n"
    "host = subprocess.Popen(\n"
    "    [\n"
    "        sys.executable, str(e2e_host), '--root', str(web),\n"
    "        '--firebase-json', str(firebase), '--api', 'http://127.0.0.1:1',\n"
    "        '--dev-upstream', f'http://127.0.0.1:{stub_port}',\n"
    "        '--port', '0', '--port-file', str(port_file),\n"
    "    ],\n"
    "    stdout=subprocess.PIPE, stderr=subprocess.PIPE,\n"
    ")\n"
    "front = 0\n"
    "for _ in range(200):\n"
    "    if port_file.is_file() and port_file.read_text().strip():\n"
    "        front = int(port_file.read_text().strip())\n"
    "        break\n"
    "    time.sleep(0.05)\n"
    "assert front, 'the front door did not report a port'\n"
    "with connect(f'ws://127.0.0.1:{front}/$dwdsSseHandler', open_timeout=5) as ws:\n"
    "    ws.send('ping')\n"
    "    got = ws.recv(timeout=5)\n"
    "host.terminate()\n"
    "if got == 'echo:ping' and seen == ['/$dwdsSseHandler']:\n"
    "    print('WS_OK')\n"
    "else:\n"
    "    print(f'WS_BAD got={got!r} seen={seen!r}')\n"
)


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


class _StubHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        self.server.paths.append(self.path.split("?", 1)[0])  # type: ignore[attr-defined]
        body: bytes = self.server.body  # type: ignore[attr-defined]
        self.send_response(200)
        self.send_header("Content-Type", self.server.content_type)  # type: ignore[attr-defined]
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        pass


class StubUpstream(ThreadingHTTPServer):
    """A stub upstream that records the paths it was asked for."""

    allow_reuse_address = True

    def __init__(self, body: bytes, content_type: str) -> None:
        super().__init__(("127.0.0.1", 0), _StubHandler)
        self.body = body
        self.content_type = content_type
        self.paths: list[str] = []

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server_address[1]}"


class _DevHost:
    """Start scripts/e2e_host.py in dev mode and expose its port."""

    def __init__(
        self,
        root: Path,
        dev_upstream: str,
        api: str = "http://127.0.0.1:1",
        access_code_file: Path | None = None,
        python: str | None = None,
    ) -> None:
        web = root / "web"
        web.mkdir(parents=True, exist_ok=True)
        (web / "index.html").write_text("<!DOCTYPE html><html><head></head></html>")
        firebase = root / "firebase.json"
        firebase.write_text(json.dumps({"hosting": {"headers": []}}))
        port_file = root / "host.port"
        if port_file.exists():
            port_file.unlink()
        args = [
            python or sys.executable,
            str(E2E_HOST),
            "--root",
            str(web),
            "--firebase-json",
            str(firebase),
            "--api",
            api,
            "--dev-upstream",
            dev_upstream,
            "--port",
            "0",
            "--port-file",
            str(port_file),
        ]
        if access_code_file is not None:
            args += ["--access-code-file", str(access_code_file)]
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


class DevFrontDoorTest(unittest.TestCase):
    """The front door's dev upstream, driven directly (AC1 and AC3)."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self._servers: list[ThreadingHTTPServer] = []

    def tearDown(self) -> None:
        for server in self._servers:
            server.shutdown()
            server.server_close()
        self._tmp.cleanup()

    def _stub(self, body: bytes, content_type: str) -> StubUpstream:
        server = StubUpstream(body, content_type)
        self._servers.append(server)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return server

    def test_demo_dev_serves_app_and_api_through_one_origin(self) -> None:
        dev = self._stub(b"<html><body>dev-app</body></html>", "text/html")
        api = self._stub(b'{"status":"ok"}', "application/json")
        host = _DevHost(self.tmp, dev.base_url, api=api.base_url)
        self.addCleanup(host.close)

        # The page comes from the dev server, the api from the api: one origin.
        status, body = http_get(host.port, "/")
        self.assertEqual(status, 200)
        self.assertIn(b"dev-app", body)

        status, body = http_get(host.port, "/api/v1/healthz")
        self.assertEqual(status, 200)
        self.assertIn(b'"status":"ok"', body)

        # A module file also goes to the dev server, not the api.
        status, _ = http_get(host.port, "/packages/app/main.dart.lib.js")
        self.assertEqual(status, 200)

        self.assertEqual(api.paths, ["/api/v1/healthz"], "only /api/** reached the api")
        self.assertIn("/", dev.paths)
        self.assertIn("/packages/app/main.dart.lib.js", dev.paths)

    def test_demo_dev_applies_access_control_like_normal_mode(self) -> None:
        dev = self._stub(b"<html><body>dev-app</body></html>", "text/html")
        api = self._stub(b'{"status":"ok"}', "application/json")
        code_file = write_code_file(self.tmp)
        host = _DevHost(
            self.tmp, dev.base_url, api=api.base_url, access_code_file=code_file
        )
        self.addCleanup(host.close)
        auth = auth_header(CODE)

        for path in ("/", "/api/v1/healthz", "/packages/app/main.dart.lib.js"):
            with self.subTest(path=path):
                status, _ = http_get(host.port, path)
                self.assertEqual(status, 401, "without the code everything is refused")
                status, body = http_get(host.port, path, auth=auth)
                self.assertEqual(status, 200, "with the code it is served")
                self.assertTrue(body)

    def test_dev_upstream_rewrites_the_debug_socket_to_the_front_door(self) -> None:
        dev = self._stub(b"", "text/javascript")
        dev.body = f"var debug='ws://127.0.0.1:{dev.server_address[1]}';".encode()
        host = _DevHost(self.tmp, dev.base_url)
        self.addCleanup(host.close)

        status, body = http_get(host.port, "/main_module.bootstrap.js")
        self.assertEqual(status, 200)
        self.assertNotIn(dev.base_url.encode(), body)
        self.assertIn(
            f"ws://127.0.0.1:{host.port}".encode(),
            body,
            "the baked-in dev socket must be rewritten to this front door",
        )

    @unittest.skipUnless(
        VENV_PYTHON.exists(),
        "needs the pinned websockets virtualenv (target/demo/venv)",
    )
    def test_dev_upstream_relays_the_debug_websocket(self) -> None:
        result = subprocess.run(
            [str(VENV_PYTHON), "-c", WS_RELAY_CHECK, str(ROOT), str(E2E_HOST)],
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("WS_OK", result.stdout, result.stdout + result.stderr)


@unittest.skipUnless(STACK, "needs the full dev stack (MT_DEMO_STACK=1)")
class DemoDevStackTest(unittest.TestCase):
    """`demo.sh --check --dev`: the three T-1113 checks on a real dev stack."""

    def test_demo_reload_changes_the_served_app(self) -> None:
        proc = subprocess.run(
            ["bash", str(DEMO), "--check", "--dev"],
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            timeout=1800,
        )
        combined = proc.stdout + proc.stderr
        self.assertEqual(proc.returncode, 0, combined)
        for name in (
            "demo_dev_serves_app_and_api_through_one_origin",
            "demo_dev_applies_access_control_like_normal_mode",
            "demo_reload_changes_the_served_app",
        ):
            self.assertIn(f"PASS {name}", combined, combined)
        self.assertIn("leak census: nothing left", combined)
        self.assertFalse((ROOT / "target/demo/state.json").exists())
        # The edit the reload check made was reverted.
        copy = (ROOT / "app/lib/copy.dart").read_text()
        self.assertNotIn("RELOAD CHECK T1113", copy)
        self.assertIn("static const productName = 'Mail Tinder';", copy)


if __name__ == "__main__":
    unittest.main()
