# T-1108: Demo mode: run the whole app locally and click through it

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 350 lines of code plus tests | T-500b, T-500c, T-1112, T-1101a |

**Read only these spec sections:** `docs/backlog/T-1101a-e2e-harness-and-required-check.md` (the harness this task reuses), `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` (e2e mode), S10 section 3.3. Nothing else is needed.

## Goal

The app is a PHONE experience, so the demo must be usable from a phone browser as well as a laptop (owner requirement, 2026-10-09). One command, `scripts/demo.sh`, starts the same local stack as `scripts/e2e.sh` (Firestore emulator, fake-google, unsub-testbed, `api` in e2e mode, the Flutter web build served by `scripts/e2e_host.py`), seeds a fake mailbox from the synthetic corpus, and leaves it running so a human can open the app in a browser and try it. It prints one URL and stops cleanly. This lets the owner use the product before any cloud environment exists.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/demo.sh` | `start` (default), `stop`, `status`, `--check` |
| Change | `scripts/e2e.sh` | Move the shared start-up steps into functions in `scripts/e2e/lib.sh` so both scripts use them (behaviour of `e2e.sh` must not change) |
| Create | `scripts/e2e/lib.sh` | Shared functions: start emulator, start fakes, start services, wait for ports, stop everything |
| Create | `scripts/demo_seed.py` | Stdlib only: seeds `invitee@example.com` with the corpus messages through the fake-google control API and creates an invite through the e2e invite route; prints the invite URL |
| Create | `docs/demo.md` | How to run it, how to reach it from a laptop with an SSH tunnel, how to use Chrome's device toolbar at phone size |

## Behaviour

1. The api binary binds `0.0.0.0` unconditionally today; T-500c makes e2e mode bind `127.0.0.1` only, and this task depends on it (found by the Sol review of PR #33). `demo.sh` binds **127.0.0.1 only** and refuses to start if any configured host is not loopback. It never listens on `0.0.0.0`.
2. It picks free ports with `scripts/free_port.py`, writes `target/demo/state.json` (ports, pids, URL), and prints `Open: http://localhost:<port>/#/invite?t=<token>`. The web app is served at one port so a single SSH tunnel is enough.
3. `stop` kills every process it started (process group), even after a crash; `status` reports which pieces are up.
4. `--check` is the automated test: start, `GET` the invite URL's page and `/api/v1/healthz` through the proxy, assert 200 and the `firebase.json` security headers, then stop and assert no child process remains. It must finish in under 3 minutes on verify1.
5. No real credentials: the same fake values as `scripts/e2e/*.env`.

## Phone access (added 2026-10-09)

6. `demo.sh --phone` additionally starts a **Cloudflare quick tunnel** (`cloudflared tunnel --url http://127.0.0.1:<front-door-port>`; the binary is expected on the PATH or at `~/bin/cloudflared`, otherwise the script prints how to install it) and prints an `https://...trycloudflare.com` URL for the phone. Nothing but the front door is exposed: the api, unsub, fakes and emulator stay on loopback.
7. Because the tunnel is reachable by anyone who has the URL, `e2e_host.py --access-code <random>` enforces a login on EVERY request when `--phone` is used (HTTP Basic auth, user `demo`, a freshly generated 16-character code printed once by `demo.sh`); the demo data is synthetic and the testkit routes exist only in this e2e stack, but the front door must still refuse unauthenticated requests. The generated code is never written to a log or committed.
8. The page must render well in a phone browser: the app already targets phones; the demo adds the standard viewport settings and verifies `GET /` returns a document that contains the viewport meta tag.
9. `demo.sh stop` also stops the tunnel; `--phone` refuses to start if the access code could not be generated.

## Acceptance criteria

- `demo_check_serves_app_and_api_then_cleans_up` (shell test run by `scripts/demo.sh --check`, wired into `tools/ci-local.sh --full` as `demo-smoke`).
- `demo_phone_requires_access_code` (a request without the code gets 401, with it 200) and `demo_phone_exposes_only_the_front_door` (the api port answers only on loopback).
- `demo_refuses_non_loopback_bind` (shell test: set a non-loopback host in the environment, expect a non-zero exit before anything starts).
- `scripts/e2e.sh` still passes unchanged (its journey runs).

## Out of scope

Public exposure of the demo, real Google, any cloud setup, new journeys.

## Done when

The checks above pass; paste the output of `scripts/demo.sh --check` and the printed URL line (token redacted) in the PR body; definition of done in S10 10.4. Do not edit `.github/workflows/`.
