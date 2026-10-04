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


def index_files(root: Path) -> dict[str, Path]:
    """Map every file under root to its absolute path (relative POSIX key).

    The request path is looked up in this map, so user input is never used to
    construct a filesystem path (CodeQL path-injection safe).
    """
    out: dict[str, Path] = {}
    for p in root.rglob("*"):
        if p.is_file():
            out[p.relative_to(root).as_posix()] = p
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
    safe_files = index_files(root)
    index_html = root / "index.html"

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

        def do_GET(self) -> None:
            rel = self.path.lstrip("/").split("?", 1)[0]
            if rel == "__csp-report":
                self.send_response(404)
                self.end_headers()
                return
            # Look up the request path in the precomputed safe map; never build
            # a path from user input. Unknown paths fall back to index.html.
            candidate = safe_files.get(rel, index_html)
            if not candidate.is_file():
                candidate = index_html
            ctype = mimetypes.guess_type(str(candidate))[0] or "application/octet-stream"
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
