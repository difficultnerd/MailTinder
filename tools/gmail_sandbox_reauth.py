#!/usr/bin/env python3
"""Trade a one-time Google authorisation code for the Gmail sandbox refresh token (T-1106).

James runs this once for the sandbox account, and again weekly while the OAuth
consent screen is in Testing mode (Google expires those refresh tokens after 7
days), or whenever the nightly run opens "Gmail sandbox needs re-authorising".

Stdlib only. What it does, in order:

1. Reads the OAuth client ID and secret from the environment (or flags).
2. Starts a loopback server on 127.0.0.1:0 (an ephemeral port).
3. Opens the browser to Google's authorisation URL with PKCE, `access_type=offline`
   and `prompt=consent`, requesting `gmail.modify`, `openid` and `email`.
   `gmail.modify` covers `messages.import`; `gmail.send` is deliberately NOT
   requested, because nothing in the live run sends.
4. Waits for the redirect, checks `state`, and exchanges the code.
5. Pipes the refresh token into
   `gh secret set GMAIL_SANDBOX_REFRESH_TOKEN --env live-sandbox` on stdin.

The refresh token, the access token and the ID token are never printed and never
written to disk.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import http.server
import json
import os
import secrets
import socket
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request
import webbrowser

AUTH_ENDPOINT = "https://accounts.google.com/o/oauth2/v2/auth"
TOKEN_ENDPOINT = "https://oauth2.googleapis.com/token"
# `gmail.modify` covers `users.messages.import` (verify at the first live run;
# add `gmail.insert` only if Google rejects the import). No `gmail.send`: the
# live run imports seed mail, it never sends.
SCOPES = ("gmail.modify", "openid", "email")
REFRESH_HANDLE = "GMAIL_SANDBOX_REFRESH_TOKEN"
ENVIRONMENT = "live-sandbox"
CALLBACK_TIMEOUT_S = 300
CLIENT_ID_VARS = ("GMAIL_SANDBOX_CLIENT_ID", "OAUTH_CLIENT_ID")
CLIENT_SECRET_VARS = ("GMAIL_SANDBOX_CLIENT_SECRET", "OAUTH_CLIENT_SECRET")


class ReauthError(Exception):
    """A failure that stops the re-authorisation with a clear message."""


def pkce_pair() -> tuple[str, str]:
    """A PKCE verifier and its S256 challenge (RFC 7636)."""
    verifier = secrets.token_urlsafe(32)
    digest = hashlib.sha256(verifier.encode("ascii")).digest()
    challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    return verifier, challenge


def build_auth_url(client_id: str, redirect_uri: str, state: str, challenge: str) -> str:
    """The authorisation URL. `access_type=offline` is what yields a refresh token."""
    query = urllib.parse.urlencode(
        {
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "response_type": "code",
            "scope": " ".join(SCOPES),
            "access_type": "offline",
            "prompt": "consent",
            "state": state,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
        }
    )
    return f"{AUTH_ENDPOINT}?{query}"


def exchange_code(
    client_id: str,
    client_secret: str,
    code: str,
    verifier: str,
    redirect_uri: str,
    opener=urllib.request.urlopen,
) -> dict:
    """POST the code to Google and return the token response."""
    body = urllib.parse.urlencode(
        {
            "grant_type": "authorization_code",
            "code": code,
            "code_verifier": verifier,
            "client_id": client_id,
            "client_secret": client_secret,
            "redirect_uri": redirect_uri,
        }
    ).encode("ascii")
    request = urllib.request.Request(
        TOKEN_ENDPOINT,
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        with opener(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:  # a bad code or a wrong secret
        raise ReauthError(f"token exchange failed with HTTP {error.code}") from None


class _Handler(http.server.BaseHTTPRequestHandler):
    """Answers the one redirect the browser makes, then stops the server."""

    def do_GET(self) -> None:  # noqa: N802 - the stdlib's name
        query = urllib.parse.urlparse(self.path).query
        params = urllib.parse.parse_qs(query)
        self.server.callback = params  # type: ignore[attr-defined]
        body = b"You can close this tab and return to the terminal."
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        # The real browser never needs this, but a plain `urlopen` in the tests
        # waits for the close rather than reading Content-Length alone.
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the stdlib's name
        """Keep the console clean, and never log a query."""
        return


def wait_for_callback(listener: socket.socket, timeout_s: int = CALLBACK_TIMEOUT_S) -> dict:
    """Serve one redirect on `listener` and return its query parameters."""
    server = http.server.HTTPServer(listener.getsockname(), _Handler, bind_and_activate=False)
    server.socket = listener
    server.callback = {}  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.handle_request, daemon=True)
    thread.start()
    thread.join(timeout_s)
    server.server_close()
    callback = getattr(server, "callback", {})
    if not callback:
        raise ReauthError("no authorisation callback arrived")
    if "error" in callback:
        raise ReauthError(f"Google refused the authorisation: {callback['error'][0]}")
    return callback


def set_refresh_token(token: str, runner=None) -> None:
    """Pipe the refresh token into `gh secret set` on stdin. Never on argv."""
    import subprocess  # local: only this path shells out

    runner = runner or subprocess.run
    command = ["gh", "secret", "set", REFRESH_HANDLE, "--env", ENVIRONMENT]
    completed = runner(command, input=token, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        # `capture_output` keeps whatever gh printed out of the terminal.
        raise ReauthError("gh secret set failed; is gh authenticated for this repository?")


def _claim_email(id_token: str | None) -> str | None:
    """The `email` claim of an ID token, or None. The token itself is not kept."""
    if not id_token:
        return None
    parts = id_token.split(".")
    if len(parts) < 2:
        return None
    try:
        padded = parts[1] + "=" * (-len(parts[1]) % 4)
        claims = json.loads(base64.urlsafe_b64decode(padded))
    except (ValueError, json.JSONDecodeError):
        return None
    email = claims.get("email")
    return email if isinstance(email, str) else None


def reauthorise(client_id: str, client_secret: str, open_url=webbrowser.open, runner=None) -> str:
    """The whole flow. Returns the sandbox address (never a token)."""
    verifier, challenge = pkce_pair()
    state = secrets.token_urlsafe(24)
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]
    redirect_uri = f"http://127.0.0.1:{port}/"
    url = build_auth_url(client_id, redirect_uri, state, challenge)
    print(f"Opening the browser. If it does not open, visit:\n{url}\n")
    open_url(url)
    callback = wait_for_callback(listener)
    if callback.get("state", [None])[0] != state:
        raise ReauthError("the callback state did not match; re-run the helper")
    code = callback.get("code", [None])[0]
    if not code:
        raise ReauthError("the callback carried no authorisation code")
    tokens = exchange_code(client_id, client_secret, code, verifier, redirect_uri)
    refresh = tokens.get("refresh_token")
    if not refresh:
        raise ReauthError(
            "Google returned no refresh token; re-run with prompt=consent, which this helper sets"
        )
    set_refresh_token(refresh, runner=runner)
    email = _claim_email(tokens.get("id_token"))
    print(
        f"Stored {REFRESH_HANDLE} in the {ENVIRONMENT} environment"
        + (f" for {email}." if email else ".")
    )
    print("The refresh token itself was never printed.")
    return email or ""


def _from_env(names: tuple[str, ...], flag: str | None, what: str) -> str:
    """A value from the flag, else the first set environment variable."""
    if flag:
        return flag
    for name in names:
        value = os.environ.get(name)
        if value and value.strip():
            return value
    raise ReauthError(f"{what} is not set (looked for {', '.join(names)}, or pass the flag)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client-id", default=None, help="Desktop OAuth client ID")
    parser.add_argument("--client-secret", default=None, help="Desktop OAuth client secret")
    args = parser.parse_args(argv)
    try:
        client_id = _from_env(CLIENT_ID_VARS, args.client_id, "the sandbox client ID")
        client_secret = _from_env(
            CLIENT_SECRET_VARS, args.client_secret, "the sandbox client secret"
        )
        reauthorise(client_id, client_secret)
    except ReauthError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
