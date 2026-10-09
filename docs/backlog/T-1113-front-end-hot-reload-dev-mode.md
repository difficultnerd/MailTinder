# T-1113: front-end hot-reload dev mode, so look and feel can be changed in seconds

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 150 lines of code plus tests | T-1108 |

**Read only these spec sections:** `docs/backlog/T-1108-demo-mode.md`, `scripts/e2e_host.py`, `scripts/demo.sh`. Nothing else is needed.

## Goal

`scripts/demo.sh --dev` runs the Flutter app with hot reload (`flutter run -d web-server`) behind the same demo front door as the normal demo, so an edit to a Dart file shows on the owner's phone within seconds, with the real fake-backed api behind it.

## Behaviour

1. `--dev` starts the same backend stack as `demo.sh` but serves the app from `flutter run -d web-server --web-hostname 127.0.0.1 --web-port <free port>` (hot reload and hot restart enabled); `e2e_host.py` gains a `--dev-upstream http://127.0.0.1:<port>` option that proxies `/` and the dev server's websocket to it and keeps proxying `/api/**` to the api, adding the same security headers and the same access control as normal demo mode.
2. Edits are made in the demo worktree; a small helper `scripts/demo_reload.sh` triggers a hot reload by sending `r` to the dev server's stdin (or the documented Flutter mechanism), so a coding agent can change a file and refresh the view without restarting anything.
3. Phone access and the access code work exactly as in T-1108 (`--phone`); a warning in the output states that the dev server is slower than the release build and is for look-and-feel iteration only.

## Acceptance criteria

- `demo_dev_serves_app_and_api_through_one_origin` (shell test inside `demo.sh --check --dev`).
- `demo_reload_changes_the_served_app` (edit a visible string, call the helper, assert the new string is served within 30 s, revert).
- `demo_dev_applies_access_control_like_normal_mode`.

## Out of scope

Production builds, release signing, native mobile builds, device farms.

## Done when

Tests pass, belt green, PR body shows a before/after of a one-line UI change reloaded in the running demo. Definition of done in S10 10.4. Do not edit `.github/workflows/`.
