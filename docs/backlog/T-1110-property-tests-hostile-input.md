# T-1110: Property and fuzz-style tests for every parser of hostile input

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 400 lines of tests | T-102, T-306, T-406, T-701 |

**Read only these spec sections:** S6 (input handling, SSRF), the hostile-input fixtures in S10; the existing `backend/crates/domain/tests/text_hostile.rs` and `backend/crates/domain/tests/redact.rs` (the style to copy). Nothing else is needed.

## Goal

Everything that parses text from the internet gets property tests with many generated inputs, so malformed mail cannot crash the service or slip past a safety rule. `proptest` is already a workspace dependency and is used for text, redaction, undo and the egress range table; this task covers the parsers that have none.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/tests/prop_list_unsubscribe.rs` | `list_unsubscribe.rs` parser properties |
| Create | `backend/crates/adapters-gmail/tests/prop_headers.rs` | `headers.rs` and `auth_results.rs` properties |
| Create | `backend/crates/domain/tests/prop_mailto_address.rs` | `mailto.rs` and `address.rs` properties |
| Create | `backend/crates/domain/tests/prop_header_rules.rs` | `header_rules.rs` properties |
| Create | `backend/crates/egress/tests/prop_target.rs` | `target.rs` / allowlist URL properties |
| Change | `backend/crates/adapters-gmail/Cargo.toml`, `backend/crates/egress/Cargo.toml` | `proptest.workspace = true` under `[dev-dependencies]` where missing |
| Create | `docs/property-testing.md` | How to run with more cases, how to keep `proptest-regressions/` files |

## Properties (each is a named test)

1. **Never panics:** every public parsing function returns `Ok` or `Err` for arbitrary byte and Unicode strings (including NUL, control characters, bidi marks, very long input up to 1 MiB, unpaired surrogates via lossy conversion).
2. **List-Unsubscribe:** whatever the header contains, the parser's accepted targets are only `https` URLs or `mailto:` addresses that pass the project's own validators; `http`, `javascript:`, `file:`, `data:`, userinfo (`user@host`), IP literals in private ranges and hosts with a trailing dot or mixed-script lookalikes are never accepted.
3. **One-click:** the one-click flag is true only when the header pair is exactly the RFC 8058 form.
4. **Headers and auth:** folded headers, duplicated headers and CR/LF injection never produce a header value containing a raw CR or LF. `assess_auth` follows the T-406 contract exactly and nothing stricter: `from_authenticated` is true only for a DKIM-aligned From or a Gmail DMARC pass, and an Authentication-Results header from any authserv-id other than `mx.google.com` changes neither `from_authenticated` nor the unsubscribe options; unsubscribe options are offered only when the trusted Authentication-Results reports a `dkim=pass` for a signature that is identified among the `DKIM-Signature` headers and covers `List-Unsubscribe`; `assess_auth` never panics. (The earlier wording "never pass without a From-tied dkim=pass" contradicted T-406, which deliberately allows a trusted DMARC pass; found by the Sol review of PR #33.)
5. **mailto/address:** round-trip for valid addresses (`parse(display(x)) == x`); never accepts an address with a CR, LF or NUL; header-injection payloads in subject or to fields are refused or neutralised.
6. **Header rules:** classification is deterministic (same input, same class) and monotone in the documented ways (adding an unsubscribe header never lowers the bulk score).
7. **Egress targets:** a URL whose host resolves in the table `egress/src/ranges.rs` as private, loopback, link-local or metadata is refused for every encoding the tests generate (decimal, hex, octal and short forms, IPv4-mapped IPv6, trailing dot, uppercase, percent-encoded dots, `@` tricks, `#` and `\` tricks).

## Behaviour

- Default `PROPTEST_CASES` stays at the crate default so the belt stays fast; add a documented way to run 20000 cases (`PROPTEST_CASES=20000 cargo test -p ... prop_`) for a nightly run.
- Commit `proptest-regressions/` files if any appear.
- If a property finds a real bug, fix the bug in the same PR and name it in the PR body (that is the point of this task). Do not weaken a property to make it pass.

## Out of scope

libFuzzer harnesses (a later stage on a nightly toolchain), new parsing features.

## Done when

The new tests pass, the belt is green, the PR body states the number of cases run per property and lists any bug found and fixed; definition of done in S10 10.4. Do not edit `.github/workflows/`.
