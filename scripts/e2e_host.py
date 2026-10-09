#!/usr/bin/env python3
"""Static server for the e2e web build: firebase.json headers + an /api proxy.

Stdlib only. Usage:

    python3 scripts/e2e_host.py --root app/build/web --firebase-json firebase.json \
        --api http://127.0.0.1:8123 --port 0 --port-file target/e2e-run/host.port

Serves every file under --root; for every response adds each firebase.json
`hosting.headers` entry whose `source` glob matches the request path; proxies
`/api/**` to --api preserving method, body and the session/CSRF/idempotency
headers, and copies back every response header (including `Set-Cookie` and
`Clear-Site-Data`). Sets `X-Forwarded-For: 127.0.0.1`. Unknown paths serve
`index.html` (the `**` rewrite). Prints `PORT <n>` on stdout once bound and,
when asked, writes the port to --port-file.
"""

import argparse
import fnmatch
import http.client
import json
import mimetypes
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

mimetypes.add_type("application/wasm", ".wasm")
mimetypes.add_type("text/javascript", ".js")

# Request headers the api needs: the session cookie, the CSRF origin and token,
# the idempotency key, and the body's content type.
FORWARD_REQUEST_HEADERS = (
    "content-type",
    "accept",
    "cookie",
    "origin",
    "x-csrf-token",
    "idempotency-key",
)
# Hop-by-hop and length headers we never copy back (we set our own length).
SKIP_RESPONSE_HEADERS = frozenset(
    {"transfer-encoding", "content-length", "connection", "keep-alive"}
)


def load_headers(config_path: Path) -> list[tuple[str, list[tuple[str, str]]]]:
    """[(source_glob, [(key, value), ...]), ...] from firebase.json hosting.headers."""
    data = json.loads(config_path.read_text())
    out = []
    for block in data["hosting"]["headers"]:
        out.append(
            (block["source"], [(h["key"], h["value"]) for h in block["headers"]])
        )
    return out


def index_files(root: Path) -> dict[str, Path]:
    """Map every file under root to its path (relative POSIX key).

    The request path is looked up in this map, so user input is never used to
    build a filesystem path (path-injection safe).
    """
    out: dict[str, Path] = {}
    for path in root.rglob("*"):
        if path.is_file():
            out[path.relative_to(root).as_posix()] = path
    return out


def parse_api(api: str) -> tuple[str, int, str]:
    parts = urllib.parse.urlsplit(api)
    host = parts.hostname or "127.0.0.1"
    port = parts.port or 80
    prefix = parts.path.rstrip("/")
    return host, port, prefix


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--firebase-json", required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--port-file", default=None)
    args = parser.parse_args()

    root = Path(args.root).resolve()
    header_blocks = load_headers(Path(args.firebase_json))
    safe_files = index_files(root)
    index_html = root / "index.html"
    api_host, api_port, api_prefix = parse_api(args.api)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        # The access log is a diagnostic (T-1101a CI): it shows exactly what the
        # browser asked for. The query string is dropped so a token can never
        # reach it (S5); the fragment never reaches the server at all.
        def log_request(self, code: object = "-", size: object = "-") -> None:
            path = self.path.split("?", 1)[0]
            sys.stderr.write(f"e2e-host: {self.command} {path} -> {code}\n")
            sys.stderr.flush()

        def log_message(self, format: str, *log_args: object) -> None:  # noqa: A002
            sys.stderr.write("e2e-host: " + (format % log_args) + "\n")
            sys.stderr.flush()

        def _apply_headers(self, path: str) -> None:
            for source, headers in header_blocks:
                if fnmatch.fnmatch(path, source):
                    for key, value in headers:
                        self.send_header(key, value)

        def _serve_file(self, rel: str) -> None:
            candidate = safe_files.get(rel, index_html)
            if not candidate.is_file():
                candidate = index_html
            body = candidate.read_bytes()
            ctype = mimetypes.guess_type(str(candidate))[0] or "application/octet-stream"
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self._apply_headers("/" + rel)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _proxy(self, method: str) -> None:
            # Trace before the upstream call: if the api hangs, the log still
            # proves the browser made the request.
            sys.stderr.write(f"e2e-host: {method} {self.path.split('?', 1)[0]} (proxy)\n")
            sys.stderr.flush()
            length = int(self.headers.get("Content-Length", 0) or 0)
            body = self.rfile.read(length) if length else None
            path = api_prefix + self.path
            headers = {"X-Forwarded-For": "127.0.0.1"}
            for name in FORWARD_REQUEST_HEADERS:
                value = self.headers.get(name)
                if value is not None:
                    headers[name] = value
            conn = http.client.HTTPConnection(api_host, api_port, timeout=30)
            conn.request(method, path, body=body, headers=headers)
            response = conn.getresponse()
            data = response.read()
            self.send_response(response.status, response.reason)
            for key, value in response.getheaders():
                if key.lower() in SKIP_RESPONSE_HEADERS:
                    continue
                # Never relay a header carrying CR/LF (response splitting); drop it rather than rewrite it.
                if any(c in key or c in value for c in "\r\n\0"):
                    sys.stderr.write(f"e2e-host: dropped upstream header with control characters: {key!r}\n")
                    continue
                self.send_header(key, value)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            conn.close()

        def _is_api(self) -> bool:
            return self.path == "/api" or self.path.startswith("/api/")

        def do_GET(self) -> None:  # noqa: N802
            if self._is_api():
                self._proxy("GET")
            else:
                self._serve_file(self.path.lstrip("/").split("?", 1)[0])

        def do_HEAD(self) -> None:  # noqa: N802
            if self._is_api():
                self._proxy("HEAD")
            else:
                self._serve_file(self.path.lstrip("/").split("?", 1)[0])

        def do_POST(self) -> None:  # noqa: N802
            self._proxy("POST") if self._is_api() else self.send_error(404)

        def do_PUT(self) -> None:  # noqa: N802
            self._proxy("PUT") if self._is_api() else self.send_error(404)

        def do_PATCH(self) -> None:  # noqa: N802
            self._proxy("PATCH") if self._is_api() else self.send_error(404)

        def do_DELETE(self) -> None:  # noqa: N802
            self._proxy("DELETE") if self._is_api() else self.send_error(404)

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    if args.port_file:
        Path(args.port_file).write_text(f"{server.server_address[1]}\n")
    print(f"PORT {server.server_address[1]}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
