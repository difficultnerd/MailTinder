"""Tests for tools/gmail_sandbox_reauth.py (T-1106).

Every test fails if the behaviour it names is removed. No network and no
credentials: the OAuth endpoints are a local `http.server` and a capture runner
stands in for `gh`.
"""

import base64
import http.server
import json
import os
import sys
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gmail_sandbox_reauth as reauth  # noqa: E402

CLIENT_ID = "client-id.apps.googleusercontent.com"
CLIENT_SECRET = "client-secret"


class CapturedRun:
    """Stands in for `subprocess.run`; captures the command and the stdin text."""

    def __init__(self, returncode=0):
        self.returncode = returncode
        self.calls = []

    def __call__(self, command, input=None, **kwargs):
        self.calls.append((command, input))

        class Completed:
            returncode = self.returncode
            stdout = ""
            stderr = ""

        return Completed()


def id_token_with_email(email):
    """A minimal unsigned-shaped ID token carrying an `email` claim."""
    claims = {"email": email}
    body = base64.urlsafe_b64encode(json.dumps(claims).encode("utf-8")).rstrip(b"=").decode("ascii")
    return f"header.{body}.signature"


class _TokenServer(http.server.HTTPServer):
    """A token endpoint with the scripted response and the last request body."""

    response: dict = {}
    status: int = 200
    body_bytes: bytes = b""


class _TokenHandler(http.server.BaseHTTPRequestHandler):
    """Answers the token exchange with a scripted body; GET redirects nowhere."""

    def do_POST(self):  # noqa: N802 - the stdlib's name
        length = int(self.headers.get("Content-Length", "0"))
        self.server.body_bytes = self.rfile.read(length)
        payload = json.dumps(self.server.response).encode("utf-8")
        status = self.server.status
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format, *args):  # noqa: A002 - the stdlib's name
        return


def start_token_server(response, status=200):
    """A local token endpoint returning `response`. Returns (server, url)."""
    server = _TokenServer(("127.0.0.1", 0), _TokenHandler)
    server.response = response
    server.status = status
    thread = threading.Thread(target=server.handle_request, daemon=True)
    thread.start()
    return server, f"http://127.0.0.1:{server.server_port}/token"


class AuthUrlTest(unittest.TestCase):
    """The authorisation URL requests exactly the documented scope set."""

    def auth_params(self):
        url = reauth.build_auth_url(CLIENT_ID, "http://127.0.0.1:9999/", "state-1", "challenge-1")
        parsed = urllib.parse.urlparse(url)
        self.assertEqual(parsed.scheme, "https")
        self.assertEqual(parsed.netloc, "accounts.google.com")
        return urllib.parse.parse_qs(parsed.query)

    def test_auth_url_has_offline_access_and_consent(self):
        params = self.auth_params()
        self.assertEqual(params["access_type"], ["offline"])
        self.assertEqual(params["prompt"], ["consent"])
        self.assertEqual(params["response_type"], ["code"])
        self.assertEqual(params["code_challenge_method"], ["S256"])

    def test_auth_url_requests_modify_openid_email_and_never_send(self):
        scopes = self.auth_params()["scope"][0].split(" ")
        self.assertIn("gmail.modify", scopes)
        self.assertIn("openid", scopes)
        self.assertIn("email", scopes)
        self.assertNotIn("gmail.send", scopes)
        self.assertNotIn("gmail.insert", scopes)
        self.assertEqual(sorted(scopes), sorted(reauth.SCOPES))

    def test_auth_url_carries_state_and_pkce_challenge(self):
        params = self.auth_params()
        self.assertEqual(params["state"], ["state-1"])
        self.assertEqual(params["code_challenge"], ["challenge-1"])
        self.assertEqual(params["redirect_uri"], ["http://127.0.0.1:9999/"])

    def test_pkce_challenge_is_the_s256_of_its_verifier(self):
        import hashlib

        verifier, challenge = reauth.pkce_pair()
        expected = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
        self.assertEqual(challenge, expected.rstrip(b"=").decode())


class TokenExchangeTest(unittest.TestCase):
    """The code exchange is a POST to the token endpoint with the verifier."""

    def test_exchange_code_posts_the_verifier_and_secret(self):
        server, url = start_token_server({"access_token": "at", "refresh_token": "rt"})
        original = reauth.TOKEN_ENDPOINT
        reauth.TOKEN_ENDPOINT = url
        try:
            tokens = reauth.exchange_code(
                CLIENT_ID, CLIENT_SECRET, "code-1", "verifier-1", "http://127.0.0.1:9999/"
            )
        finally:
            reauth.TOKEN_ENDPOINT = original
        self.assertEqual(tokens["refresh_token"], "rt")
        body = urllib.parse.parse_qs(server.body_bytes.decode("ascii"))
        self.assertEqual(body["grant_type"], ["authorization_code"])
        self.assertEqual(body["code"], ["code-1"])
        self.assertEqual(body["code_verifier"], ["verifier-1"])
        self.assertEqual(body["client_secret"], [CLIENT_SECRET])
        self.assertEqual(body["redirect_uri"], ["http://127.0.0.1:9999/"])

    def test_exchange_code_reports_an_http_error_without_leaking_it(self):
        server, url = start_token_server({"error": "invalid_grant"}, status=400)
        original = reauth.TOKEN_ENDPOINT
        reauth.TOKEN_ENDPOINT = url
        try:
            with self.assertRaises(reauth.ReauthError) as caught:
                reauth.exchange_code(
                    CLIENT_ID, CLIENT_SECRET, "code", "verifier", "http://127.0.0.1:9999/"
                )
        finally:
            reauth.TOKEN_ENDPOINT = original
        server.server_close()
        self.assertIn("400", str(caught.exception))
        self.assertNotIn(CLIENT_SECRET, str(caught.exception))


class SecretTest(unittest.TestCase):
    """The refresh token reaches `gh secret set` on stdin, never on argv."""

    def test_refresh_token_is_piped_to_gh_secret_set_on_stdin(self):
        runner = CapturedRun()
        reauth.set_refresh_token("REFRESH-CANARY", runner=runner)
        self.assertEqual(len(runner.calls), 1)
        command, stdin = runner.calls[0]
        self.assertEqual(
            command, ["gh", "secret", "set", reauth.REFRESH_HANDLE, "--env", reauth.ENVIRONMENT]
        )
        self.assertEqual(stdin, "REFRESH-CANARY")
        self.assertNotIn("REFRESH-CANARY", " ".join(command))

    def test_a_failed_gh_exit_is_reported(self):
        with self.assertRaises(reauth.ReauthError):
            reauth.set_refresh_token("REFRESH-CANARY", runner=CapturedRun(returncode=1))


class EmailClaimTest(unittest.TestCase):
    """The reported address comes from the ID token claim, never from stdout."""

    def test_email_claim_is_read_from_the_id_token(self):
        self.assertEqual(
            reauth._claim_email(id_token_with_email("sandbox@example.com")),
            "sandbox@example.com",
        )

    def test_a_missing_or_malformed_id_token_gives_no_address(self):
        self.assertIsNone(reauth._claim_email(None))
        self.assertIsNone(reauth._claim_email("not-a-jwt"))
        self.assertIsNone(reauth._claim_email("header.not-base64!!.sig"))


class CallbackTest(unittest.TestCase):
    """The loopback server answers one redirect and validates `state`."""

    def test_callback_returns_the_query_parameters(self):
        listener = reauth.socket.socket()
        listener.setsockopt(reauth.socket.SOL_SOCKET, reauth.socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]

        def redirect():
            target = f"http://127.0.0.1:{port}/?code=code-1&state=state-1"
            # `target` is this test's own loopback socket, bound just above.
            # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected, python.lang.security.audit.dynamic-urllib-use-detected
            urllib.request.urlopen(target).read()

        thread = threading.Thread(target=redirect, daemon=True)
        thread.start()
        try:
            callback = reauth.wait_for_callback(listener, timeout_s=10)
        finally:
            listener.close()
        self.assertEqual(callback["code"], ["code-1"])
        self.assertEqual(callback["state"], ["state-1"])

    def test_an_error_callback_is_reported(self):
        listener = reauth.socket.socket()
        listener.setsockopt(reauth.socket.SOL_SOCKET, reauth.socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]

        def redirect():
            target = f"http://127.0.0.1:{port}/?error=access_denied"
            try:
                # `target` is this test's own loopback socket, bound just above.
                # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected, python.lang.security.audit.dynamic-urllib-use-detected
                urllib.request.urlopen(target).read()
            except urllib.error.HTTPError:
                pass

        thread = threading.Thread(target=redirect, daemon=True)
        thread.start()
        try:
            with self.assertRaises(reauth.ReauthError) as caught:
                reauth.wait_for_callback(listener, timeout_s=10)
        finally:
            listener.close()
        self.assertIn("access_denied", str(caught.exception))


class EnvironmentTest(unittest.TestCase):
    """The client ID and secret come from the environment or the flags."""

    def setUp(self):
        self.saved = {name: os.environ.get(name) for name in reauth.CLIENT_ID_VARS + reauth.CLIENT_SECRET_VARS}
        for name in self.saved:
            os.environ.pop(name, None)

    def tearDown(self):
        for name, value in self.saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value

    def test_missing_client_id_is_reported_and_never_solved_by_a_default(self):
        with self.assertRaises(reauth.ReauthError) as caught:
            reauth._from_env(reauth.CLIENT_ID_VARS, None, "the sandbox client ID")
        self.assertIn("GMAIL_SANDBOX_CLIENT_ID", str(caught.exception))

    def test_environment_variables_are_read(self):
        os.environ["GMAIL_SANDBOX_CLIENT_ID"] = CLIENT_ID
        self.assertEqual(
            reauth._from_env(reauth.CLIENT_ID_VARS, None, "the sandbox client ID"), CLIENT_ID
        )

    def test_a_flag_wins_over_the_environment(self):
        os.environ["GMAIL_SANDBOX_CLIENT_ID"] = "from-env"
        self.assertEqual(
            reauth._from_env(reauth.CLIENT_ID_VARS, "from-flag", "the sandbox client ID"),
            "from-flag",
        )


class EndToEndFlowTest(unittest.TestCase):
    """The whole helper, against a local token endpoint and a capture runner.

    The one redirect is issued from a daemon thread: a plain `urlopen` waits for
    the connection close, and the helper's handler answers the real browser, not
    a client that insists on a close.
    """

    def with_server(self, response):
        server, url = start_token_server(response)
        original = reauth.TOKEN_ENDPOINT
        reauth.TOKEN_ENDPOINT = url
        self.addCleanup(setattr, reauth, "TOKEN_ENDPOINT", original)
        self.addCleanup(server.server_close)
        return server

    def redirect_thread(self, target, state, opened):
        """Record the opened URL and issue its callback from a daemon thread."""
        opened.append(target)
        query = urllib.parse.parse_qs(urllib.parse.urlparse(target).query)
        redirect_uri = query["redirect_uri"][0]

        def go():
            try:
                target = f"{redirect_uri}?code=code-1&state={state(query)}"
                # `redirect_uri` is the helper's own loopback callback.
                # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected, python.lang.security.audit.dynamic-urllib-use-detected
                urllib.request.urlopen(target, timeout=5).read()
            except Exception:  # the helper answered; the close is not the assertion
                pass

        threading.Thread(target=go, daemon=True).start()

    def test_reauthorise_opens_the_browser_and_stores_the_token(self):
        self.with_server(
            {
                "access_token": "at",
                "refresh_token": "REFRESH-CANARY",
                "id_token": id_token_with_email("sandbox@example.com"),
            }
        )
        runner = CapturedRun()
        opened = []
        address = reauth.reauthorise(
            CLIENT_ID,
            CLIENT_SECRET,
            open_url=lambda target: self.redirect_thread(
                target, lambda q: q["state"][0], opened
            ),
            runner=runner,
        )
        self.assertEqual(address, "sandbox@example.com")
        self.assertEqual(len(opened), 1)
        self.assertIn("/o/oauth2/v2/auth", opened[0])
        self.assertEqual(runner.calls[0][1], "REFRESH-CANARY")

    def test_reauthorise_reports_a_state_mismatch(self):
        self.with_server({"refresh_token": "REFRESH-CANARY"})
        opened = []
        with self.assertRaises(reauth.ReauthError) as caught:
            reauth.reauthorise(
                CLIENT_ID,
                CLIENT_SECRET,
                open_url=lambda target: self.redirect_thread(
                    target, lambda q: "wrong-state", opened
                ),
                runner=CapturedRun(),
            )
        self.assertIn("state", str(caught.exception))

    def test_reauthorise_reports_a_missing_refresh_token(self):
        self.with_server({"access_token": "at"})
        opened = []
        with self.assertRaises(reauth.ReauthError) as caught:
            reauth.reauthorise(
                CLIENT_ID,
                CLIENT_SECRET,
                open_url=lambda target: self.redirect_thread(
                    target, lambda q: q["state"][0], opened
                ),
                runner=CapturedRun(),
            )
        self.assertIn("refresh token", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
