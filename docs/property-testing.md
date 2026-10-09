# Property testing

Every parser of hostile input in the backend has property tests: they feed
arbitrary bytes and Unicode to the parser and assert an invariant that must hold
for *all* inputs, not just the cases the author thought of. `proptest` is a
workspace dependency (T-001); it is already used for text, redaction, undo, the
header classifier and the egress range table, and T-1110 adds it to the parsers
that had none.

## Where they live

Integration tests under each crate's `tests/` directory, one file per group of
parsers:

| File | Parsers |
| --- | --- |
| `backend/crates/adapters-gmail/tests/prop_list_unsubscribe.rs` | `list_unsubscribe.rs` (`List-Unsubscribe` URI list) |
| `backend/crates/adapters-gmail/tests/prop_headers.rs` | `headers.rs` and `auth_results.rs` (raw headers, `From`, DKIM coverage) |
| `backend/crates/domain/tests/prop_mailto_address.rs` | `mailto.rs` and `address.rs` |
| `backend/crates/domain/tests/prop_header_rules.rs` | `header_rules.rs` |
| `backend/crates/egress/tests/prop_target.rs` | `target.rs` and the allowlist URL rules |

The parser items these tests reach (`headers`, `list_unsubscribe`,
`auth_results` in `adapters-gmail`) are public. They hold only parsers of
untrusted text, no Gmail wire type, so the XC-02 rule (no wire type escapes the
crate root) still holds; the `xc_02_gmail_types_not_public` test lists the
allowed public modules.

## Running them

The default number of cases per property is `proptest`'s own default (256), so
the normal belt stays fast:

```sh
cd backend && cargo test
```

Each file can be run on its own, and the case count is raised with the
`PROPTEST_CASES` environment variable. The nightly run uses 20,000 cases:

```sh
cd backend
PROPTEST_CASES=20000 cargo test -p adapters-gmail --test prop_list_unsubscribe
PROPTEST_CASES=20000 cargo test -p adapters-gmail --test prop_headers
PROPTEST_CASES=20000 cargo test -p domain --test prop_mailto_address
PROPTEST_CASES=20000 cargo test -p domain --test prop_header_rules
PROPTEST_CASES=20000 cargo test -p egress --test prop_target
```

A single property can be selected by name, for example
`PROPTEST_CASES=20000 cargo test -p egress --test prop_target t1110_one_click_url_refuses_ip_literal_encodings`.

## `proptest-regressions/` files

When a property fails, `proptest` shrinks the input to a minimal case and
writes it to `proptest-regressions/<file>.txt` next to the test's manifest
(for example `backend/crates/domain/tests/header_rules.proptest-regressions`).
On every later run those cases are replayed first, so a fixed bug stays fixed.

- **Commit these files.** They are small, contain no real mail or personal
  data, and are the regression test for the bug the property found.
- **Never delete or edit them by hand** to make a run pass. If a stored case no
  longer fails, the property it guards was weakened; find out why before
  touching it.
- If a property finds a real bug, fix the bug and keep the regression file
  (T-1110: a property is never weakened to make it pass).

## Writing a new property

1. State the invariant in one sentence. Prefer an invariant over "does not
   panic": only accepting safe targets, only offering a covered unsubscribe,
   never emitting a raw CR or LF.
2. Generate the *hostile* inputs, not just valid ones: arbitrary `char`s (NUL,
   controls, bidirectional marks), lossy bytes (unpaired surrogates), CR/LF
   injection, folded and duplicated headers, and the decimal/hex/octal/short
   encodings of an IP address.
3. Keep the default case count; do not `#[ignore]` a property or lower a
   coverage floor to get green.
