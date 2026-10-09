#!/usr/bin/env python3
"""demo_smoke.py -- the headline user story of the demo, exercised the way a BROWSER does it, against a RUNNING demo.

Why this exists (AAR 3.89, CASE-STUDY lesson 20): the first phone-demo URL failed three ways in the owner's browser because nothing had ever driven the
sign-in through the public front door: credential-less requests were counted as bad guesses, and the OAuth client was registered for the local origin so
"Continue with Google" returned invalid_request. Run this BEFORE any demo link is handed to anyone, and in CI against the stub-tunnel stack.

    scripts/demo_smoke.py --url URL --code CODE --invite-token TOKEN [--origin ORIGIN] [--no-redeem]

  --url           where to connect (the public tunnel URL, or the loopback front door)
  --origin        the app origin the api expects in `Origin` and in redirect_uri (default: --url); in CI the connection is loopback but the origin is the
                  tunnel's public URL
  --no-redeem     stop after the authorise step so the invite is not redeemed. WARNING: the authorise step still consumes fake Google's single-use
                  scripted login (`next-login`), so a LIVE demo must be re-seeded (scripts/demo_seed.py) after any run; never run this against a demo
                  a person is about to use. Use it on a throwaway demo, or run it, then re-seed, then hand over the fresh invite.

Steps (each prints `ok:` or the script exits 1 with `FAIL:`):
  1. a burst of credential-less requests (/, favicon, icons) is answered 401, never 429  (a browser sends these before it shows the prompt)
  2. the page loads with the right code and carries the viewport meta tag
  3. /api/v1/session gives an anonymous session and a CSRF token
  4. POST /api/v1/auth/google/start (intent join + invite) returns an authorization_url on --origin that goes through the front door
  5. the authorise step REDIRECTS (302) to --origin /api/v1/auth/google/callback (a 400 here is the "invalid request" bug)
  6. (unless --no-redeem) the callback redirects to #/auth/result?outcome=joined and the session is then authenticated with a mailbox
  7. the fake-google control plane is NOT reachable through the front door (404)
"""
import argparse
import base64
import http.client
import json
import re
import sys
import urllib.parse


class Fail(Exception):
    pass


class Client:
    def __init__(self, url: str, code: str, origin: str) -> None:
        self.base = urllib.parse.urlsplit(url.rstrip("/"))
        self.origin = origin.rstrip("/")
        self.auth = "Basic " + base64.b64encode(f"demo:{code}".encode()).decode()
        self.cookies: dict[str, str] = {}

    def _conn(self):
        cls = http.client.HTTPSConnection if self.base.scheme == "https" else http.client.HTTPConnection
        return cls(self.base.hostname, self.base.port, timeout=30)

    def request(self, method, path, *, creds=True, body=None, headers=None):
        h = {"Accept": "*/*", "User-Agent": "demo-smoke"}
        if creds:
            h["Authorization"] = self.auth
        if self.cookies:
            h["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        h.update(headers or {})
        conn = self._conn()
        try:
            conn.request(method, path, body=body, headers=h)
            r = conn.getresponse()
            data = r.read()
            for line in r.msg.get_all("Set-Cookie") or []:
                name, _, rest = line.partition("=")
                self.cookies[name.strip()] = rest.split(";", 1)[0]
            return r.status, {k.lower(): v for k, v in r.getheaders()}, data
        finally:
            conn.close()


def ok(msg: str) -> None:
    print(f"ok: {msg}")


def need(cond: bool, msg: str) -> None:
    if not cond:
        raise Fail(msg)


def run(args) -> None:
    origin = (args.origin or args.url).rstrip("/")
    c = Client(args.url, args.code, origin)

    # 1. what a browser does before it shows the sign-in prompt
    codes = [c.request("GET", p, creds=False)[0] for p in ["/", "/favicon.ico", "/favicon.png", "/icons/Icon-192.png", "/apple-touch-icon.png", "/", "/", "/"]]
    need(all(s == 401 for s in codes), f"credential-less requests must be 401 and never lock the right code out, got {codes}")
    ok(f"8 credential-less requests answered 401 {codes}")

    # 2. the page, with the right code
    status, _, page = c.request("GET", "/")
    need(status == 200, f"page with the right code returned {status} (locked out?)")
    need(b"viewport" in page, "page has no viewport meta tag")
    ok("page loads with the right code and has the viewport meta tag")

    # 3. session + CSRF
    status, _, body = c.request("GET", "/api/v1/session")
    need(status == 200, f"/api/v1/session returned {status}")
    session = json.loads(body)
    csrf = session.get("csrf_token")
    need(session.get("state") == "anonymous" and csrf, f"expected an anonymous session with a csrf token, got {session}")
    ok("anonymous session and CSRF token")

    # 4. start sign-in the way the app does
    payload = json.dumps({"intent": "join", "invite_token": args.invite_token, "mailbox_id": None})
    status, _, body = c.request(
        "POST",
        "/api/v1/auth/google/start",
        body=payload,
        headers={"Origin": origin, "X-CSRF-Token": csrf, "Content-Type": "application/json", "Idempotency-Key": "demo-smoke-1"},
    )
    need(status == 200, f"auth/google/start returned {status}: {body[:200]!r}")
    auth_url = json.loads(body).get("authorization_url", "")
    need(auth_url.startswith(origin + "/fake-google/"), f"authorization_url must go through the front door on {origin}, got {auth_url!r}")
    redirect_uri = urllib.parse.parse_qs(urllib.parse.urlsplit(auth_url).query).get("redirect_uri", [""])[0]
    need(redirect_uri == origin + "/api/v1/auth/google/callback", f"redirect_uri is {redirect_uri!r}")
    ok(f"sign-in started; authorization_url on {origin}, redirect_uri {redirect_uri}")

    # 5. the authorise step must REDIRECT (the invalid_request bug returned 400 here)
    rel = auth_url[len(origin):]
    status, headers, body = c.request("GET", rel)
    need(status == 302, f"authorise returned {status} (400 = the OAuth client is not registered for {redirect_uri}): {body[:120]!r}")
    location = headers.get("location", "")
    need(location.startswith(redirect_uri + "?code="), f"authorise redirected to {location!r}")
    ok("authorise redirects to the registered callback (no invalid_request)")

    # 7 (before consuming anything). the control plane stays closed
    status, _, _ = c.request("GET", "/fake-google/reset")
    need(status == 404, f"the fake-google control plane answered {status} through the front door; must be 404")
    ok("fake-google control plane is not reachable (404)")

    if args.no_redeem:
        print("PASS (no-redeem): invite not redeemed; the scripted login WAS consumed, re-seed before handing over")
        return

    # 6. the callback signs the invited user in
    status, headers, _ = c.request("GET", location[len(origin):])
    need(status == 302 and "outcome=joined" in headers.get("location", ""), f"callback returned {status} {headers.get('location')!r}")
    ok("callback redirects to #/auth/result?outcome=joined")
    status, _, body = c.request("GET", "/api/v1/session")
    session = json.loads(body)
    need(session.get("state") == "authenticated", f"session after sign-in is {session.get('state')!r}")
    need(len(session.get("mailboxes", [])) >= 1, "signed in but no mailbox linked")
    ok(f"signed in: session authenticated, {len(session['mailboxes'])} mailbox(es)")
    print("PASS")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True)
    ap.add_argument("--code", required=True)
    ap.add_argument("--invite-token", required=True)
    ap.add_argument("--origin")
    ap.add_argument("--no-redeem", action="store_true")
    args = ap.parse_args()
    try:
        run(args)
    except Fail as e:
        print(f"FAIL: {e}")
        return 1
    except (OSError, http.client.HTTPException, ValueError) as e:
        print(f"FAIL: could not complete the journey: {e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
