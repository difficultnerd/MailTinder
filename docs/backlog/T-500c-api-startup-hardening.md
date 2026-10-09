# T-500c: api start-up hardening: follow-ups from the T-500b reviews

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | sonnet | about 200 lines of code plus tests | T-500b |

**Read only these spec sections:** `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` (what was built), `backend/crates/api/src/{config.rs,startup.rs,startup_e2e.rs}` and `backend/crates/api/tests/startup.rs`. The two independent reviews of PR #33 (Claude on 5cad4859, Sol on 00e17b7e) are the source of every item below.

## Goal

Close the Low and Info findings that T-500b merged with, so the api binary is strict in production and safe in e2e mode. No new features.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/config.rs` | `APP_ORIGIN` validation depends on the mode: production accepts `https` only; loopback `http` is accepted only in e2e mode. An empty value of any required variable is `Missing(name)`, consistently |
| Change | `backend/crates/api/src/startup.rs` | Listener host from `API_BIND_HOST`: default `0.0.0.0` in production; in e2e mode it is forced to `127.0.0.1` and any other value is `Invalid("API_BIND_HOST")`. Validate `GOOGLE_CLOUD_PROJECT` against `^[a-z][a-z0-9-]{4,28}[a-z0-9]$` before it is used in KMS, Tasks or queue resource names |
| Change | `backend/crates/api/src/startup_e2e.rs` | `LoopbackEgress::new` requires `ip.is_loopback()` (IPv4 `127.0.0.0/8` and IPv6 `::1`, including the bracketed `[::1]` URL form); response bodies are read with a 1 MiB cap |
| Change | `backend/crates/api/tests/startup.rs` | Tests below; fix the log-canary test so the canary value is actually read |
| Delete | the two `backend/crates/api/default_*.profraw` files | Build debris committed by mistake |
| Change | `.gitignore` | Add `*.profraw` |
| Change | `docs/backlog/T-500b-api-binary-wiring-and-e2e-config.md` | Make the contradiction explicit: empty required variable is `Missing`; an origin that is present but malformed is `Invalid` |

## Acceptance criteria (each is a named test that fails if the behaviour is removed)

- `api_production_rejects_http_app_origin` (also for `http://localhost`, `http://127.0.0.1`, `http://[::1]`) and `api_e2e_accepts_loopback_http_app_origin_only`.
- `api_e2e_binds_loopback_only`: in e2e mode the bound address is `127.0.0.1`; `API_BIND_HOST=0.0.0.0` in e2e mode is refused.
- `api_production_bind_host_defaults_to_all_interfaces_and_is_configurable`.
- `api_log_canary_value_is_read_then_never_logged`: set a valid `APP_ORIGIN`, set `GOOGLE_OAUTH_CLIENT_ID=canary-…`, leave a LATER variable missing (`UNSUB_BASE_URL`), also put a second canary in an invalid value; assert neither canary appears in stdout or stderr and the exit code is non-zero.
- `api_rejects_invalid_project_id` (uppercase, too short, too long, contains `/` or `..`).
- `api_loopback_egress_rejects_non_loopback_ip_and_caps_body` (a non-loopback literal IP is refused at construction; a response over 1 MiB is refused).
- `api_empty_required_variable_is_missing` (each of the required variables).
- `api_no_profraw_tracked`: a test (or a `tools/` check run by the belt) that fails if any `*.profraw` file is tracked by git.

## Notes

- The "binary refuses `MT_E2E=1` without `testkit`" test only runs in a build WITHOUT the `testkit` feature, while CI and `tools/ci-local.sh` use `--all-features`. Do not change workflows here; T-1101a's PR adds a `cargo test --locked -p api --test startup` step without features to the `e2e` job. This task only makes sure that command passes with default features.
- Do not edit `.github/workflows/`, `CLAUDE.md` or `tools/apply_branch_protection.sh`.

## Done when

The tests above pass, the belt is green, and the PR body lists each finding (F3, F4, F5, F6, F7, F9, N3) with the test that proves it fixed; definition of done in S10 10.4.
