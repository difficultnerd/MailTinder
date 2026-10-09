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

Phone mode (T-1108c, `demo.sh --phone`) adds two things:

* `--access-code-file <path>` turns on HTTP Basic auth (user `demo`) on EVERY
  request. The code is read from the file (mode 600, never an argument, so it
  never shows in the process list) and compared in constant time. More than five
  failed attempts within 60 s, counted globally -- behind the tunnel every
  request arrives from the loopback tunnel process, so a per-address key is
  meaningless -- answer `429` for a minute (review finding N2; 60 s so a typo cannot lock the owner out for long).
* `--fake-google <loopback url>` proxies EXACTLY `GET
  /fake-google/o/oauth2/v2/auth` (query string included) to fake-google on
  loopback: that is the one server-to-server path the browser needs for the
  sign-in round trip (review finding F1). Every other `/fake-google/**` path is
  the control plane (`/reset`, `/tokens`, `/fail`, `/gmail/*`, ...) and answers
  `404` even with the code. Paths containing `..` or an encoded slash/dot
  (`%2f`, `%2e`) are refused before the `/fake-google/` prefix is matched
  (review finding N1).

Dev mode (T-1113, `demo.sh --dev`) adds `--dev-upstream
http://127.0.0.1:<port>`: instead of serving the static release build, the front
door reverse-proxies `/` (and the Flutter dev server's websocket) to a running
`flutter run -d web-server`, while `/api/**` keeps going to the api. The dev
server has no access control of its own, so it binds loopback and every request
still passes the same access control and the same firebase.json `hosting.headers`
as the normal demo. The websocket relay uses the pinned `websockets` package
(installed by demo.sh into target/demo/venv); the HTTP handler peeks the first
bytes of each connection so the upgrade is handed to that relay untouched.
"""

import argparse
import base64
import binascii
import fnmatch
import hmac
import http.client
import json
import mimetypes
import select
import socket
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

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

# Dev mode (T-1113): headers forwarded to the `flutter run -d web-server`
# upstream so the browser gets the same assets it would from the dev server.
DEV_FORWARD_REQUEST_HEADERS = (
    "accept",
    "accept-encoding",
    "accept-language",
    "content-type",
    "cookie",
    "origin",
    "range",
    "if-none-match",
    "if-modified-since",
)

# A websocket upgrade must be answered by the relay, not the HTTP handler, so
# the first bytes of a connection are peeked (non-destructively) before
# BaseHTTPRequestHandler consumes them.
WEBSOCKET_PEEK_BYTES = 65536
WEBSOCKET_PEEK_TIMEOUT_S = 5.0

# The one fake-google path the browser needs, and the standard viewport meta
# every served page must carry so the demo is usable on a phone (Behaviour 3).
FAKE_GOOGLE_AUTHORISE_PATH = "/fake-google/o/oauth2/v2/auth"
VIEWPORT_META = b'<meta name="viewport" content="width=device-width, initial-scale=1.0">'

# Rate-limit tunables (Behaviour 2, review finding N2).
MAX_FAILURES = 5
FAILURE_WINDOW_S = 60.0
BLOCK_SECONDS = 60.0


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


def path_is_unsafe(path: str) -> bool:
    """True for a traversal or encoded-separator path we refuse outright (N1).

    Checked before the `/fake-google/` prefix is matched, so no encoding can
    ever smuggle a fake-google control-plane path past the exact-path guard.
    """
    lowered = path.lower()
    return (
        ".." in path
        or "\\" in path
        or "%2f" in lowered
        or "%2e" in lowered
        or "%5c" in lowered
    )


def header_value(head: bytes, name: str) -> str | None:
    """Value of `name` in a raw request head, or None (case-insensitive)."""
    wanted = name.lower().encode("ascii")
    for line in head.split(b"\r\n")[1:]:
        if not line:
            break
        key, sep, value = line.partition(b":")
        if sep and key.strip().lower() == wanted:
            return value.strip().decode("latin-1")
    return None


def relay_websocket(client: Any, upstream: Any) -> None:
    """Copy messages both ways between two websockets connections (T-1113).

    The pinned `websockets` package owns the framing on both legs; this only
    pumps whole messages, so no frame is ever hand-assembled.
    """

    def pump(source: Any, sink: Any) -> None:
        try:
            while True:
                sink.send(source.recv())
        except Exception:
            pass
        finally:
            try:
                sink.close()
            except Exception:
                pass

    thread = threading.Thread(target=pump, args=(client, upstream), daemon=True)
    thread.start()
    pump(upstream, client)
    thread.join()


class AccessControl:
    """HTTP Basic auth (user `demo`) with a global failed-attempt limiter."""

    def __init__(
        self,
        code: str,
        max_failures: int = MAX_FAILURES,
        window: float = FAILURE_WINDOW_S,
        block: float = BLOCK_SECONDS,
    ) -> None:
        self._code = code.encode("utf-8")
        self._max = max_failures
        self._window = window
        self._block = block
        self._failures: list[float] = []
        self._blocked_until = 0.0
        self._lock = threading.Lock()

    def verdict(self, header: str | None) -> str:
        """Return 'ok', 'unauthorized' or 'blocked' for an Authorization header."""
        now = time.monotonic()
        with self._lock:
            if now < self._blocked_until:
                return "blocked"
            if self._valid(header):
                return "ok"
            if not header:
                # No credentials at all is a browser's normal first request (and its icon requests) before it shows the prompt. It is not a guess, and
                # counting it locked the right code out for everyone on the first real phone visit (AAR 3.88).
                return "unauthorized"
            self._failures = [t for t in self._failures if now - t <= self._window]
            self._failures.append(now)
            if len(self._failures) > self._max:
                # More than five failures in the window: block for BLOCK_SECONDS (60 s).
                self._blocked_until = now + self._block
                self._failures = []
                return "blocked"
            return "unauthorized"

    def _valid(self, header: str | None) -> bool:
        if not header or not header.lower().startswith("basic "):
            return False
        try:
            raw = base64.b64decode(header.split(" ", 1)[1], validate=True)
            user, _, password = raw.decode("utf-8").partition(":")
        except (binascii.Error, UnicodeDecodeError, ValueError):
            user, password = "", ""
        # Compare the password in constant time even when the user is wrong.
        return hmac.compare_digest(password.encode("utf-8"), self._code) and user == "demo"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--firebase-json", required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--port-file", default=None)
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="loopback address to bind (default 127.0.0.1)",
    )
    parser.add_argument(
        "--access-code-file",
        default=None,
        help="phone mode: file holding the access code (Basic auth user 'demo')",
    )
    parser.add_argument(
        "--fake-google",
        default=None,
        help="phone mode: loopback fake-google base URL to proxy the authorise path",
    )
    parser.add_argument(
        "--dev-upstream",
        default=None,
        help="dev mode (T-1113): loopback flutter run -d web-server base URL to proxy / to",
    )
    args = parser.parse_args()

    root = Path(args.root).resolve()
    header_blocks = load_headers(Path(args.firebase_json))
    safe_files = index_files(root)
    index_html = root / "index.html"
    api_host, api_port, api_prefix = parse_api(args.api)

    access = None
    if args.access_code_file:
        code = Path(args.access_code_file).read_text().strip()
        access = AccessControl(code)
    fake_google = parse_api(args.fake_google) if args.fake_google else None
    dev_upstream = parse_api(args.dev_upstream) if args.dev_upstream else None

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

        # -- dev-mode websocket handoff (T-1113) -------------------------------
        # A websocket upgrade cannot be answered by BaseHTTPRequestHandler, and
        # by the time its do_GET runs the request bytes are already consumed, so
        # the connection is peeked (non-destructively) first and, when it is an
        # upgrade, handed straight to the `websockets` relay.
        def handle(self) -> None:
            if dev_upstream is not None:
                head = self._peek_request_head()
                if self._is_websocket_upgrade(head):
                    self._serve_dev_websocket(head)
                    return
            super().handle()

        def _peek_request_head(self) -> bytes:
            sock = self.connection
            data = b""
            deadline = time.monotonic() + WEBSOCKET_PEEK_TIMEOUT_S
            while b"\r\n\r\n" not in data:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                ready, _, _ = select.select([sock], [], [], remaining)
                if not ready:
                    break
                try:
                    chunk = sock.recv(WEBSOCKET_PEEK_BYTES, socket.MSG_PEEK)
                except OSError:
                    break
                if not chunk:
                    break
                # MSG_PEEK returns the same bytes until they are consumed; wait
                # for more rather than spinning while the head is still arriving.
                if chunk == data:
                    time.sleep(0.02)
                    continue
                data = chunk
            return data

        @staticmethod
        def _is_websocket_upgrade(head: bytes) -> bool:
            if not head:
                return False
            first_line = head.split(b"\r\n", 1)[0]
            if not first_line.upper().startswith(b"GET "):
                return False
            lowered = head.lower()
            return b"\r\nupgrade:" in lowered and b"websocket" in lowered

        def _serve_dev_websocket(self, head: bytes) -> None:
            assert dev_upstream is not None
            verdict = "ok"
            if access is not None:
                verdict = access.verdict(header_value(head, "Authorization"))
            if verdict != "ok":
                self._deny_websocket(verdict)
                return
            try:
                from websockets.server import ServerProtocol
                from websockets.sync.client import connect
                from websockets.sync.server import ServerConnection
            except ImportError:
                self.send_error(500, "dev mode needs the websockets package")
                return
            host, port, prefix = dev_upstream
            connection = ServerConnection(self.connection, ServerProtocol())
            state: dict[str, Any] = {}

            def process_request(client: Any, request: Any) -> None:
                # The upstream leg is a fresh websockets client connection; the
                # package owns its framing and the browser-facing one alike.
                offered = request.headers.get("Sec-WebSocket-Protocol")
                subprotocols = [p.strip() for p in offered.split(",") if p.strip()] if offered else None
                state["upstream"] = connect(
                    f"ws://{host}:{port}{prefix}{request.path}",  # nosemgrep: python.django.security.injection.tainted-url-host.tainted-url-host, javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
                    subprotocols=subprotocols,
                    max_size=None,
                    open_timeout=30,
                )
                return None

            def process_response(client: Any, request: Any, response: Any) -> None:
                upstream = state.get("upstream")
                subprotocol = getattr(upstream, "subprotocol", None)
                if subprotocol:
                    response.headers["Sec-WebSocket-Protocol"] = subprotocol
                return None

            connection.handshake(
                process_request=process_request,
                process_response=process_response,
            )
            upstream = state.get("upstream")
            if upstream is None:
                return
            try:
                relay_websocket(connection, upstream)
            finally:
                for peer in (upstream, connection):
                    try:
                        peer.close()
                    except Exception:
                        pass

        def _deny_websocket(self, verdict: str) -> None:
            if verdict == "blocked":
                status = 429
            else:
                status = 401
            reason = http.client.responses.get(status, "")
            lines = [f"HTTP/1.1 {status} {reason}"]
            if status == 429:
                lines.append(f"Retry-After: {int(BLOCK_SECONDS)}")
            lines.append('WWW-Authenticate: Basic realm="demo"')
            lines.append("Content-Length: 0")
            lines.append("Connection: close")
            lines.append("")
            lines.append("")
            self.connection.sendall("\r\n".join(lines).encode("ascii"))

        # -- access control (Behaviour 2) --------------------------------------
        def _gate(self) -> bool:
            if access is None:
                return True
            verdict = access.verdict(self.headers.get("Authorization"))
            if verdict == "ok":
                return True
            if verdict == "blocked":
                self.send_response(429)
                self.send_header("Retry-After", str(int(BLOCK_SECONDS)))
            else:
                self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="demo"')
            self.send_header("Content-Length", "0")
            self.end_headers()
            return False

        # -- static files, with the viewport meta guaranteed (Behaviour 3) -----
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
            if ctype == "text/html" and b'name="viewport"' not in body:
                body = ensure_viewport(body)
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self._apply_headers("/" + rel)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        # -- /api proxy --------------------------------------------------------
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
            self._relay(method, api_host, api_port, path, body=body, headers=headers)

        # -- dev-mode app proxy (T-1113) ---------------------------------------
        def _dev_proxy(self, method: str) -> None:
            # The dev server is loopback-only and has no access control of its
            # own: everything reaches it through this front door, which already
            # ran `_gate` and adds the firebase.json headers.
            assert dev_upstream is not None
            path = self.path.split("?", 1)[0]
            sys.stderr.write(f"e2e-host: {method} {path} (dev)\n")
            sys.stderr.flush()
            length = int(self.headers.get("Content-Length", 0) or 0)
            body = self.rfile.read(length) if length else None
            host, port, prefix = dev_upstream
            headers = {"X-Forwarded-For": "127.0.0.1"}
            for name in DEV_FORWARD_REQUEST_HEADERS:
                value = self.headers.get(name)
                if value is not None:
                    headers[name] = value
            self._relay(
                method,
                host,
                port,
                prefix + self.path,
                body=body,
                headers=headers,
                config_headers=path,
            )

        # -- fake-google authorise proxy (Behaviour 0d) ------------------------
        def _fake_google(self, method: str, path: str) -> None:
            if method != "GET" or path != FAKE_GOOGLE_AUTHORISE_PATH or fake_google is None:
                # The control plane stays unreachable, even with the code.
                self.send_error(404)
                return
            host, port, prefix = fake_google
            query = ""
            if "?" in self.path:
                query = "?" + self.path.split("?", 1)[1]
            sys.stderr.write(
                f"e2e-host: GET {FAKE_GOOGLE_AUTHORISE_PATH} (fake-google)\n"
            )
            sys.stderr.flush()
            self._relay("GET", host, port, prefix + "/o/oauth2/v2/auth" + query)

        # -- shared upstream relay --------------------------------------------
        def _relay(
            self,
            method: str,
            host: str,
            port: int,
            path: str,
            body: bytes | None = None,
            headers: dict[str, str] | None = None,
            config_headers: str | None = None,
        ) -> None:
            conn = http.client.HTTPConnection(host, port, timeout=30)
            conn.request(method, path, body=body, headers=headers or {})
            response = conn.getresponse()
            data = response.read()
            self.send_response(response.status, response.reason)
            for key, value in response.getheaders():
                if key.lower() in SKIP_RESPONSE_HEADERS:
                    continue
                # Never relay a header carrying CR/LF (response splitting); drop it rather than rewrite it.
                if any(c in key or c in value for c in "\r\n\0"):
                    sys.stderr.write(
                        f"e2e-host: dropped upstream header with control characters: {key!r}\n"
                    )
                    continue
                self.send_header(key, value)
            # Dev mode keeps the same firebase.json security headers as normal
            # mode (T-1113), matched against the browser-facing path.
            if config_headers is not None:
                self._apply_headers(config_headers)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            conn.close()

        def _is_api(self) -> bool:
            return self.path == "/api" or self.path.startswith("/api/")

        def _route(self, method: str) -> None:
            if not self._gate():
                return
            path = self.path.split("?", 1)[0]
            if path_is_unsafe(path):
                self.send_error(404)
                return
            if self._is_api():
                self._proxy(method)
                return
            if path.startswith("/fake-google/"):
                self._fake_google(method, path)
                return
            if dev_upstream is not None:
                # Dev mode serves the app from `flutter run -d web-server`
                # instead of the static release build (T-1113).
                self._dev_proxy(method)
                return
            if method in ("GET", "HEAD"):
                self._serve_file(self.path.lstrip("/").split("?", 1)[0])
            else:
                self.send_error(404)

        def do_GET(self) -> None:  # noqa: N802
            self._route("GET")

        def do_HEAD(self) -> None:  # noqa: N802
            self._route("HEAD")

        def do_POST(self) -> None:  # noqa: N802
            self._route("POST")

        def do_PUT(self) -> None:  # noqa: N802
            self._route("PUT")

        def do_PATCH(self) -> None:  # noqa: N802
            self._route("PATCH")

        def do_DELETE(self) -> None:  # noqa: N802
            self._route("DELETE")

    # Bind the address the caller asked for. An IPv6 literal (::1) needs an
    # AF_INET6 socket; everything else is IPv4. Previously this was the literal
    # 127.0.0.1 and --host only changed the printed URL (T-1108a F2).
    family = socket.AF_INET6 if ":" in args.host else socket.AF_INET

    class _Server(ThreadingHTTPServer):
        address_family = family

    server = _Server((args.host, args.port), Handler)
    if args.port_file:
        Path(args.port_file).write_text(f"{server.server_address[1]}\n")
    print(f"PORT {server.server_address[1]}", flush=True)
    server.serve_forever()


def ensure_viewport(body: bytes) -> bytes:
    """Return `body` with the standard viewport meta tag present (Behaviour 3)."""
    if b"<head>" in body:
        return body.replace(b"<head>", b"<head>" + VIEWPORT_META, 1)
    if b"</head>" in body:
        return body.replace(b"</head>", VIEWPORT_META + b"</head>", 1)
    return VIEWPORT_META + body


if __name__ == "__main__":
    main()
