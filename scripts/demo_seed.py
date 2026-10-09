#!/usr/bin/env python3
"""Seed the demo stack (T-1108b): a fake mailbox, the corpus and an invite URL.

Stdlib only. Resets fake-google first, so seeding twice can never duplicate
mail, gives `invitee@example.com` the whole synthetic corpus, scripts the next
Google login to approve that account (so opening the printed URL signs in and
shows the Feed) and creates an invite through the api's e2e invite route.

    python3 scripts/demo_seed.py \
        --fake-google-url http://127.0.0.1:8086 \
        --api-url http://127.0.0.1:8087 \
        --app-origin http://127.0.0.1:8080
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_EMAIL = "invitee@example.com"
DEFAULT_SUB = "sub-invitee"
# The fake OAuth client the e2e stack uses (scripts/e2e/api.env); a value, not a
# secret.
CLIENT_ID = "fake-e2e-client"
CLIENT_SECRET = "test-only-not-a-secret"


def is_loopback(host: str) -> bool:
    """True for a loopback host: `localhost`, `::1` or a 127.0.0.0/8 quad."""
    if host in ("localhost", "::1"):
        return True
    parts = host.split(".")
    if len(parts) != 4 or parts[0] != "127":
        return False
    return all(part.isdigit() and int(part) <= 255 for part in parts)


def validate_url(url: str) -> str:
    """Return `url` when it is an http URL on a loopback host.

    Every request this script makes targets a local fake stack, so anything
    that is not loopback http (a `file:` URL, a remote host, a DNS name that
    merely starts with `127.`) is refused before `urlopen` sees it.
    """
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "http":
        raise ValueError(f"refusing non-http URL: {url!r}")
    host = parts.hostname
    if host is None or not is_loopback(host):
        raise ValueError(f"refusing non-loopback URL: {url!r}")
    return url


def post_json(url: str, payload: dict) -> dict:
    """POST a JSON body and return the decoded JSON response."""
    validate_url(url)
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    try:
        # The URL was checked by validate_url (loopback http only) before the
        # request was built, so a dynamic value here cannot reach file:// or a
        # remote host.
        # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected, python.lang.security.audit.dynamic-urllib-use-detected
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode()
    except urllib.error.HTTPError as error:
        raise SystemExit(
            f"demo_seed: POST {url} failed: {error.code} "
            f"{error.read().decode(errors='replace')}"
        ) from error
    return json.loads(body) if body else {}


def seed(fake_google_url: str, api_url: str, app_origin: str, email: str, sub: str) -> dict:
    """Seed the mailbox and invite; return the invite URL and message count."""
    fake = fake_google_url.rstrip("/")
    api = api_url.rstrip("/")
    origin = app_origin.rstrip("/")

    # A fresh fake state makes the seed idempotent: running it twice cannot
    # duplicate the corpus (T-1108b behaviour 1).
    post_json(f"{fake}/__fake/reset", {})
    post_json(f"{fake}/__fake/gmail/mailboxes", {"email": email})
    seeded = post_json(f"{fake}/__fake/gmail/seed-corpus", {"email": email})
    ids = seeded.get("ids", [])

    # The sign-in round trip: register the app's fake OAuth client and script
    # the next authorisation to approve the seeded account.
    post_json(
        f"{fake}/__fake/identity/clients",
        {
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "redirect_uris": [f"{origin}/api/v1/auth/google/callback"],
        },
    )
    post_json(
        f"{fake}/__fake/identity/next-login",
        {"sub": sub, "email": email, "email_verified": True, "outcome": "approve"},
    )

    invite = post_json(f"{api}/internal/test/invites", {"email": email})
    token = invite.get("token")
    if not token:
        raise SystemExit("demo_seed: the invite route returned no token")
    return {
        "email": email,
        "message_count": len(ids),
        "invite_url": f"{origin}/#/invite?t={token}",
    }


def main(argv: list[str] | None = None) -> int:
    """CLI entry point."""
    parser = argparse.ArgumentParser(description="Seed the demo stack (T-1108b).")
    parser.add_argument("--fake-google-url", required=True)
    parser.add_argument("--api-url", required=True)
    parser.add_argument("--app-origin", required=True)
    parser.add_argument("--email", default=DEFAULT_EMAIL)
    parser.add_argument("--sub", default=DEFAULT_SUB)
    parser.add_argument("--json", action="store_true", help="print the summary as one JSON line")
    args = parser.parse_args(argv)

    result = seed(args.fake_google_url, args.api_url, args.app_origin, args.email, args.sub)
    if args.json:
        print(json.dumps(result))
    else:
        print(f"demo: seeded {result['message_count']} messages for {result['email']}")
        print(f"demo: invite URL: {result['invite_url']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
