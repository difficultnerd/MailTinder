# T-1108c: demo --phone: HTTPS tunnel to the front door with an access code

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 120 lines of code plus tests | T-1108a |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

`demo.sh --phone` makes the demo reachable from a phone through a Cloudflare quick tunnel to the front door only, protected by a generated access code.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `scripts/demo.sh` | `--phone`: start `cloudflared tunnel --url http://127.0.0.1:<port>` (binary on PATH or `~/bin/cloudflared`), print the `https://...trycloudflare.com` URL and the access code once; stop also stops the tunnel. The binary's `--version` and SHA-256 are checked against `scripts/demo/cloudflared.sha256` before it is run, and the `--url` argument must be a loopback address (refuse otherwise) |
| Change | `scripts/e2e_host.py` | `--access-code-file <path>`: HTTP Basic auth (user `demo`) on EVERY request when set; `/fake-google/**` proxy to loopback fake-google |
| Change | `backend/crates/api/src/startup_e2e.rs` | `MT_PUBLIC_BASE_URL`: browser-facing authorisation URL (see Behaviour 0) |
| Create | `scripts/demo/cloudflared.sha256` | Pinned `cloudflared` version (2026.10.0) and the SHA-256 of the linux-arm64 and linux-amd64 artefacts |

## Behaviour

0. **The sign-in round trip must work from a phone (review finding F1).** The browser is redirected to the fake Google's authorise page, which lives on the laptop's loopback and is unreachable from a phone. So in `--phone` mode: (a) `demo.sh` starts the tunnel FIRST and reads the public `https://...trycloudflare.com` URL; (b) it then starts the api with `APP_ORIGIN=<public url>` (the OAuth redirect URI and the cookies' `Secure` attribute need the public https origin) and a new variable `MT_PUBLIC_BASE_URL=<public url>`; (c) in e2e mode the api builds the BROWSER-facing authorisation URL as `${MT_PUBLIC_BASE_URL}/fake-google/o/oauth2/v2/auth` (when the variable is set; otherwise exactly as today), while server-to-server calls (token, jwks, revoke, Gmail, Drive) keep using the loopback `FAKE_GOOGLE_URL`; (d) `e2e_host.py` proxies `/fake-google/**` to fake-google on loopback (path prefix stripped) and applies the access control to it like every other path. The change in (c) belongs to this task and touches `backend/crates/api/src/startup_e2e.rs` only.

1. Only the front door is exposed; api, unsub, fakes and emulator stay on loopback.
2. **Access code (review finding F2):** 16 characters from the alphabet `A-Za-z0-9` drawn from the OS random source (at least 95 bits); compared in constant time (`hmac.compare_digest`); more than 5 failed attempts from one address within 60 s are answered `429` for 5 minutes; the code is handed to `e2e_host.py` through a file with mode 600 (`--access-code-file`), never on the command line, so it does not appear in the process list; it is printed once to the terminal and never logged or committed; `--phone` refuses to start if it could not be generated.
3. The served page contains the standard viewport meta tag.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `demo_phone_requires_access_code` (401 without, 200 with)
- `demo_phone_exposes_only_the_front_door`
- `demo_page_has_viewport_meta`
- `demo_phone_signin_goes_through_the_front_door` (with a stub `cloudflared` that prints a fixed public URL: the api's authorisation URL starts with `MT_PUBLIC_BASE_URL/fake-google/`, and a request to that path through the front door reaches fake-google and is refused without the code)
- `demo_access_code_is_rate_limited_and_not_in_argv` (6 wrong tries get 429; the code does not appear in the process list)
- `demo_refuses_unpinned_or_wrong_cloudflared` and `demo_refuses_non_loopback_tunnel_target`

## Out of scope

Seeding, hot reload.

## Done when

The tunnel is tested with a STUB `cloudflared` that records its argv and prints a fixed URL (no real tunnel in CI): the test asserts the single loopback front-door target. The PR body shows a request without the code refused and with it served, and the sign-in round trip through the front door. Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
