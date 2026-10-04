# Privacy commitments

Mail Tinder is built on a simple principle: no mail content (bodies, subjects, snippets, headers) at rest on Mail Tinder's servers. The server keeps only job, Needs Attention, session, invite and security log records, each with a TTL or deletion path; user state lives in the user's own Drive app folder, encrypted; logs carry only the S5 C1 fields.

## Checklist for every change

- No email content, tokens, passwords or message bodies reach logs, error reports or analytics.
- Sensitive types use a redacting wrapper; none derive `Debug` with raw fields.
- No mail content is written to disk, cache or database; every stored field is listed in S5.
- Third-party SDKs are reviewed for telemetry before they are added.
- Any new exception to the commitment is recorded here with a date and reason.

## What CI enforces

- `.semgrep/privacy.yml`: flags logging of sensitive identifiers and `Debug` derives on sensitive structs.
- `.semgrep/mailtinder.yml`: flags permanent deletion of mail and direct calls to wall clock or thread RNG outside adapters.
- Semgrep rule tests in `.semgrep/tests/`: verify privacy and project rule enforcement.
- `backend/clippy.toml`: bans `println!`, `eprintln!`, `print!`, `eprint!` and `dbg!`.
- Dart `avoid_print` lint (already on in `app/analysis_options.yaml`).
- T-307's log-scanning test (LOG-1) is the runtime check.

These checks catch accidents by name and cannot prove that retention never happens. Review
storage code paths by hand.
