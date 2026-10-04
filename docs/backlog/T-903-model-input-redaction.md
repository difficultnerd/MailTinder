# T-903: Model input redaction

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | strong | about 350 lines of code plus about 300 lines of tests | T-402, T-901 |

**Read only these spec sections:** S4 5.5 (`docs/specs/S4-architecture.md`), CR-01 1.5 (`docs/change-requests/CR-01-pluggable-classifier-jev-pilot.md`), S5 "Third parties that receive data" row for Vertex AI and TypeSafe, and test EXP-2 (`docs/specs/S5-data-inventory.md`), S10 9.2 row EXP-2 and S10 5 paragraph on canary tokens (`docs/specs/S10-test-strategy.md`), S6 threat T18 (`docs/specs/S6-security.md`), CR-01a G4 rows `text_tokens_bucket` and `lang_is_english` (`docs/change-requests/CR-01a-bakeoff-publishable-stats.md`). Nothing else is needed.

## Goal

Build the exact, minimised input that Gemini and Jev receive, in memory, from a message's metadata and stripped text: an allowlist of header facts, the subject and about 500 tokens of body text, with URLs, email addresses and long digit runs replaced by placeholders, and nothing else. One rendering function produces the bytes both models send, so their input is identical. This task also fixes the shared question wording and the version strings recorded with every evaluation.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/redact.rs` | Pure redaction, truncation, token estimate, buckets |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod redact;` |
| Change | `backend/crates/domain/Cargo.toml` | Add `regex` and `whatlang` `[DEFAULT: both pure Rust, MIT]` |
| Change | `backend/crates/api/src/classify/input.rs` | Replace T-901's stub `build_input` |
| Create | `backend/crates/api/src/classify/prompt.rs` | `QUESTION_VERSION`, question text, `render_model_text` |
| Change | `backend/crates/ports/src/mail.rs` | Add `get_text` (see Types) |
| Change | `backend/crates/adapters-gmail/src/read.rs` | Implement `get_text`; `get_preview` calls it with 300 |
| Change | `backend/crates/testkit/src/fake_mailbox.rs` | Implement `get_text` |
| Create | `backend/crates/domain/tests/redact.rs` | Unit and property tests |
| Create | `backend/crates/api/tests/exp_2_input_redaction.rs` | Corpus-wide EXP-2 test |

## Types and signatures

```rust
// domain/src/redact.rs
pub const INPUT_VERSION: &str = "1";          // bump on any change to this file's rules
pub const MAX_TEXT_WORDS: usize = 500;        // "about 500 tokens" (S4 5.5); words stand in for tokens [DEFAULT]
pub const MAX_TEXT_CHARS: usize = 3000;       // hard cap after the word cut [DEFAULT]
pub const MODEL_TEXT_FETCH_CHARS: usize = 4000; // how much stripped text the Feed asks the adapter for [DEFAULT]
pub const URL_PLACEHOLDER: &str = "[url]";
pub const EMAIL_PLACEHOLDER: &str = "[email]";
pub const NUMBER_PLACEHOLDER: &str = "[number]";

/// Removes control, bidi and zero-width characters, then replaces URLs, then email addresses,
/// then long digit runs. Idempotent: redact(redact(x)) == redact(x).
pub fn redact(input: &str) -> String;
/// Cuts to MAX_TEXT_WORDS whitespace-separated words, then to MAX_TEXT_CHARS chars on a char boundary.
pub fn truncate_words(input: &str) -> String;
pub fn word_count(input: &str) -> usize;
/// Input token estimate used for cost (T-908a): ceil(chars / 4) of the rendered model text [DEFAULT].
pub fn approx_tokens(rendered: &str) -> u32;
pub fn text_tokens_bucket(words: usize) -> TextTokensBucket;      // <100, 100 to 300, >300 (CR-01a G4)
pub fn is_english(input: &str) -> bool;                          // whatlang: Lang::Eng and is_reliable(); empty is false
/// Domain part of an address, lower case; empty when there is no '@'.
pub fn from_domain(address: &str) -> String;

pub struct InputFacts { pub text_tokens_bucket: TextTokensBucket, pub lang_is_english: bool }

// api/src/classify/input.rs  (replaces T-901's stub; same signature plus side data)
pub fn build_input(meta: &MessageMeta, stripped_text: &str) -> (ClassifierInput, InputFacts);

// api/src/classify/prompt.rs
pub const QUESTION_VERSION: &str = "1";
pub const CLASS_OPTIONS: [(&str, &str); 5] = [            // fixed order (CR-01 1.1); same for both models
    ("list", "a mailing list or newsletter the person subscribed to"),
    ("bulk_no_header", "bulk or marketing mail with no proper unsubscribe header"),
    ("notice", "an account, billing or security notice from a service"),
    ("personal", "a one-to-one message written by a person"),
    ("suspect", "spam or phishing"),
];
pub const CLASS_QUESTION: &str = "Which kind of email is this?";
pub const BULK_QUESTION: &str = "How likely is it that this email was sent in bulk to many people, from 0 (certainly one-to-one) to 100 (certainly bulk)?";
/// The one rendering both models send as their content or `state`. Fixed field order, one per line.
pub fn render_model_text(input: &ClassifierInput) -> String;

// ports/src/mail.rs (shared trait change; report it)
/// Plain stripped text, cut to max_chars grapheme clusters. get_preview(mb, id) == get_text(mb, id, 300).
async fn get_text(&self, mb: &MailboxCtx, id: &MessageId, max_chars: usize) -> Result<String, MailError>;
```

`ClassifierInput` (T-201a) fields and how `build_input` fills them:

| Field | Value |
| --- | --- |
| `from_display` | `redact(&meta.from_display)` cut to 100 chars |
| `from_domain` | `from_domain(&meta.from_address)` (domain only, never the local part) |
| `list_id` | `meta.facts.list_id` passed through `redact`, cut to 200 chars |
| `has_list_unsubscribe` | `meta.facts.list_unsubscribe_present` (presence only, never the URL) |
| `has_list_unsubscribe_post` | `meta.facts.list_unsubscribe.map(one_click_https.is_some())` |
| `precedence` | `Some("bulk")` when `precedence_bulk`, else `None` |
| `auto_submitted` | `Some("auto-generated")` when `auto_submitted`, else `None` |
| `esp_header_names` | `meta.facts.esp_hint` as a one-element vector, else empty |
| `auth_summary` | `"from_authenticated=pass"` or `"=fail"` |
| `subject` | `redact(&meta.subject)` cut to 300 chars |
| `text` | `truncate_words(&redact(stripped_text))` |
| `input_version` | `INPUT_VERSION` |

`InputFacts` is computed on `redact(stripped_text)` before truncation.

## Algorithm

`redact(input)`, in this order:

1. Remove characters in: C0 and C1 controls except space (tab and newline become a space), U+061C, U+200B to U+200F, U+202A to U+202E, U+2060 to U+2064, U+2066 to U+2069, U+FEFF.
2. URLs to `[url]`, case-insensitive, in this order:
   - any `scheme:` from {`http`, `https`, `ftp`, `mailto`, `javascript`, `data`, `file`, `tel`} followed by non-space characters up to whitespace or one of `<>"'()[]{}`;
   - `www.` followed by the same;
   - a bare host with a path: `\b[a-z0-9-]+(\.[a-z0-9-]+)+/\S*`.
3. Email addresses to `[email]`: `[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)+` (also catches the user's own address).
4. Long digit runs to `[number]`: six or more digits, allowing single spaces, hyphens or dots between them: `\d(?:[ \-.]?\d){5,}` `[DEFAULT: "long" = 6 or more; dates such as 2026-10-03 are also replaced, which is acceptable over-redaction]`.
5. Collapse runs of whitespace to one space and trim.

Compile each regex once (`std::sync::OnceLock`); a regex that fails to compile is a test failure, so use `Regex::new(..)` in a function that returns `Result` and fail closed (return an empty string and log `redact_regex_error`) rather than `expect`.

`render_model_text(input)` writes exactly these lines, each `name: value`, `none` for empty values:

```
from_name: <from_display>
from_domain: <from_domain>
list_id: <list_id or none>
list_unsubscribe: present|absent
list_unsubscribe_post: one-click|absent
precedence: <precedence or none>
auto_submitted: <auto_submitted or none>
esp: <comma-joined esp_header_names or none>
authentication: <auth_summary>
subject: <subject>
text: <text>
```

The question wording and options are sent separately by each model (T-904, T-905) from the `prompt.rs` constants, so both ask the same thing.

Never included: `To`, `Cc`, any recipient address, the message ID, thread ID, labels, attachments, the unsubscribe URL or mailto, any header value not in the table.

Feed change: for a card whose gate is open, the Feed calls `get_text(.., MODEL_TEXT_FETCH_CHARS)` once and derives the 300-character preview from it with `sanitise_plain(text, PREVIEW_MAX_CHARS)`, so there is still one provider call per card. For gate-closed cards, keep `get_preview`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| EXP-2 | Input sent to Gemini or Jev holds no recipient address, URL, message ID or unsubscribe URL; URLs, addresses and long digit runs are placeholders; text cut at about 500 tokens |
| CL-03 AC1 | Both models receive identical input (one rendering) |
| V14.2.3 | Sensitive data is not sent to third parties beyond the minimised allowlist |

## Tests that must pass

- `exp_2_input_redaction` (service integration over every corpus message from T-204: render the input; it contains no corpus email address, no `http`, `https`, `www.` or `mailto`, no planted digit run, no message ID, no `To` or `Cc` value, no List-Unsubscribe value; `word_count(text) <= 500`)
- `exp_2_canary_in_subject_and_body_allowed_but_addresses_not` (service integration: the subject canary is present, the planted address canary is replaced)
- `exp_2_redact_table` (unit, cases below)
- `exp_2_redact_idempotent` (property: `redact(redact(s)) == redact(s)` for arbitrary strings)
- `exp_2_redact_never_leaves_at_sign_address` (property: generated `local@domain.tld` embedded in arbitrary text never survives)
- `exp_2_hostile_input_bounded` (unit: 1 MB of text returns within the word and char caps; RTL override and zero-width characters removed)
- `cl_03_ac1_render_is_identical_for_both_models` (unit: same input rendered twice gives equal bytes)
- `asvs_v14_2_3_only_allowlisted_fields_rendered` (unit: `render_model_text` output has exactly the 11 field names)
- `text_tokens_bucket_boundaries` (unit: 99, 100, 300, 301)
- `approx_tokens_rounds_up` (unit: 0 chars 0, 1 char 1, 8 chars 2, 9 chars 3)
- `is_english_cases` (unit: an English paragraph true; a German paragraph false; empty false)
- `get_text_matches_preview_at_300` (contract, `MailProvider` suite: `get_preview == get_text(.., 300)`)

Redaction table (input, expected):

| Input | Expected |
| --- | --- |
| `Visit https://example.com/a?b=1 now` | `Visit [url] now` |
| `see www.example.org/x.` | `see [url]` |
| `go to example.net/unsubscribe today` | `go to [url] today` |
| `mailto:list@example.com` | `[url]` |
| `Write to jo.bloggs+news@example.co.uk please` | `Write to [email] please` |
| `Call 0412 345 678` | `Call [number]` |
| `Ref 123456` | `Ref [number]` |
| `Order 12345` | `Order 12345` |
| `Card 4111-1111-1111-1111` | `Card [number]` |
| `a\u{202E}b\u{200B}c` | `abc` |
| `line1\nline2\tx` | `line1 line2 x` |

## Edge cases and traps

- Replace URLs before emails, or `https://user@example.com/x` leaves `https://[email]/x`.
- The From domain is allowed (S4 5.5); the From local part is not. Never pass `from_address` itself.
- The display name can contain an address (`"jo@example.com" <...>`): it goes through `redact`.
- `List-Id` can contain an address-like string; it goes through `redact`.
- Do not log the input, the rendered text or any intermediate string. Wrap the rendered text in `Sensitive` until it is handed to the egress body.
- `get_preview` keeps its 300-character contract (FD-01 AC2); do not loosen it for the Feed card.
- The canary tokens in subject and body are allowed to reach the models; EXP-2 is about addresses, URLs, digit runs and identifiers.
- Any change to these rules bumps `INPUT_VERSION`; any change to the question text bumps `QUESTION_VERSION` (CR-01a G5).

## Out of scope

- Calling the models: T-904, T-905. Storing buckets and versions: T-906a.

## Security review checklist

- The allowlist table above is the only source of fields; there is no generic header pass-through.
- URL, email and digit patterns are applied to every free-text field (display name, List-Id, subject, text), not only the body.
- No recipient field (`To`, `Cc`, `Bcc`, `Delivered-To`) is read anywhere in this module.
- The word and character caps hold for any input size, and regexes run in linear time (the `regex` crate guarantees this).
- Nothing in this module logs or stores text.
- `get_text` still never fetches an attachment or URL (T-401 rules).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR reports the `MailProvider::get_text` addition for CONVENTIONS.md.
