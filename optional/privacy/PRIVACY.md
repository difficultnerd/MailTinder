# Privacy commitments

Replace this text with the project's actual no-data-retention commitment, then keep the
checklist true.

## Checklist for every change

- No email content, tokens, passwords or message bodies reach logs, error reports or analytics.
- Sensitive types use a redacting wrapper; none derive `Debug` with raw fields.
- Data is held in memory only for the life of the request; nothing is written to disk, cache or database.
- Third-party SDKs are reviewed for telemetry before they are added.
- Any new exception to the commitment is recorded here with a date and reason.

## What CI enforces

- `.semgrep/privacy.yml`: flags logging of sensitive identifiers and `Debug` derives on sensitive structs.
- `backend/clippy.toml`: bans `println!`, `eprintln!` and `dbg!`.
- Dart `avoid_print` lint (already on in `app/analysis_options.yaml`).

These checks catch accidents by name and cannot prove that retention never happens. Review
storage code paths by hand.
