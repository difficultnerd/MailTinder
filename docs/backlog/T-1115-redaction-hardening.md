# T-1115: Model input redaction hardening: quoted and bracketed addresses, sender-domain validation

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | small: about 80 lines of code plus tests | T-903 |

**Read only these spec sections:** `docs/backlog/T-903-model-input-redaction.md` (Goal, Algorithm, Acceptance criteria EXP-2 and V14.2.3); S4 5.5 and CR-01 1.5 as T-903 cites them. Nothing else is needed.

**Why this task exists.** The independent review of PR #20 (2026-10-10) found two Medium defects in code that already merged with T-903 (findings F11 and F12). Both were checked against `main` by the on-call engineer (lines below are from `backend/crates/domain/src/redact.rs` at the time of writing): they are privacy gaps in what leaves for the model.

## Goal

Nothing that looks like an email address survives `redact`, including quoted local parts, address literals and dotless domains, and `from_domain` can only ever return a syntactically valid hostname (or the empty string) before it is rendered into the classifier input.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/domain/src/redact.rs` | email patterns (`Patterns::compile`, field `email`, line 46) and `from_domain` (line 121) |
| Change | `backend/crates/domain/tests/redact.rs` | new table rows and properties below |
| Change | `backend/crates/api/tests/exp_2_input_redaction.rs` | one corpus-independent case: assert the LITERAL line `from_domain: ` (empty) in the rendered input for a hostile `From`, not a string built with `from_domain(...)` itself (the existing line 221 pattern is tautological for this purpose) |

## Types and signatures

Unchanged public signatures: `pub fn redact(input: &str) -> String`, `pub fn from_domain(address: &str) -> String`.

## Algorithm

1. **Email redaction (F11).** Today `email` is `[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)+`, which misses (a) a quoted local part such as `"a b"@x.example`, (b) a domain literal such as `a@[192.0.2.1]`, and (c) a dotless domain such as `a@localhost`. Whitespace is normalised before matching (line 77), so a quoted local part with a space is still intact at match time. Add, applied before the existing pattern and keeping the existing order of URL patterns: a quoted-local-part pattern `"[^"]*"@\S+` (no upper bound; the regex crate is linear-time), a literal pattern `\S*@\[[^\]]+\]`, and a final catch-all that redacts any `@` with non-whitespace text on both sides whatever characters surround it: `\S*@\S+` over the whitespace-normalised string (so `a@"b`, `"@x`, `a@<b>` and `a@@b` are all redacted). Replace each match with `EMAIL_PLACEHOLDER`. The placeholder contains no `@`, so `redact` stays idempotent (`exp_2_redact_idempotent` must still pass).
2. **Sender domain (F12).** Today `from_domain` returns everything after the last `@`, lower-cased. Make it return the lower-cased domain only if the whole input has no whitespace or control character AND the part after the last `@` is a valid hostname: 1 to 253 characters, dot-separated labels of 1 to 63 ASCII letters, digits or hyphens, none starting or ending with a hyphen, at least one dot (punycode labels `xn--` are letters, digits and hyphens and pass). Otherwise return the empty string. This keeps `local@EXAMPLE.COM` giving `example.com` and `no-address` giving the empty string, and turns `a@evil.example.com @other.com` and bracketed or whitespace-laden inputs into the empty string.
3. Do not log any address or domain on rejection (the redaction rules forbid it).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| EXP-2 | Quoted-local-part (any length), address-literal, dotless-domain and oddly delimited addresses are replaced by `[email]` |
| V14.2.3 | Sensitive data is not sent to third parties beyond the minimised allowlist (touched row: re-verified by the tests below) |
| EXP-2 | `from_domain` returns only a valid hostname or the empty string (second behaviour of the same row) |

## Tests that must pass

- `exp_2_redact_table` extended (unit): `"a b"@x.example`, `a@[192.0.2.1]`, `a@localhost`, `Mail "x y"@z.test now`, a quoted local part of 65 characters, `a@"b`, `"@x`, `a@<b>` and `a@@b` all lose the address.
- `exp_2_redact_never_leaves_at_sign_address` still passes (property) and a new property `exp_2_redact_no_at_sign_left_in_adjacent_text`: any generated string with `@` flanked by non-whitespace characters never keeps an `@`-adjacent non-space run (the generator must be unconstrained apart from whitespace).
- `exp_2_from_domain_only` extended (unit): `local@EXAMPLE.COM` gives `example.com`; `a@evil.example.com @other.com`, `x@[1.2.3.4]`, `a@-bad.example`, `a@bad..example`, `a@nodot`, `""@x.example y` give the empty string.
- `exp_2_input_redaction` (service integration) unchanged and green over every corpus message.

## Edge cases and traps

- Behaviour change to expect and accept: a trailing-dot domain (`example.com.`) and non-ASCII (un-punycoded IDN) domains now give an empty `from_domain` (fail closed; the classifier loses the domain feature for such senders, no privacy impact).
- Redaction must stay linear time: do not add a pattern with nested unbounded quantifiers (the regex crate is linear, but keep the quoted-local-part bounded).
- Idempotence and the 1 MB hostile-input bound (`exp_2_hostile_input_bounded`) must keep passing.
- No behaviour change for ordinary addresses and URLs.
- Do not touch sealed-token handling (finding N1 of the same review is a separate decision).

## Out of scope

- Sealed-token `TokenError::Malformed` handling in the swipe handler and the BAKE-5 verification row (review findings N1, N2).
- Any change to `auth_results.rs`, which takes its own `from_domain` argument.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
