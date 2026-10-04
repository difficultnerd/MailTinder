# T-406: DKIM header coverage check

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | strong | about 450 lines of code plus tests | T-401, T-404 |

**Read only these spec sections:** S6 section 6 "One DKIM rule" and "Mailto" bullets, section 3 rows T9, T11, T16 (`docs/specs/S6-security.md`); S2 UN-02 AC1, AC2, AC2a; UN-03 AC3; UN-04 AC6; CL-01 AC2; SR-01 AC3, AC6; PB-01 AC4 (`docs/specs/S2-v1-acceptance-criteria.md`); S3 "Message classes" paragraph "Classifier inputs" and "Rule matching and counting" (`docs/specs/S3-domain-model.md`); S10 section 5 required cases table (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-401-gmail-read-messages-and-headers.md` `auth_results.rs` stub. Nothing else is needed.

## Goal

Replace T-401's fail-closed stub with the real check that decides which unsubscribe options a message may use and whether its From is authenticated. An option is offered only when Gmail's own `Authentication-Results` header reports a DKIM pass for a signature that we can identify among the message's `DKIM-Signature` headers, and whose `h=` tag lists `List-Unsubscribe` (and `List-Unsubscribe-Post` for one-click). The result fills `HeaderFacts.list_unsubscribe` and `HeaderFacts.from_authenticated`, which the header rules (T-102), guard (T-103), reject swipe (T-605) and unsubscribe senders (T-702, T-703) trust.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/adapters-gmail/src/auth_results.rs` | Real `assess_auth`; AR parser; DKIM-Signature tag parser; matching |
| Create | `backend/crates/adapters-gmail/src/list_unsubscribe.rs` | RFC 2369 `List-Unsubscribe` URI list parser |
| Change | `backend/crates/adapters-gmail/src/headers.rs` | Set `HeaderFacts.list_unsubscribe_present` from `AuthAssessment` |
| Create | `backend/crates/adapters-gmail/tests/dkim_coverage.rs` | Table tests over the S10 corpus cases |
| Change | `backend/crates/adapters-gmail/Cargo.toml` | Add `psl = "2"` (embedded public suffix list, no I/O) |

## Types and signatures

```rust
// auth_results.rs
pub const TRUSTED_AUTHSERV_ID: &str = "mx.google.com";   // Gmail's receiving server; [TUNABLE] only if Google changes it
pub const MIN_HEADER_B_CHARS: usize = 8;                 // Gmail reports the first 8 characters of b=

pub struct AuthAssessment {
    pub unsubscribe: Option<UnsubscribeOptions>,  // only DKIM-covered options; None when nothing qualifies
    pub list_unsubscribe_present: bool,           // the header exists at all, covered or not (CL-01 AC2, SR-01 AC3)
    pub from_authenticated: bool,                 // DKIM-aligned From or Gmail DMARC pass
}
pub fn assess_auth(h: &RawHeaders, from_domain: &str) -> AuthAssessment;

// Internal, unit tested directly (pub(crate))
pub(crate) struct ResInfo { pub method: String, pub result: String, pub props: Vec<(String, String)> } // keys lower case
pub(crate) struct AuthResults { pub authserv_id: String, pub results: Vec<ResInfo> }
pub(crate) fn parse_authentication_results(value: &str) -> Option<AuthResults>;
pub(crate) struct DkimSig { pub d: String, pub s: String, pub i_domain: String, pub h: Vec<String>, pub b: String }
pub(crate) fn parse_dkim_signature(value: &str) -> Option<DkimSig>;
pub(crate) fn strip_comments(value: &str) -> Option<String>;   // None on unbalanced parentheses or quotes

// list_unsubscribe.rs
pub(crate) struct LuUris { pub https: Option<Url>, pub mailto: Option<MailtoTarget> }
pub(crate) fn parse_list_unsubscribe(value: &str) -> LuUris;
pub(crate) const ONE_CLICK_VALUE: &str = "List-Unsubscribe=One-Click";
```

`HeaderFacts.list_unsubscribe_present` (added by T-101) is what CL-01 AC2 (`bulk_no_header`) and SR-01 AC3 (sender-only rules match only mail carrying the header) need; this task sets it from `AuthAssessment`. T-101's comment on `UnsubscribeOptions.https` allows plain `http`; this task does not (see step 5.6), because S6 6 and ASVS V1.2.2 accept only `https` and `mailto`, and T-704's `safe_link` refuses non-https links anyway.

## Algorithm

All header lookups use `RawHeaders` from T-401, which keeps Gmail's order (topmost header first) and duplicates.

### 1. Trusted Authentication-Results

1. Take the first `Authentication-Results` header in message order. If there is none, there is no trusted result.
2. `parse_authentication_results` on it. If it fails, or `authserv_id` is not `TRUSTED_AUTHSERV_ID` (ASCII case-insensitive), there is no trusted result.
3. Ignore every other `Authentication-Results` and every `ARC-Authentication-Results`. Gmail adds its own header above everything the sender wrote, so only the topmost can be Gmail's; a sender can write a fake `Authentication-Results: mx.google.com; dkim=pass` lower down.
4. With no trusted result: `unsubscribe = None`, `from_authenticated = false` (fail closed).

### 2. Parsing Authentication-Results (RFC 8601)

1. `strip_comments`: walk the characters; inside a quoted string (`"` to unescaped `"`, `\` escapes the next character) copy as is; outside, `(` increments depth, `)` decrements (below zero is a parse failure), `\` inside a comment escapes the next character, and characters at depth above zero are replaced by one space. End with depth 0 and no open quote, else `None`.
2. Split the result on `;` outside quoted strings.
3. The first piece is the authserv-id: its first whitespace-separated token. (A following version number is ignored.)
4. Each later piece, trimmed, if not empty and not `none`: split into tokens on whitespace outside quotes. The first token is `method[/version]=result`: `method` is the part before `=` with any `/version` removed, lower-cased; `result` the part after, lower-cased. Each further token of the form `key=value` with a `.` in `key` (for example `header.b`) is a property: key lower-cased, value with surrounding quotes removed. A `reason=` token is ignored.
5. Unparsable pieces are skipped; they do not fail the whole header.

Example Gmail value (from the corpus, example.com only):

```
mx.google.com;
       dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12;
       spf=pass (google.com: domain of bounce@example.com designates 192.0.2.1 as permitted sender) smtp.mailfrom=bounce@example.com;
       dmarc=pass (p=NONE sp=NONE dis=NONE) header.from=example.com
```

### 3. Parsing DKIM-Signature (RFC 6376 section 3.2)

1. Remove `\r` and `\n`. Split on `;`; trim each piece; skip empty pieces (a trailing `;` is legal).
2. Each piece is `tag=value`: tag is the text before the first `=`, trimmed; value the rest, trimmed. A piece with no `=` makes the signature invalid. A tag seen twice makes the signature invalid.
3. Required tags: `v` equal to `1`, `d`, `s`, `h`, `b`. Any missing: invalid.
4. `d`: lower-cased. `s`: lower-cased. `b`: all whitespace removed (keep case).
5. `h`: split on `:`, trim each name, lower-case, drop empty names. Must contain `from` (RFC 6376 requires it), else invalid.
6. `i`: optional. If present, take the part after the last `@`, lower-cased; it must equal `d` or end with `.` + `d`, else invalid. If absent, `i_domain = d`.
7. Invalid signatures are dropped silently.

### 4. Which signatures passed

For each result in the trusted header with `method == "dkim"` and `result == "pass"`:

1. Reported domain: property `header.d` if present (lower-cased); else the part after the last `@` of `header.i` (lower-cased); else skip this result.
2. Candidates: parsed signatures where, if `header.d` was present, `sig.d == header.d`; otherwise `sig.i_domain == domain of header.i`. This is the "d= domain matches" check.
3. If `header.s` is present, keep only candidates with `sig.s == header.s` (ASCII case-insensitive).
4. If `header.b` is present with at least `MIN_HEADER_B_CHARS` characters, keep only candidates whose `sig.b` starts with `header.b` (exact case).
5. Exactly one candidate: it is a passing signature. More than one: mark the group "ambiguous"; the group counts as covering a header only if every candidate in it covers that header. None: this result passes nothing.

`P` is the set of passing signatures (and ambiguous groups) from all `dkim=pass` results.

### 5. Coverage and options

1. `lu_count = count("List-Unsubscribe")`, `lup_count = count("List-Unsubscribe-Post")`. `list_unsubscribe_present = lu_count > 0`.
2. If `lu_count != 1`, `unsubscribe = None`. A second `List-Unsubscribe` above a signed one is how a sender's signed header gets shadowed; DKIM signs only the bottom instance.
3. `lu_covered`: some member of `P` has `list-unsubscribe` in `h`.
4. `one_click_covered`: some single member of `P` has both `list-unsubscribe` and `list-unsubscribe-post` in `h`, and `lup_count == 1`, and that header's value trimmed equals `ONE_CLICK_VALUE` exactly (RFC 8058).
5. If not `lu_covered`: `unsubscribe = None` (the message will be `bulk_no_header`; CL-01 AC2).
6. `parse_list_unsubscribe(value)`:
   1. Take every substring between `<` and the next `>`, in order; ignore text outside angle brackets (RFC 2369 comments and whitespace). Cap at 5 URIs and 2048 characters each `[DEFAULT]`.
   2. `https://...`: parse with `url::Url`; accept only scheme `https`, a host, no username or password, at most 2048 characters. Keep the first one.
   3. `mailto:...`: `MailtoTarget::parse` (T-404). Keep the first that parses.
   4. Every other scheme (`http`, `ftp`, `javascript`, ...) is ignored.
7. Build `UnsubscribeOptions`:
   - `one_click_https = Some(https)` when `one_click_covered` and an https URI exists.
   - `https = Some(https)` when an https URI exists but one-click does not qualify (UN-04 AC6, "Open unsubscribe page" link), else `None`.
   - `mailto = Some(target)` when a valid mailto exists.
   - If all three are `None`, `unsubscribe = None`.

### 6. From authentication

`from_authenticated` is true when either holds:

1. The trusted header has a result `method == "dmarc"`, `result == "pass"`, with `header.from` equal to `from_domain` (ASCII case-insensitive).
2. Some passing signature in `P` (not an ambiguous group) has `psl::domain_str(sig.d) == psl::domain_str(from_domain)` (relaxed alignment by organisational domain). If `psl` returns `None` for either, require `sig.d == from_domain`.

`from_domain` empty (no parsable From) gives false.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-02 AC1 | One-click is offered only when a passing DKIM signature covers both `List-Unsubscribe` and `List-Unsubscribe-Post` |
| UN-02 AC2 | Without such a signature, no unsubscribe option of any method is offered |
| UN-02 AC2a | DKIM over the unsubscribe headers is enough; DMARC pass is not required |
| UN-03 AC3 | Mailto is offered only when a passing DKIM signature covers `List-Unsubscribe` |
| UN-04 AC6 | The https-only "Open unsubscribe page" link comes only from the DKIM-covered header |
| CL-01 AC2 | A header without DKIM cover yields no options, so the class cannot be `list` |
| SR-01 AC6 | A From with no aligned DKIM and no DMARC pass is not authenticated |
| PB-01 AC4 | Spoofed personal mail is not authenticated, so it never counts toward a block |
| V1.2.2 | Only `https` and `mailto` unsubscribe URIs are accepted |

## Tests that must pass

All use hand-written header fixtures with `example.com`, `example.org`, `example.net` only, mirroring the S10 section 5 cases; T-204's corpus loader may supply them.

- `un_02_ac1_one_click_covered_by_passing_signature` (unit)
- `un_02_ac1_one_click_needs_same_signature_for_both_headers` (unit: one signature covers LU, another covers LUP: no one-click)
- `un_02_ac1_post_value_must_be_exact` (unit)
- `un_02_ac2_headers_not_in_h_tag_give_no_options` (unit)
- `un_02_ac2_failing_dkim_gives_no_options` (unit: `dkim=fail`)
- `un_02_ac2_fake_authentication_results_below_gmail_ignored` (unit: sender-written `mx.google.com; dkim=pass` as the second AR header)
- `un_02_ac2_first_ar_not_gmail_gives_no_options` (unit)
- `un_02_ac2_two_list_unsubscribe_headers_give_no_options` (unit)
- `un_02_ac2a_esp_domain_signature_dmarc_fail_still_one_click` (unit)
- `un_03_ac3_mailto_needs_dkim_cover` (unit)
- `un_04_ac6_https_only_link_from_covered_header` (unit)
- `cl_01_ac2_uncovered_header_present_but_no_options` (unit: `list_unsubscribe_present` true, `unsubscribe` None)
- `sr_01_ac6_spoofed_from_not_authenticated` (unit)
- `pb_01_ac4_relaxed_alignment_subdomain_signature_authenticates` (unit: `d=mail.example.com`, From `example.com`)
- `asvs_v1_2_2_http_and_javascript_uris_ignored` (unit)
- `dkim_ar_comments_and_quotes_parsed` (unit: nested comments, `;` inside a comment and inside a quoted string)
- `dkim_ambiguous_signatures_need_all_to_cover` (unit)
- `dkim_header_b_prefix_selects_signature` (unit: two signatures same `d` and `s`)
- `dkim_signature_duplicate_tag_invalid` (unit)
- `dkim_signature_i_outside_d_invalid` (unit)
- `dkim_parsers_never_panic` (property: random strings into both parsers and `assess_auth`)

## Edge cases and traps

- Trust only the topmost `Authentication-Results`, and only when its authserv-id is `mx.google.com`. Scanning all of them is the classic spoof.
- Do not verify DKIM cryptographically or call DNS; Gmail already did, and the spec relies on its result (no real DNS in tests).
- Header names inside `h=` are case-insensitive and may have whitespace around `:`.
- `header.b` is a prefix of `b=` with whitespace removed; compare case-sensitively (base64).
- `;` and `=` can appear inside comments and quoted strings; strip comments first.
- One-click needs both headers in the same signature's `h=`; two different signatures do not combine.
- `http://` links are dropped here; never upgrade them to https.
- `List-Unsubscribe-Post` present without one-click cover still allows the https link as a Needs Attention link (UN-04 AC6) when `List-Unsubscribe` is covered.
- Do not log header values, domains or selectors; at most counts and a reason code.
- Do not treat SPF pass as From authentication.

## Out of scope

- The class and score decisions that consume these facts: T-102, T-103.
- Sending the one-click POST and the mailto: T-702, T-703.
- Rule creation gated on `from_authenticated`: T-605.

## Security review checklist

- Only the first `Authentication-Results` header is consulted, and its authserv-id is compared exactly with `mx.google.com`.
- Every unparsable input fails closed (no options, not authenticated).
- A signature is counted only after its `d`, `s` (when reported) and `b` prefix (when reported) match the pass result; ambiguous groups need every member to cover.
- One-click requires the same signature to cover both headers, exactly one `List-Unsubscribe-Post` with the exact RFC 8058 value, and exactly one `List-Unsubscribe`.
- The https option rejects userinfo and non-https schemes; the mailto option goes through `MailtoTarget::parse` only.
- The URI list is bounded in count and length; parsers are linear and covered by a no-panic property test.
- No new network, DNS or logging of header content.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The T-401 stub is gone and `HeaderFacts.list_unsubscribe_present` is set by the Gmail adapter.
