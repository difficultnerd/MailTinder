#!/usr/bin/env python3
"""Static server that applies firebase.json hosting headers and collects CSP reports.

Stdlib only. Usage:
    python3 scripts/csp_serve.py --root app/build/web --config firebase.json --port 0 --reports <file>

Serves files under --root; unknown paths serve index.html (the "**" rewrite);
applies every header whose "source" glob matches; appends "; report-uri /__csp-report"
to Content-Security-Policy; POST /__csp-report appends the body as one JSON line
to --reports; prints "PORT <n>" on stdout once bound.
"""
import argparse
import fnmatch
import json
import mimetypes
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

mimetypes.add_type("application/wasm", ".wasm")
mimetypes.add_type("text/javascript", ".js")


def load_headers(config_path: Path) -> list[tuple[str, list[tuple[str, str]]]]:
    """Return [(source_glob, [(key, value), ...]), ...] from firebase.json hosting.headers."""
    data = json.loads(config_path.read_text())
    out = []
    for block in data["hosting"]["headers"]:
        headers = [(h["key"], h["value"]) for h in block["headers"]]
        out.append((block["source"], headers))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--config", required=True)
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--reports", required=True)
    args = ap.parse_args()

    root = Path(args.root).resolve()
    reports_path = Path(args.reports)
    header_blocks = load_headers(Path(args.config))
    reports_path.parent.mkdir(parents=True, exist_ok=True)
    reports_lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args) -> None:
            pass

        def _apply_headers(self, path: str) -> None:
            for source, headers in header_blocks:
                if fnmatch.fnmatch(path, source):
                    for key, value in headers:
                        if key == "Content-Security-Policy":
                            value = value + "; report-uri /__csp-report"
                        self.send_header(key, value)

        def _resolve(self, rel: str) -> Path:
            """Resolve a request path to a file under root, or index.html.

            Rejects traversal and unsafe characters before touching the
            filesystem (CodeQL path-injection sanitizer).
            """
            if ".." in rel or not all(c.isalnum() or c in "._-/" for c in rel):
                return root / "index.html"
            # codeql[py/path-injection] -- dev-only localhost test server; path sanitized below
            candidate = (root / rel).resolve()
            # Containment check CodeQL recognises: the resolved path must share
            # the root as its common path, else fall back to index.html.
            if os.path.commonpath([str(candidate), str(root)]) != str(root):
                return root / "index.html"
            return candidate

        def do_GET(self) -> None:
            rel = self.path.lstrip("/").split("?", 1)[0]
            if rel == "__csp-report":
                self.send_response(404)
                self.end_headers()
                return
            # codeql[py/path-injection] -- dev-only localhost test server; _resolve sanitizes
            candidate = self._resolve(rel)
            if not candidate.is_file():
                candidate = root / "index.html"
            ctype = mimetypes.guess_type(str(candidate))[0] or "application/octet-stream"
            # codeql[py/path-injection] -- dev-only localhost test server; candidate is under root
            body = candidate.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self._apply_headers("/" + rel)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self) -> None:
            if self.path != "/__csp-report":
                self.send_response(404)
                self.end_headers()
                return
            length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(length).decode("utf-8", "replace")
            with reports_lock:
                with reports_path.open("a") as f:
                    f.write(body + "\n")
            self.send_response(204)
            self.end_headers()

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"PORT {server.server_address[1]}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
