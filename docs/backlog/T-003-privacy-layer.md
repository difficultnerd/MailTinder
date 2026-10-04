# T-003: Install the privacy layer and extend its rules

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | sonnet | about 150 lines of YAML and config plus Semgrep rule tests | T-001 |

**Read only these spec sections:** S5 "Logs and telemetry" and "Classification levels" (`docs/specs/S5-data-inventory.md`), S6 8 (`docs/specs/S6-security.md`), S10 7.3 last paragraph, S10 6.4 and S10 10.1 (`docs/specs/S10-test-strategy.md`), S10 1 rule 2 (deterministic by construction). In the repo: `optional/privacy/` (all files), `optional/install.sh`, `tools/apply_branch_protection.sh`, `README.md` "Optional layers". Nothing else is needed.

## Goal

The template's `optional/privacy` layer moves into the core (decision T4): `privacy-checks` runs on every push and pull request, Clippy bans the print macros across the whole workspace, and the Semgrep privacy rules know the S5 field names. Two project rules join them: no permanent delete of mail (INV-5 structural check, S10 6.4) and no wall clock or thread RNG outside the real adapters (S10 rule 2). `privacy-checks` is added to the branch protection script; James applies it.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `.github/workflows/privacy.yml` | From `optional/privacy`, plus the rule tests and the project rules step |
| Create | `.semgrep/privacy.yml` | From `optional/privacy`, extended with S5 field names |
| Create | `.semgrep/mailtinder.yml` | Two project rules (below) |
| Create | `.semgrep/tests/privacy.rs`, `.semgrep/tests/privacy.dart`, `.semgrep/tests/mailtinder.rs` | Semgrep rule tests (annotated examples) |
| Create | `backend/clippy.toml` | From `optional/privacy`, plus `print!` and `eprint!` |
| Create | `PRIVACY.md` | Mail Tinder's real commitment and checklist |
| Change | `tools/apply_branch_protection.sh` | Add `privacy-checks` to `contexts` |
| Change | `README.md` | Core toolchain table gains the privacy row; the optional layers table drops `privacy` |
| Delete | `optional/privacy/` | The layer now lives in the core; one copy only `[DEFAULT]` (two copies drift) |

## Types and signatures

`backend/clippy.toml`:

```toml
# Forces all output through the structured logger (T-307), where redaction is applied in one place.
disallowed-macros = [
    { path = "std::println", reason = "use tracing; never print request data" },
    { path = "std::eprintln", reason = "use tracing; never print request data" },
    { path = "std::print", reason = "use tracing; never print request data" },
    { path = "std::eprint", reason = "use tracing; never print request data" },
    { path = "std::dbg", reason = "dbg! can leak sensitive values" },
]
```

`.semgrep/privacy.yml` keeps the three template rules and their IDs (`privacy-log-sensitive-identifier`, `privacy-rust-derive-debug-on-sensitive-struct`, `privacy-dart-to-string-print-of-model`) and extends the two identifier lists.

Log rule: replace the final group of `pattern-regex` with

```
\b\w*(email|password|passwd|secret|token|authorization|api_?key|cookie|bearer|(mail|message|msg)_?(body|content|text|subject)|subject|snippet|preview|from_(address|display|name)|sender_(address|name|display)|display_name|email_?address|list_unsubscribe|unsubscribe_(url|link|target)|message_?id|refresh|csrf|session_?(id|hash)|invite_?token|data_?key|url|href)\w*\b
```

Debug-derive rule: replace the field-name group with

```
\b\w*(email|password|passwd|secret|token|api_?key|(mail|message|msg)_?(body|content|text)|subject|snippet|preview|from_(address|display)|sender_(address|name|display)|display_name|email_?address|refresh|csrf|data_?key|url|link)\w*\s*:
```

`.semgrep/mailtinder.yml`:

```yaml
rules:
  - id: mailtinder-no-permanent-delete
    languages: [generic]
    severity: ERROR
    message: >-
      Permanent delete of mail is forbidden (S3 INV-5). Use trash. Gmail messages.delete,
      batchDelete and threads.delete, and Graph permanentDelete, must never be called.
    paths:
      include: ["*.rs", "*.dart"]
      exclude: ["backend/crates/testkit/**", "backend/crates/fake-google/**", ".semgrep/**"]
    pattern-regex: >-
      (?i)(batchDelete|permanentDelete|threads\.delete|messages\.delete|(\.delete\(|Method::DELETE)[^;]{0,300}/(messages|threads)/)

  - id: mailtinder-no-wall-clock-or-thread-rng
    languages: [generic]
    severity: ERROR
    message: >-
      Time comes only from the Clock port and randomness only from the Rng port (S10 1 rule 2,
      CONVENTIONS). Only the real adapters may call these.
    paths:
      include: ["*.rs"]
      exclude: ["**/system_clock.rs", "**/os_rng.rs", "backend/crates/adapters-*/**", "backend/crates/egress/**", ".semgrep/**"]
    pattern-regex: >-
      (SystemTime::now|OffsetDateTime::now_utc|OffsetDateTime::now_local|Utc::now|thread_rng|rand::random|OsRng)
```

## Algorithm

1. Copy `optional/privacy/.github/workflows/privacy.yml`, `optional/privacy/.semgrep/privacy.yml` and `optional/privacy/backend/clippy.toml` to the same paths at the repo root (`optional/install.sh privacy` does this; check the result by hand), then delete `optional/privacy/`. `optional/install.sh` keeps working for `zap` and `gpg-signing`.
2. Apply the `clippy.toml` and `privacy.yml` changes above. Keep each rule's `message` text and add, to the log rule's message, the sentence "S5 bans tokens, cookies, message IDs, addresses, names, subjects, snippets, bodies and URLs from logs."
3. Write `.semgrep/mailtinder.yml` as above.
4. Write the rule tests. Each file holds examples annotated with Semgrep's test comments: a line `// ruleid: <rule-id>` directly above every line that must match, and `// ok: <rule-id>` above every line that must not. Minimum cases:
   - `privacy.rs`: `tracing::info!(subject = %m.subject)` (ruleid), `info!(from_address = ...)` (ruleid), `warn!("bad url {}", u)` (ruleid), `info!(message_id = ...)` (ruleid), `info!(route = "feed.next", status = 200)` (ok), `info!(latency_ms = 12)` (ok); a struct with `#[derive(Debug)]` and a `subject: String` field (ruleid on the derive line), the same struct without `Debug` (ok).
   - `privacy.dart`: `print(user)` (ruleid for the Dart rule), `debugPrint('loaded')` (ok).
   - `mailtinder.rs`: `client.delete(format!("{BASE}/users/me/messages/{id}"))` (ruleid), `"batchDelete"` (ruleid), `client.delete(format!("{BASE}/users/me/labels/{id}"))` (ok: label delete is allowed, API-CAT-4), `let now = SystemTime::now();` (ruleid), `let now = clock.now();` (ok).
5. Update `.github/workflows/privacy.yml` so the `privacy-checks` job runs, in order: checkout; `pip install semgrep`; `semgrep --test --config .semgrep/ .semgrep/tests/` (rule tests); `semgrep scan --error --metrics=off --config .semgrep/privacy.yml --config .semgrep/mailtinder.yml backend app`; the Clippy step from the template, changed to `cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features --locked -- -D warnings`. Keep the template's pinned action SHAs exactly.
6. Write `PRIVACY.md` (replace the template's placeholder paragraph). Commitment, in plain words, from S5: no mail content (bodies, subjects, snippets, headers) at rest on Mail Tinder's servers; the server keeps only job, Needs Attention, session, invite and security log records, each with a TTL or deletion path; user state lives in the user's own Drive app folder, encrypted; logs carry only the S5 C1 fields. Keep the template's "Checklist for every change" but change "nothing is written to disk, cache or database" to "no mail content is written to disk, cache or database; every stored field is listed in S5". Update "What CI enforces" to name both Semgrep files, the rule tests and `backend/clippy.toml`, and add a line that T-307's log-scanning test (LOG-1) is the runtime check.
7. In `tools/apply_branch_protection.sh`, change `contexts` to `'["rust","dart","language-policy","gitleaks","semgrep","cargo-audit","cargo-deny","dart-licenses","privacy-checks"]'` and update the comment above it to say the names match the job IDs in `ci.yml`, `security.yml` and `privacy.yml`.
8. Update `README.md`: add a "Privacy" row to the core toolchain table (`.semgrep/privacy.yml`, `.semgrep/mailtinder.yml`, `backend/clippy.toml`; CI `privacy-checks`); change "Branch protection requires all eight checks" to nine and add `privacy-checks` to the list; remove the `optional/privacy` row from the optional layers table.
9. Run locally: `cd backend && cargo clippy --all-targets --all-features --locked -- -D warnings` (must pass on the T-001 skeleton), and `semgrep --test --config .semgrep/ .semgrep/tests/` if Semgrep is installed.
10. **Step for James (not the builder):** after this pull request merges and `privacy-checks` has run once on `main`, run `tools/apply_branch_protection.sh` with admin rights so `privacy-checks` becomes required. The builder says so in the pull request description.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | This task adds CI controls, proven by the Semgrep rule tests. XC-01 is proven by T-307's log-scanning test; INV-5 by T-203 and T-205 tests |

## Tests that must pass

- `semgrep --test --config .semgrep/ .semgrep/tests/` passes: every `ruleid` line matches and no `ok` line matches (rule test, CI `privacy-checks`).
- `privacy-checks` scan of `backend app` finds nothing on `main` (CI).
- `cargo clippy ... -D warnings` with the new `clippy.toml` passes on the workspace (CI `rust` and `privacy-checks`).

## Edge cases and traps

- Clippy reads `backend/clippy.toml` for every workspace crate because it searches parent folders; do not copy it into each crate.
- The Semgrep regexes are case-insensitive (`(?is)`) and also scan string literals, so `#[error("bad url {0}")]` in a `thiserror` enum matches the log rule (`error(` plus `url`). Write error messages without sensitive words (`#[error("invalid unsubscribe target")]`), do not weaken the rule.
- Do not add `target` or `link` to the log rule: `tracing`'s `info!(target: "security", ...)` and security event names such as `"mailbox_link"` would match everywhere.
- The `mailtinder-no-permanent-delete` regex must not match label deletion (`/labels/{id}`), which API-CAT-4 needs. Keep the `ok` case for it.
- The rule tests live under `.semgrep/tests/` and contain deliberate violations. They must stay out of every scan: the privacy scan only covers `backend app`, and the existing `semgrep` job in `security.yml` runs only registry packs, which do not include these rules. Do not add `.semgrep/` to that job.
- If `semgrep --test` cannot pair rules with tests in the `tests/` folder (the pairing is by file stem: `privacy.yml` with `tests/privacy.*`), move the test files beside the rule files and run `semgrep --test .semgrep/` instead; say which layout you used in the pull request.
- Gitleaks scans every file. Test examples use obviously fake values (`"example-token"`), never anything that looks like a real key.
- Outside the adapter crates (`adapters-*`, `egress`), only files named `system_clock.rs` and `os_rng.rs` may read the wall clock or the OS RNG. Whichever task builds the real `Clock` and `Rng` must use those file names.
- `std::time::Instant::now` is deliberately allowed: request latency for logs (S5 C1 field) is measured with a monotonic timer, not the domain clock. Do not use `Instant` for any business decision.
- Do not add `uri` to the log rule: `\w*uri\w*` matches `security`, and security events are logged everywhere.
- Adding `privacy-checks` to the protection script before the job has ever run on `main` would block every pull request. The script change merges here; James runs it afterwards.
- `--locked` on the privacy Clippy step needs the committed `Cargo.lock` from T-001.

## Out of scope

- The redacting `Sensitive<T>` type and the log-scanning test: T-307.
- Semgrep rules for the ASVS `semgrep` register rows (V1.2.3, V11.3.1 and others): not owned by this task (see the T-002 rule ID convention `asvs-<row>-<slug>`).
- `optional/zap`: installed when staging exists (T-1105).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The pull request description tells James to run `tools/apply_branch_protection.sh` after merge.
