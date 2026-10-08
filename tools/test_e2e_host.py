"""Local-only regression tests for the T-1101a browser host."""

import http.client
import importlib.util
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import cast


REPO = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("e2e_host", REPO / "scripts/e2e_host.py")
assert SPEC is not None and SPEC.loader is not None
host = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(host)


class UpstreamServer(ThreadingHTTPServer):
    def __init__(self):
        self.requests = queue.Queue()
        super().__init__(("127.0.0.1", 0), Upstream)


class Upstream(BaseHTTPRequestHandler):
    def handle_request(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        cast(UpstreamServer, self.server).requests.put((self.command, self.path, self.headers, body))
        self.send_response(418 if self.path.startswith("/api/error") else 200)
        self.send_header("Set-Cookie", "session=synthetic; Secure; HttpOnly; SameSite=Lax")
        self.send_header("Set-Cookie", "csrf=synthetic; Secure; SameSite=Lax")
        self.send_header("Clear-Site-Data", '"cookies", "storage"')
        self.send_header("X-Upstream", "preserved")
        self.send_header("Location", "/api/next")
        self.send_header("Content-Type", "application/octet-stream")
        if self.path.startswith("/api/chunked"):
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            self.wfile.write(b"3\r\nabc\r\n0\r\n\r\n")
        else:
            self.send_header("Content-Length", "3")
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(b"abc")

    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = do_OPTIONS = handle_request

    def log_message(self, format, *args):
        pass


class HostTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e2e-host-", dir=os.environ.get("TMPDIR"))
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "web"
        self.root.mkdir()
        (self.root / "index.html").write_bytes(b"<html>synthetic app</html>")
        (self.root / "assets").mkdir()
        (self.root / "assets/main.js").write_bytes(b"synthetic javascript")
        (self.root / ".private").write_bytes(b"must not leak")
        outside = self.base / "secret.txt"
        outside.write_bytes(b"must not leak")
        (self.root / "escape.txt").symlink_to(outside)
        (self.root / "hidden.txt").symlink_to(self.root / ".private")
        self.firebase = self.base / "firebase.json"
        self.firebase.write_text(json.dumps({"hosting": {"headers": [
            {"source": "**", "headers": [{"key": "X-Global", "value": "yes"}]},
            {"source": "/index.html", "headers": [{"key": "Cache-Control", "value": "no-cache"}]},
            {"source": "/assets/*", "headers": [{"key": "X-Asset", "value": "yes"}]},
            {"source": "/api/**", "headers": [{"key": "X-Api", "value": "yes"}]},
        ]}}))
        self.upstream = UpstreamServer()
        self.start_server(self.upstream)
        self.server = host.create_server(self.root, self.firebase,
                                         f"http://127.0.0.1:{self.upstream.server_port}")
        self.start_server(self.server)

    def start_server(self, server):
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(thread.join, 3)
        self.addCleanup(server.shutdown)

    def request(self, path, method="GET", body=None, headers=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=3)
        self.addCleanup(connection.close)
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        return response.status, response.headers, response.read()

    def test_t_1101a_proxy_methods_body_and_security_headers(self):
        for method in ("GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"):
            with self.subTest(method=method):
                headers = {"Cookie": "session=synthetic; csrf=synthetic",
                           "Origin": "http://localhost:1234", "X-CSRF-Token": "synthetic",
                           "Idempotency-Key": "synthetic-key", "Content-Type": "application/json",
                           "X-Forwarded-For": "203.0.113.1"}
                status, response_headers, body = self.request(
                    "/api/action?value=%2F", method, b'{"synthetic":true}', headers)
                actual_method, path, forwarded, forwarded_body = self.upstream.requests.get(timeout=3)
                self.assertEqual((actual_method, path, forwarded_body),
                                 (method, "/api/action?value=%2F", b'{"synthetic":true}'))
                for name in ("Cookie", "Origin", "X-CSRF-Token", "Idempotency-Key", "Content-Type"):
                    self.assertEqual(forwarded[name], headers[name])
                self.assertEqual(forwarded.get_all("X-Forwarded-For"), ["127.0.0.1"])
                self.assertEqual(status, 200)
                self.assertEqual(body, b"" if method == "HEAD" else b"abc")
                self.assertEqual(response_headers["X-Api"], "yes")

    def test_t_1101a_proxy_preserves_duplicate_cookies_and_all_end_to_end_headers(self):
        status, headers, body = self.request("/api/error")
        self.assertEqual((status, body), (418, b"abc"))
        self.assertEqual(headers.get_all("Set-Cookie"), [
            "session=synthetic; Secure; HttpOnly; SameSite=Lax",
            "csrf=synthetic; Secure; SameSite=Lax"])
        self.assertEqual(headers["Clear-Site-Data"], '"cookies", "storage"')
        self.assertEqual(headers["X-Upstream"], "preserved")
        self.assertEqual(headers["Location"], "/api/next")
        self.assertEqual(headers["Content-Type"], "application/octet-stream")
        self.assertEqual(headers["X-Global"], "yes")

    def test_t_1101a_chunked_upstream_is_reframed_without_corruption(self):
        status, headers, body = self.request("/api/chunked")
        self.assertEqual((status, body), (200, b"abc"))
        self.assertIsNone(headers.get("Transfer-Encoding"))
        self.assertEqual(headers["Content-Length"], "3")

    def test_t_1101a_static_and_head_headers(self):
        status, headers, body = self.request("/assets/main.js?version=1")
        self.assertEqual((status, body), (200, b"synthetic javascript"))
        self.assertEqual(headers["X-Global"], "yes")
        self.assertEqual(headers["X-Asset"], "yes")
        self.assertIn("javascript", headers["Content-Type"])
        status, headers, body = self.request("/assets/main.js", "HEAD")
        self.assertEqual((status, body), (200, b""))
        self.assertEqual(headers["Content-Length"], str(len(b"synthetic javascript")))

    def test_t_1101a_spa_fallback_without_directory_listing_or_api_prefix_confusion(self):
        for path in ("/", "/feed?tab=inbox", "/assets/", "/apix/action"):
            with self.subTest(path=path):
                status, headers, body = self.request(path)
                self.assertEqual((status, body), (200, b"<html>synthetic app</html>"))
                self.assertEqual(headers["X-Global"], "yes")
                self.assertEqual(headers["Cache-Control"], "no-cache")
        self.assertTrue(self.upstream.requests.empty())

    def test_t_1101a_paths_cannot_escape_root_or_expose_dotfiles(self):
        for path in ("/../secret.txt", "/%2e%2e/secret.txt", "/assets/%2e%2e/%2e%2e/secret.txt",
                     "/escape.txt", "/hidden.txt", "/.private", "/%2eprivate", "/assets%5c..%5csecret.txt",
                     "/bad%00path", "http://example.invalid/api/action"):
            with self.subTest(path=path):
                status, headers, body = self.request(path)
                self.assertIn(status, (400, 403))
                self.assertNotIn(b"must not leak", body)
                self.assertEqual(headers["X-Global"], "yes")
        self.assertTrue(self.upstream.requests.empty())

    def test_t_1101a_request_hop_by_hop_headers_are_not_forwarded(self):
        self.request("/api/action", headers={"Connection": "X-Remove", "X-Remove": "secret"})
        _, _, headers, _ = self.upstream.requests.get(timeout=3)
        self.assertIsNone(headers.get("X-Remove"))

    def test_t_1101a_bad_request_framing_is_rejected(self):
        for headers in ({"Content-Length": "-1"}, {"Content-Length": "invalid"},
                        {"Transfer-Encoding": "chunked"}):
            with self.subTest(headers=headers):
                status, response_headers, _ = self.request("/api/action", "POST", headers=headers)
                self.assertEqual(status, 400)
                self.assertEqual(response_headers["X-Global"], "yes")
        self.assertTrue(self.upstream.requests.empty())

    def test_t_1101a_upstream_unavailable_returns_generic_gateway_error(self):
        self.server.api = "http://127.0.0.1:0"
        status, headers, body = self.request("/api/action")
        self.assertEqual(status, 502)
        self.assertEqual(headers["X-Global"], "yes")
        self.assertNotIn(b"127.0.0.1", body)

    def test_t_1101a_firebase_globs_are_path_aware(self):
        server = host.create_server(self.root, REPO / "firebase.json",
                                    f"http://127.0.0.1:{self.upstream.server_port}")
        self.start_server(server)
        connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=3)
        self.addCleanup(connection.close)
        connection.request("GET", "/")
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        self.assertEqual(response.headers["Cache-Control"], "no-cache")
        self.assertEqual(response.headers["X-Content-Type-Options"], "nosniff")
        self.assertIn("default-src 'none'", response.headers["Content-Security-Policy"])
        response.read()
        cases = [("**", "/a/b", True), ("/api/**", "/api/a/b", True),
                 ("/assets/*", "/assets/a/b", False), ("/assets/**", "/assets/a/b", True),
                 ("**/*.js", "/main.js", True), ("**/*.js", "/assets/main.js", True),
                 ("/{assets,fonts}/**", "/fonts/a.woff", True),
                 ("/assets/?.js", "/assets/ab.js", False)]
        for pattern, path, expected in cases:
            with self.subTest(pattern=pattern, path=path):
                self.assertEqual(host.glob_matches(pattern, path), expected)

    def test_t_1101a_cli_ephemeral_port_file_and_loopback_binding(self):
        port_file = self.base / "host.port"
        process = subprocess.Popen([sys.executable, str(REPO / "scripts/e2e_host.py"),
                                    "--root", str(self.root), "--firebase-json", str(self.firebase),
                                    "--api", f"http://127.0.0.1:{self.upstream.server_port}",
                                    "--port-file", str(port_file)], stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        def stop():
            process.terminate()
            process.wait(timeout=5)
        self.addCleanup(stop)
        deadline = time.monotonic() + 5
        while not port_file.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertIsNone(process.poll())
        port = int(port_file.read_text())
        self.assertGreater(port, 0)
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
        self.addCleanup(connection.close)
        connection.request("GET", "/")
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        self.assertEqual(response.read(), b"<html>synthetic app</html>")


if __name__ == "__main__":
    unittest.main()
