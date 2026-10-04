# T-402: Server-side HTML to plain text

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | strong | about 300 lines of code plus tests | T-401 |

**Read only these spec sections:** S2 FD-01 AC2 and AC3 (`docs/specs/S2-v1-acceptance-criteria.md`); S6 section 3 row T2 (`docs/specs/S6-security.md`); S7 section 5.4 "Card" table and the paragraph after it (`docs/specs/S7-api-contract.md`); S10 section 5 row "Hostile content" and "Remote images and tracking pixels" (`docs/specs/S10-test-strategy.md`); ASVS register rows V1.3.1, V1.3.3 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

The `domain` crate gets two pure functions: `sanitise_plain`, which makes any mail string safe plain text (control, bidirectional and invisible characters removed, length cut on a character boundary), and `html_to_text`, which turns an HTML body into a short plain-text preview without loading anything. The Gmail adapter (T-401) uses them for previews, subjects and display names, and the model input redaction (T-903) builds on the same output.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/text.rs` | `sanitise_plain`, `html_to_text`, constants |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod text;` |
| Change | `backend/crates/domain/Cargo.toml` | Add `html5ever = "0.27"` (tokenizer only, pure Rust, no I/O), `unicode-segmentation = "1"` |
| Change | `backend/crates/adapters-gmail/src/headers.rs`, `read.rs` | Replace the T-401 `// T-402` placeholders with calls to these functions |
| Create | `backend/crates/domain/tests/text_hostile.rs` | Hostile corpus cases and property tests |

## Types and signatures

```rust
// domain/src/text.rs
pub const PREVIEW_MAX_CHARS: usize = 300;          // [TUNABLE] S2 FD-01 AC2
pub const HTML_INPUT_MAX_BYTES: usize = 512 * 1024; // [DEFAULT] stop tokenising after this; bounds CPU per card
pub const TEXT_INPUT_MAX_BYTES: usize = 256 * 1024; // [DEFAULT] same bound for plain text

/// Plain text safe to show with a Flutter `Text` widget and to log nowhere.
/// Removes C0 and C1 controls (newline and tab become a space), bidirectional
/// controls, zero-width and other invisible format characters; collapses runs
/// of whitespace to one space; trims; cuts to `max_chars` grapheme clusters.
pub fn sanitise_plain(input: &str, max_chars: usize) -> String;

/// Visible text of an HTML document, then `sanitise_plain`. Never resolves,
/// fetches or returns a URL from an attribute; attributes are ignored entirely.
pub fn html_to_text(html: &str, max_chars: usize) -> String;
```

## Algorithm

### `sanitise_plain(input, max_chars)`

1. Take at most `TEXT_INPUT_MAX_BYTES` of `input`, cut back to a `char` boundary (`str::is_char_boundary`).
2. Map each `char`:
   - `\n`, `\r`, `\t`, U+000B, U+000C, U+0085, U+2028, U+2029: a space.
   - Any other `char::is_control()` (C0 U+0000 to U+001F, DEL U+007F, C1 U+0080 to U+009F): dropped.
   - Bidirectional controls dropped: U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069.
   - Invisible format characters dropped: U+00AD (soft hyphen), U+034F, U+180E, U+200B to U+200D, U+2060 to U+2064, U+FEFF. Variation selectors U+FE00 to U+FE0F are kept `[DEFAULT]`: emoji need them and they cannot reorder text.
   - Tag characters U+E0000 to U+E007F dropped. Private use and unassigned code points are kept (they render as boxes, harmlessly).
   - U+00A0 and the other Unicode space separators (`char::is_whitespace`): a space.
3. Collapse runs of spaces to one space; trim both ends.
4. If the result has more than `max_chars` grapheme clusters (`unicode_segmentation::UnicodeSegmentation::graphemes(s, true)`), keep the first `max_chars - 1` and append U+2026 (horizontal ellipsis). `max_chars == usize::MAX` means no cut.

### `html_to_text(html, max_chars)`

1. Take at most `HTML_INPUT_MAX_BYTES`, cut back to a `char` boundary.
2. Run the `html5ever` tokenizer (not the tree builder; no DOM is built, so nesting depth cannot blow the stack) with a `TokenSink` that keeps:
   - `skip_depth: u32` and `out: String` (stop appending once `out.len()` exceeds `max_chars * 8` bytes `[DEFAULT]`, enough before collapsing).
3. On a start tag whose name is in `SKIP_TAGS` = `script`, `style`, `head`, `title`, `noscript`, `template`, `svg`, `math`, `iframe`, `object`, `embed`, `select`, `textarea`, `button`: `skip_depth += 1` unless self-closing. On the matching end tag: `skip_depth = skip_depth.saturating_sub(1)`.
4. On a start or end tag in `BLOCK_TAGS` = `p`, `div`, `br`, `li`, `tr`, `td`, `th`, `h1` to `h6`, `table`, `section`, `article`, `header`, `footer`, `blockquote`, `hr`, `ul`, `ol`: push one space.
5. On character tokens when `skip_depth == 0`: append the text. Entities are already decoded by the tokenizer.
6. Ignore comments, doctype, processing instructions, every attribute (`src`, `href`, `style`, `alt` included) and `img` entirely. Nothing about images, links or CSS reaches the output, so there are no tracking pixels and no URLs from attributes.
7. Return `sanitise_plain(&out, max_chars)`.
8. Malformed HTML never errors: the tokenizer recovers. An empty result is `""`.

The `html5ever` `TokenSink` signature differs between versions; pin the version above, and if the API differs from the docs you read, keep the behaviour in steps 2 to 7.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-01 AC2 | Preview is plain text, at most 300 characters, HTML stripped, no remote content loaded |
| FD-01 AC3 | Building a preview writes nothing to a store, log or cache (pure function, no I/O) |
| V1.3.1 | Mail HTML is reduced to plain text on the server; no markup survives |
| V1.3.3 | Input length and character limits applied before any later use |

## Tests that must pass

- `fd_01_ac2_preview_at_most_300_chars` (property, `domain`: any input gives at most 300 grapheme clusters)
- `fd_01_ac2_html_stripped_no_tags_survive` (property: output never contains `<` followed by an ASCII letter that came from a tag in the input; generate random tag soup with `proptest`)
- `fd_01_ac2_no_url_from_img_or_link_attributes` (unit: `<img src="https://tracker.example.com/p.gif">` and `<a href="https://example.com/x">Click</a>` give `"Click"`)
- `fd_01_ac3_text_module_has_no_io` (unit: reads `domain/Cargo.toml` and fails on `reqwest`, `tokio` with `net`, `tracing`)
- `asvs_v1_3_1_script_and_style_content_removed` (unit: `<script>alert(1)</script><style>p{}</style>Hello` gives `"Hello"`)
- `asvs_v1_3_1_malformed_and_unclosed_tags` (unit table: `<scr<script>ipt>`, unclosed `<script>`, `<!-- <p>x -->`, CDATA)
- `asvs_v1_3_3_bidi_and_zero_width_removed` (unit: U+202E, U+2066, U+200B, U+FEFF all removed)
- `asvs_v1_3_3_controls_removed_newlines_become_spaces` (unit)
- `asvs_v1_3_3_huge_input_bounded` (unit: 10 MB HTML and 10 MB of nested `<div>` finish under 200 ms in release and do not overflow the stack)
- `text_entities_decoded_once` (unit: `&amp;lt;` gives `&lt;`, not `<`)
- `text_grapheme_cut_keeps_emoji_whole` (unit: a family emoji at the cut point is kept or dropped whole)
- `text_hostile_corpus_cases` (unit over the S10 section 5 "Hostile content" fixtures from T-204: subject, display name and body each end up as text, preview at most 300)

## Edge cases and traps

- Do not use a regex to strip tags. `<scr<script>ipt>` and comments defeat it.
- Do not build a DOM with `html5ever::parse_document` or `scraper`: deep nesting costs memory and stack; the tokenizer is linear.
- Do not decode entities a second time after the tokenizer; `&amp;lt;` must stay `&lt;`.
- Do not cut by bytes or `char`s for the final length: cut by grapheme clusters so a combining mark or emoji is never split.
- Do not keep `alt` text or `title` attributes; they are attacker controlled and the spec wants visible text only.
- Do not add `ammonia` or any sanitiser that outputs HTML; the output is never HTML.
- Do not log the input or output, even at `trace` level.
- `max_chars == 0` returns `""`; `max_chars == 1` returns the ellipsis only when cutting.

## Out of scope

- Model input redaction (URLs, addresses, digit runs replaced): T-903.
- Choosing which MIME part to strip: T-401.
- The Flutter side rendering text with `Text` widgets: T-1002.

## Security review checklist

- No code path returns a string that came from an HTML attribute.
- Skip tags cover `script`, `style`, `template`, `svg`, `math`, `iframe`, `object`, `noscript`, and an unclosed skip tag hides the rest of the document instead of leaking it.
- Bidi, zero-width and control removal runs on the final output of both functions, after entity decoding (so `&#x202E;` is removed too).
- Input caps apply before tokenising; the hostile 10 MB case is in the test list and runs in CI.
- The crate added to `domain` has no I/O, network or logging dependency (`cargo tree -p domain` checked).
- The grapheme cut cannot panic on any input (property test covers random Unicode).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- T-401's placeholders are replaced and its `gmail_preview_html_part_is_stripped` test is enabled and passing.
