# T-1120a: Synthetic mailbox generator v1 (deterministic, with ground truth)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: about 600 lines of Rust plus tests | T-204, T-205a |

**Read only these spec sections:** `docs/SYNTHETIC-MAILBOX-DESIGN.md` is in the private ops repo and is NOT available to you; its rules are restated below. Code to read: `backend/crates/testkit/src/corpus/mod.rs` (`CaseSpec`, `Expected`, `Corpus::all_addresses`, `all_urls`, `canaries`), `backend/crates/testkit/fixtures/mail/manifest.toml` (header comment and two cases), `backend/crates/testkit/src/mailbox/state.rs` (`SeedMessage`), `backend/crates/fake-google/src/seeder.rs`. Nothing else is needed.

## Why this task exists

The only mail data today is the 24 hand-written cases of T-204: right for correctness, useless for realism (no volume, history, threads or sender variety). The owner decided (11 Oct 2026) that staging gets a TEST mailbox only, filled with synthetic mail, with several demo accounts that can be reset on demand (T-1120b). This task produces the mail and its ground truth; it sends nothing anywhere.

## Goal

`cargo run -p testkit --example synth_mailbox -- --seed 1 --persona freelancer --messages 5000 --out DIR` writes a deterministic mailbox: `DIR/manifest.jsonl` (one JSON object per message) and `DIR/eml/<id>.eml`, byte-identical for the same arguments on every run and machine.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/testkit/src/synth/mod.rs`, `rng.rs`, `persona.rs`, `senders.rs`, `threads.rs`, `render.rs` | generator |
| Create | `backend/crates/testkit/examples/synth_mailbox.rs` | CLI |
| Create | `backend/crates/testkit/tests/synth_mailbox.rs` | tests below |
| Change | `backend/crates/testkit/src/lib.rs` | `pub mod synth;` |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1120a` |

## Rules (non-negotiable)

1. **Deterministic:** an in-tree SplitMix64 generator (about 15 lines); NO new dependency, no `HashMap` iteration order, no wall clock, no `rand`. Same seed and arguments give identical bytes.
2. **Reserved domains only (v1 has no exception):** every address and every URL host, in headers AND in text and HTML bodies, uses `example.com/net/org`, `*.test` or `*.invalid`. There is NO `--domain` option in v1; a real-domain tier is a separate owner-approved task with its own parsing, allow-list and egress rules. `Corpus::all_addresses` / `all_urls` are collectors, not validators (`backend/crates/testkit/tests/corpus.rs:55-94` holds the actual check): write a validator in the synth tests that extracts every email address and every `http(s)` URL from every emitted `.eml` (headers, text part, HTML part, including `href` and `src`) and asserts the host is reserved. No real brand names, logos or real people.
3. **Provenance marker:** every `.eml` carries the header `X-MT-Synthetic-Corpus: <corpus-hash>` (the hash of `SHA256SUMS` at generation) and a `Message-ID` of the form `<synth-<id>@<reserved-host>>`; the seed tool (T-1120b) relies on both. The manifest's first line is a header object `{corpus_hash, persona, seed, slice, generator_version}`.
4. **Ground truth on every message:** `expected.class` (one of the S3 names `list, bulk_no_header, notice, personal, suspect`), `bulk_score_min/max`, `method` (as `ExpectedMethod`), `reason`, `labels`, `internal_date`, a unique `canary`, and `hostile: bool`.
5. **Hostile content is flagged:** phishing-style, header-injection, huge-body and odd-encoding cases carry `hostile: true`; `--no-hostile` omits them (the loader for a real account always sets it).
6. **No send, no network.**

## Shape (v1)

- **Personas (3):** `student`, `freelancer`, `parent`; each has interests, 40 to 120 recurring senders drawn from archetypes (retailer, bank-style notice, newsletter, SaaS notification, community group, travel, utility, social, family), a cadence per sender (daily, weekly, monthly, burst around sales), a lifetime (start, optional address change, go quiet), and 5 to 15 personal correspondents.
- **Age:** messages span 1 to 10 years before `--epoch` (default 2026-10-01T00:00:00Z), skewed to recent; unread share rises with age; `internal_date` equals the `Date` header.
- **Threads:** 2 to 40 messages with consistent `Message-ID`, `In-Reply-To`, `References`, quoted replies, subject drift, forwards.
- **Spam spectrum:** every message gets a bulk band; signals vary and combine (missing or malformed `List-Unsubscribe`, one-click present or absent, tracking pixel and wrapped links to the testbed host only, urgency subjects, display-name and `Reply-To` mismatch). Include the legitimate-but-noisy cases (receipts, shipping, bank-style notices).
- **Sizes:** `--messages N` (default 5000); `--slice demo` selects a deterministic 500-message subset that keeps the class mix.
- **Output:** `manifest.jsonl` lines sorted by `internal_date`, `eml/` files named by a stable id, `SHA256SUMS` over all files, and `mbox` export with `--mbox`.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | Same arguments twice give byte-identical output (compare `SHA256SUMS`) |
| B2 | 5000 messages contain at least 100 of each of the five classes and every bulk band decile has at least 50 |
| B3 | The validator finds every address and URL host (headers, text, HTML `href`/`src`) in every emitted file reserved; no `hostile` message appears with `--no-hostile`; there is no `--domain` option |
| B4 | Every canary is unique and absent from `Corpus::all_canaries()` (no collision with the T-204 corpus) |
| B5 | Threads are consistent: every `In-Reply-To` and `References` id exists earlier in the same mailbox |
| B6 | The demo slice has exactly 500 messages and the same class proportions within 3 points |
| B8 | Every `.eml` has the `X-MT-Synthetic-Corpus` header equal to the manifest header's `corpus_hash` and a unique `synth-` Message-ID |
| B7 | Loading the manifest into `fake-google` with `FakeGoogleSeeder::seed` reproduces the message count and labels |

## Tests that must pass

`backend/crates/testkit/tests/synth_mailbox.rs` covering B1 to B8 (B2 may use 5000 messages if the runtime stays under 20 s; otherwise 2000 with proportional bounds and say so), plus all existing tests and required checks.

## Edge cases and traps

- Do not read the system clock, locale or environment anywhere in the generator.
- Do not write outside `--out`; refuse a non-empty `--out` without `--force`.
- Keep prose pools small and committed as fixtures; never call a model at generation time.
- File size limits and the binary allow-list apply: emit no binary files.

## Out of scope

Loading into Gmail (T-1120b), the seed workflow (owner-merged follow-up), any real-domain tier, attachments (v2).

## Done when

The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
