# T-204: Synthetic mail corpus and fixture loader

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 400 lines of code plus a 300-line TOML manifest and tests | T-101, T-203 |

**Read only these spec sections:** S10 5 (`docs/specs/S10-test-strategy.md`, the whole section including the required cases table) and S10 7.3 first bullet (canaries); `docs/backlog/T-406-dkim-header-coverage-check.md` sections "1. Trusted Authentication-Results" and the example Gmail value under section 2 (what the headers must look like). Nothing else is needed.

## Goal

`testkit` holds a synthetic mail corpus described in one TOML manifest. A loader turns each case into RFC 5322 bytes (`.eml`), a `SeedMessage` for `FakeMailbox`, and the expected outcome (class, score band, unsubscribe method, reason). Every display name, subject and body carries a canary token for leak tests. Agents add cases by editing TOML, never by hand-writing MIME.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/testkit/fixtures/mail/manifest.toml` | one `[[case]]` per S10 5 required case (at least 22) |
| Create | `backend/crates/testkit/src/corpus/mod.rs` | `Corpus`, `CorpusCase`, `Expected`, `load()` |
| Create | `backend/crates/testkit/src/corpus/build.rs` | header and MIME builder |
| Create | `backend/crates/testkit/src/bin/write_corpus.rs` | writes every case as `<id>.eml` into a given folder (for e2e seeding; output never committed) |
| Change | `backend/crates/testkit/Cargo.toml` | add `toml`, `base64`; `[[bin]] write-corpus` |
| Create | `backend/crates/testkit/tests/corpus.rs` | corpus rule tests |

## Types and signatures

```rust
// testkit/src/corpus/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum DkimScenario {
    PassCoversBoth,        // one-click: h= has List-Unsubscribe and List-Unsubscribe-Post; AR dkim=pass
    PassCoversListUnsub,   // h= has List-Unsubscribe only; AR dkim=pass
    PassNotCovering,       // signature passes but h= lacks the unsubscribe headers
    EspPassDmarcFail,      // d= is the ESP domain (example.net), pass; AR dmarc=fail for the From domain
    Fail,                  // AR dkim=fail
    Unsigned,              // no DKIM-Signature; AR dkim=none
    ForgedLowerAr,         // trusted AR says dkim=none; a second, lower AR from the sender claims pass
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ExpectedMethod { OneClick, Mailto, NeedsAttentionHttps, NeedsAttentionHttp, None }
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Expected {
    pub class: MessageClass,           // list, bulk_no_header, notice, personal, suspect
    pub bulk_score_min: u8, pub bulk_score_max: u8,
    pub method: ExpectedMethod,
    pub reason: String,                // short reason words, e.g. "one-click list"
    pub from_authenticated: bool,
}
#[derive(Clone, Debug, Deserialize)] #[serde(deny_unknown_fields)]
pub struct CaseSpec {
    pub id: String,                    // kebab-case, unique
    pub source: String,                // S10 5 "Source" column, e.g. "E1", "SR-01 AC3"
    pub from_display: String, pub from_address: String,  // reserved domains only
    pub reply_to: Option<String>,
    pub subject: String,
    pub list_id: Option<String>,
    pub list_unsubscribe: Option<String>,        // full header value, e.g. "<https://u.example.com/x>, <mailto:u@example.com?subject=stop>"
    pub one_click_post: bool,                    // adds List-Unsubscribe-Post: List-Unsubscribe=One-Click
    pub feedback_id: Option<String>,
    pub precedence: Option<String>, pub auto_submitted: Option<String>,
    pub extra_headers: Vec<(String, String)>,    // e.g. ESP fingerprint headers
    pub dkim: DkimScenario,
    pub dkim_domain: Option<String>,             // defaults to the From domain
    pub body_text: String,
    pub body_html: Option<String>,
    pub remote_images: bool,                     // adds <img src="https://img.example.com/p.gif"> to the HTML part
    pub body_repeat: u32,                        // repeat body_text N times (default 1); hostile 10,000-char case
    pub header_padding_bytes: u32,               // adds X-Pad headers to reach this size; hostile 1 MB case
    pub date: String,                            // RFC 3339
    pub labels: Vec<String>,
    pub expected: Expected,
}
pub struct CorpusCase { pub spec: CaseSpec, pub eml: Vec<u8>, pub headers: Vec<(String, String)> }
impl CorpusCase {
    pub fn seed_message(&self) -> SeedMessage;   // for FakeMailbox (T-203); facts built from `expected` and the spec
    pub fn canaries(&self) -> Vec<String>;       // every canary planted in this case
}
pub struct Corpus { pub cases: Vec<CorpusCase> }
impl Corpus { pub fn case(&self, id: &str) -> Option<&CorpusCase>; pub fn all_canaries(&self) -> Vec<String>;
              pub fn all_addresses(&self) -> Vec<String>; pub fn all_urls(&self) -> Vec<String>; }
pub fn load() -> Result<Corpus, String>;         // parses the manifest via include_str!, builds every case
pub const RESERVED_DOMAIN_SUFFIXES: [&str; 5] = ["example.com", "example.net", "example.org", ".test", ".invalid"];
```

## Algorithm

1. `load()`: `toml::from_str` the manifest (embedded with `include_str!`, so tests need no file paths). Reject duplicate IDs.
2. Canaries: the builder appends ` CANARY-<id>-display` to `from_display`, ` CANARY-<id>-subject` to `subject`, and a line `CANARY-<id>-body` to `body_text` and inside `body_html` (in a `<p>`). The manifest itself holds no canaries, so a case cannot forget one.
3. Headers, top first (Gmail order: newest headers at the top):
   1. `Authentication-Results: mx.google.com;` with `dkim=<pass|fail|none> header.i=@<dkim_domain> header.s=s1 header.b=<first 8 chars of b>`; `spf=pass smtp.mailfrom=<from domain>`; `dmarc=<pass|fail> header.from=<from domain>`. For `EspPassDmarcFail`, `dkim=pass header.i=@example.net` and `dmarc=fail`. For `Unsigned`, `dkim=none`.
   2. `DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=<dkim_domain>; s=s1; h=<list>; bh=<base64 of SHA-256 of the body>; b=<base64 of SHA-256 of id + "sig">`. `h=` always has `from:to:subject:date`, plus `list-unsubscribe` for `PassCoversListUnsub` and `PassCoversBoth`, plus `list-unsubscribe-post` for `PassCoversBoth`. `b=` is not a real signature `[DEFAULT]`: T-406 trusts Gmail's verdict and never verifies DKIM itself, so no test keys and no fake DNS are needed. `header.b` in the AR is the first 8 characters of this `b=`.
   3. For `ForgedLowerAr`: the trusted AR says `dkim=none`; after the DKIM header add a second `Authentication-Results: mx.google.com; dkim=pass ...` (the forgery T-406 must ignore).
   4. Then `From`, `Reply-To` (if set), `To: reader@example.org`, `Subject`, `Date` (RFC 5322 format from `date`), `Message-ID: <id@example.com>`, `List-Id`, `List-Unsubscribe`, `List-Unsubscribe-Post`, `Feedback-ID`, `Precedence`, `Auto-Submitted`, `extra_headers`, then `X-Pad-n` headers of 998-byte lines until `header_padding_bytes` is reached.
   5. Non-ASCII display names and subjects are written as RFC 2047 encoded words (`=?UTF-8?B?...?=`), so every hostile case exercises decoding.
4. Body: `text/plain; charset=utf-8` only, or `multipart/alternative` with plain and HTML parts when `body_html` is set. Use base64 transfer encoding for bodies with non-ASCII. Lines end in CRLF.
5. `seed_message()` builds `HeaderFacts` from the spec and `expected`: `list_unsubscribe` is `Some` only when `expected.method` is `OneClick` or `Mailto` (DKIM-covered); `from_authenticated` from `expected`; the rest from the spec headers.
6. `write_corpus <dir>` writes `<id>.eml` for each case; the e2e script (T-1101) and the fake-google seeding (T-205a) use it.
7. Required cases: one per row of the S10 5 table, plus `forged-lower-ar` and `esp-pass-dmarc-fail` if not already there. Suggested IDs: `one-click-covered`, `one-click-plus-mailto`, `esp-pass-dmarc-fail`, `one-click-not-covered`, `dkim-fail`, `mailto-covered`, `mailto-uncovered`, `https-only-covered`, `http-plain-link`, `notice-no-header`, `relay-no-header`, `transactional-one-click`, `bulk-body-link-only`, `phishing-lookalike`, `personal-one-to-one`, `same-sender-list-a`, `same-sender-list-b`, `same-sender-no-list`, `receipt-same-sender-no-lu`, `spoofed-personal`, `hostile-headers`, `hostile-long-preview`, `remote-images`, `forged-lower-ar`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | Fixture task. It supplies the S10 5 corpus that FD-01 AC2, UN-02 AC2, UN-03 AC3, SR-01 AC3, PB-01, CL-01 AC2 and LOG-1 tests use in their own tasks |

## Tests that must pass

- `corpus_loads_every_case` (unit).
- `corpus_uses_reserved_domains_only` (unit): every address, URL host, `dkim_domain` and `Message-ID` domain ends with a `RESERVED_DOMAIN_SUFFIXES` entry (S10 5 rule).
- `corpus_every_case_has_display_subject_and_body_canaries` (unit).
- `corpus_covers_every_s10_required_case` (unit): a hand-written list of the required IDs is a subset of the manifest.
- `corpus_eml_parses_back` (unit): for each case, split headers and body at the first CRLF CRLF; header count is at least 8; every line ends in CRLF; `Subject` decodes (encoded words) to the spec subject plus canary.
- `corpus_hostile_header_block_is_about_one_megabyte` (unit).
- `corpus_dkim_h_tag_matches_scenario` (unit).
- `corpus_seed_message_facts_follow_expected_method` (unit).

## Edge cases and traps

- Reserved domains only (RFC 2606): `example.com`, `example.net`, `example.org`, `*.test`, `*.invalid`. No real ESP domains, not even in `extra_headers` values; use names like `X-Mailer: esp-sample`.
- Never copy a real email into the corpus, even redacted.
- Header lines over 998 bytes are illegal; fold long values with CRLF plus a space.
- The forged lower `Authentication-Results` must sit below the trusted one; order matters to T-406.
- Keep the manifest free of canaries; the builder adds them, so leak tests can rely on every case having them.
- Do not read the manifest from disk at run time with a relative path; `include_str!` avoids working-directory surprises.
- `write_corpus` output goes to a temp or `target/` folder; add nothing it writes to git.
- `body_repeat` and `header_padding_bytes` default to 1 and 0 (`#[serde(default)]`).

## Out of scope

- Classifying the corpus (T-102), DKIM coverage (T-406), HTML stripping (T-402).
- Seeding `fake-google` (T-205a uses `CorpusCase::eml`).
- Real DKIM signing and a fake DNS resolver: not needed while T-406 relies on Gmail's `Authentication-Results` (reported against S10 5).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Gitleaks and the pre-commit `detect-private-key` hook pass on the new files (no key material of any kind).
