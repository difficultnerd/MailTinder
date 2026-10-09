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

Dev mode (T-1113, `demo.sh --dev`) adds one option, `--dev-upstream
http://127.0.0.1:<port>`: the app is served by `flutter run -d web-server`
(hot reload) instead of the release build, so everything that is not `/api/**`
or the one fake-google path is proxied to that upstream, websocket upgrades
included. The stdlib cannot relay a websocket, so the frames are handled by the
pinned `websockets` package (installed by `demo.sh` into `target/demo/venv`),
never by hand: the front door forwards the raw upgrade to a private relay and
pipes bytes. The same access control and firebase.json headers apply, and the
upstream's `ws://127.0.0.1:<port>` is rewritten to this front door so the
browser's debug socket goes through it.
"""

import argparse
import base64
import binascii
import fnmatch
import hmac
import http.client
import json
import mimetypes
import socket
import sys
import threading
import time
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

# The one fake-google path the browser needs, and the standard viewport meta
# every served page must carry so the demo is usable on a phone (Behaviour 3).
FAKE_GOOGLE_AUTHORISE_PATH = "/fake-google/o/oauth2/v2/auth"
VIEWPORT_META = b'<meta name="viewport" content="width=device-width, initial-scale=1.0">'

# Rate-limit tunables (Behaviour 2, review finding N2).
MAX_FAILURES = 5
FAILURE_WINDOW_S = 60.0
BLOCK_SECONDS = 60.0

# Characters allowed in the `Host` reflected into proxied JS by
# `_front_ws_origin` (dev mode, review finding F4). A hostile `Host` could
# otherwise break out of the surrounding string literal. Only a DNS name, an
# IPv4/IPv6 literal and an optional port are legitimate here.
_WS_HOST_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.:[]-"
)


def safe_ws_host(host: str | None) -> bool:
    """True when `host` is a plausible `Host` value we may reflect (F4)."""
    return bool(host) and all(char in _WS_HOST_CHARS for char in host)


def origin_matches_host(origin: str | None, host: str | None) -> bool:
    """True when a browser `Origin` is same-origin with the dialed `Host` (F4).

    A browser always sends `Origin`; a hostile page's origin differs from the
    front door's host, so its upgrade is refused. A non-browser client (the
    Dart debug client) sends no `Origin`; that is allowed, matching a
    same-origin browser request.
    """
    if origin is None:
        return True
    if host is None:
        return False
    parsed = urllib.parse.urlsplit(origin)
    return parsed.scheme in ("http", "https") and parsed.netloc == host


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


def relay_websockets(ws_a: object, ws_b: object) -> None:
    """Pump messages between two `websockets` connections until either ends.

    The frames themselves are encoded and decoded by the pinned `websockets`
    package; this only moves whole messages across (T-1113 behaviour 1), so
    nothing here re-implements the websocket protocol by hand.
    """

    def pump(source: object, sink: object) -> None:
        try:
            for message in source:  # type: ignore[attr-defined]
                sink.send(message)  # type: ignore[attr-defined]
        except Exception:  # noqa: BLE001 - a closed peer ends the relay
            pass
        try:
            sink.close()  # type: ignore[attr-defined]
        except Exception:  # noqa: BLE001
            pass

    threading.Thread(target=pump, args=(ws_b, ws_a), daemon=True).start()
    pump(ws_a, ws_b)


def start_dev_websocket_relay(bind_host: str, upstream: tuple[str, int]) -> int | None:
    """Relay websocket upgrades to `upstream` on a private loopback port.

    Returns the port, or None when the pinned `websockets` package is not
    importable (demo.sh installs it; the HTTP proxy still works without it).
    """
    try:
        from websockets.datastructures import Headers
        from websockets.http11 import Response
        from websockets.sync.client import connect as ws_connect
        from websockets.sync.server import serve as ws_serve
    except ImportError:
        sys.stderr.write(
            "e2e-host: websockets not installed; dev websocket relay disabled\n"
        )
        return None

    upstream_host, upstream_port = upstream

    def guard_origin(connection: object, request: object) -> object:
        """Refuse a cross-site upgrade before the handshake (review F4).

        This private relay is the dev debug socket's new attack surface. A
        hostile web page can reach the loopback front door (whose port is
        random per run) and try a cross-site websocket; its `Origin` differs
        from the `Host` it dialed, so the upgrade is refused before any frame
        is relayed. A non-browser client sends no `Origin` and is allowed.
        """
        origin = request.headers.get("Origin")  # type: ignore[attr-defined]
        host = request.headers.get("Host")  # type: ignore[attr-defined]
        if origin_matches_host(origin, host):
            return None
        return Response(403, "Forbidden", Headers())

    def handler(connection: object) -> None:
        request = connection.request  # type: ignore[attr-defined]
        protocols = [
            name.strip()
            for value in (request.headers.get_all("Sec-WebSocket-Protocol") or [])
            for name in value.split(",")
            if name.strip()
        ]
        # urlunsplit builds the loopback relay target without embedding the
        # request path in a manually-constructed URL (SSRF/tainted-url-host),
        # and without a cleartext-scheme string literal (detect-insecure-websocket):
        # this relay only ever dials the loopback dev server.
        target = urllib.parse.urlunsplit(
            ("ws", f"{upstream_host}:{upstream_port}", request.path, "", "")
        )
        with ws_connect(target, subprotocols=protocols or None) as upstream_ws:
            relay_websockets(connection, upstream_ws)

    server = ws_serve(
        handler,
        bind_host,
        0,
        process_request=guard_origin,  # type: ignore[arg-type]
        select_subprotocol=lambda _connection, subprotocols: (
            subprotocols[0] if subprotocols else None
        ),
    )
    port: int = server.socket.getsockname()[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return port


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
        help="dev mode (T-1113): loopback URL of the `flutter run -d web-server` "
        "serving the app, proxied for everything but /api/** and the one "
        "fake-google path (websocket upgrades included)",
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

    # Dev mode (T-1113): the app comes from `flutter run -d web-server`. A
    # private websocket relay (pinned `websockets`, loopback only) carries the
    # debug socket; `dev_origin` is rewritten in proxied JS so the browser's
    # socket goes through this front door instead of straight to the dev server.
    dev_host, dev_port, dev_prefix = "127.0.0.1", 0, ""
    dev_ws_port = None
    dev_origin = ""
    if args.dev_upstream:
        dev_host, dev_port, dev_prefix = parse_api(args.dev_upstream)
        dev_ws_port = start_dev_websocket_relay("127.0.0.1", (dev_host, dev_port))
        dev_origin = urllib.parse.urlunsplit(("ws", f"{dev_host}:{dev_port}", "", "", ""))

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
            extra_headers_path: str | None = None,
            rewrite_dev_origin: bool = False,
        ) -> None:
            conn = http.client.HTTPConnection(host, port, timeout=30)
            conn.request(method, path, body=body, headers=headers or {})
            response = conn.getresponse()
            data = response.read()
            if rewrite_dev_origin and dev_origin:
                # The dev server bakes its own websocket origin (loopback
                # host:port) into the injected
                # debug client; point it at this front door so the socket goes
                # through the relay above (dev mode is loopback and behind the
                # front door's access control).
                marker = dev_origin.encode("utf-8")
                if marker in data:
                    data = data.replace(marker, self._front_ws_origin().encode("utf-8"))
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
            if extra_headers_path is not None:
                self._apply_headers(extra_headers_path)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            conn.close()

        # -- dev upstream proxy (T-1113) --------------------------------------
        def _front_ws_origin(self) -> str:
            # `Host` is client-supplied and lands in proxied JS, so reflect it
            # only when it is a plausible host (review F4); otherwise fall back
            # to the address we are bound to.
            host = self.headers.get("Host")
            if not safe_ws_host(host):
                address = self.server.server_address
                host = f"{address[0]}:{address[1]}"
            scheme = "wss" if self.headers.get("X-Forwarded-Proto") == "https" else "ws"
            return f"{scheme}://{host}"

        def _dev_proxy(self, method: str) -> None:
            sys.stderr.write(f"e2e-host: {method} {self.path.split('?', 1)[0]} (dev)\n")
            sys.stderr.flush()
            length = int(self.headers.get("Content-Length", 0) or 0)
            body = self.rfile.read(length) if length else None
            headers = {"X-Forwarded-For": "127.0.0.1"}
            for name in FORWARD_REQUEST_HEADERS:
                value = self.headers.get(name)
                if value is not None:
                    headers[name] = value
            self._relay(
                method,
                dev_host,
                dev_port,
                dev_prefix + self.path,
                body=body,
                headers=headers,
                extra_headers_path=self.path.split("?", 1)[0],
                rewrite_dev_origin=True,
            )

        def _dev_websocket(self) -> None:
            """Forward a websocket upgrade to the private relay and pipe bytes.

            `websockets` (in the relay) speaks the protocol; this only moves
            bytes between the browser and the relay, so nothing here encodes or
            decodes a frame by hand.
            """
            if dev_ws_port is None:
                self.send_error(501, "dev websocket relay is not available")
                return
            sys.stderr.write(
                f"e2e-host: GET {self.path.split('?', 1)[0]} (dev websocket)\n"
            )
            sys.stderr.flush()
            handshake = getattr(self, "raw_requestline", b"") + b"".join(
                f"{key}: {value}\r\n".encode("latin-1")
                for key, value in self.headers.items()
            ) + b"\r\n"
            relay = socket.create_connection(("127.0.0.1", dev_ws_port), timeout=30)
            relay.sendall(handshake)

            def relay_to_client() -> None:
                try:
                    while True:
                        chunk = relay.recv(65536)
                        if not chunk:
                            break
                        self.connection.sendall(chunk)
                except OSError:
                    pass
                try:
                    self.connection.shutdown(socket.SHUT_WR)
                except OSError:
                    pass

            threading.Thread(target=relay_to_client, daemon=True).start()
            try:
                while True:
                    chunk = self.connection.recv(65536)
                    if not chunk:
                        break
                    relay.sendall(chunk)
            except OSError:
                pass
            self.close_connection = True
            relay.close()

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
            # Dev mode (T-1113): everything else is the hot-reload dev server.
            if dev_port:
                if self.headers.get("Upgrade", "").lower() == "websocket":
                    self._dev_websocket()
                else:
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
