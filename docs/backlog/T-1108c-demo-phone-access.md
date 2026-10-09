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
| Change | `scripts/demo.sh` | `--phone`: start `cloudflared tunnel --url http://127.0.0.1:<port>` (binary on PATH or `~/bin/cloudflared`), print the `https://...trycloudflare.com` URL and a generated 16-character code once; stop also stops the tunnel |
| Change | `scripts/e2e_host.py` | `--access-code`: HTTP Basic auth (user `demo`) on EVERY request when set |

## Behaviour

1. Only the front door is exposed; api, unsub, fakes and emulator stay on loopback.
2. The code is never logged or committed; `--phone` refuses to start if no code could be generated.
3. The served page contains the standard viewport meta tag.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `demo_phone_requires_access_code` (401 without, 200 with)
- `demo_phone_exposes_only_the_front_door`
- `demo_page_has_viewport_meta`

## Out of scope

Seeding, hot reload.

## Done when

PR body shows a request without the code refused and with it served (use the local front door, no real tunnel needed in CI). Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
