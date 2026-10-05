//! Plain-text reduction for mail content (T-402).
//!
//! Two pure functions with no I/O, no network and no logging: [`sanitise_plain`]
//! turns any string into safe plain text for a Flutter `Text` widget, and
//! [`html_to_text`] turns an HTML mail body into a short plain-text preview
//! without loading anything. Both bound their input first (ASVS V1.3.3) and
//! neither ever returns text that came from an HTML attribute, so a tracking
//! pixel or a link is dropped rather than surfaced (FD-01 AC2, ASVS V1.3.1).

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use unicode_segmentation::UnicodeSegmentation;

/// The longest preview kept `[TUNABLE]` (S2 FD-01 AC2).
pub const PREVIEW_MAX_CHARS: usize = 300;
/// Tokenising stops after this many input bytes `[DEFAULT]`; bounds CPU per card.
pub const HTML_INPUT_MAX_BYTES: usize = 512 * 1024;
/// Plain text is cut back to this many input bytes `[DEFAULT]`.
pub const TEXT_INPUT_MAX_BYTES: usize = 256 * 1024;

/// Elements whose content is never visible text: markup, styling, scripting,
/// embedded documents or form controls. Content under one is dropped entirely.
const SKIP_TAGS: [&str; 14] = [
    "script", "style", "head", "title", "noscript", "template", "svg", "math", "iframe", "object",
    "embed", "select", "textarea", "button",
];

/// Elements that separate visible text; each boundary becomes one space.
const BLOCK_TAGS: [&str; 21] = [
    "p",
    "div",
    "br",
    "li",
    "tr",
    "td",
    "th",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "section",
    "article",
    "header",
    "footer",
    "blockquote",
    "hr",
    "ol",
];

/// Plain text safe to show with a Flutter `Text` widget and to log nowhere.
///
/// Removes C0 and C1 controls (newline and tab become a space), bidirectional
/// controls, zero-width and other invisible format characters; collapses runs
/// of whitespace to one space; trims; cuts to `max_chars` grapheme clusters.
pub fn sanitise_plain(input: &str, max_chars: usize) -> String {
    let capped = cap_bytes(input, TEXT_INPUT_MAX_BYTES);
    let mut mapped = String::with_capacity(capped.len().min(TEXT_INPUT_MAX_BYTES));
    for c in capped.chars() {
        if let Some(kept) = map_char(c) {
            mapped.push(kept);
        }
    }
    cut_graphemes(&collapse_spaces(&mapped), max_chars)
}

/// Visible text of an HTML document, then [`sanitise_plain`]. Never resolves,
/// fetches or returns a URL from an attribute; attributes are ignored entirely.
pub fn html_to_text(html: &str, max_chars: usize) -> String {
    let capped = cap_bytes(html, HTML_INPUT_MAX_BYTES);
    let sink = TextSink {
        skip_depth: 0,
        out: String::new(),
        max_out_bytes: max_chars.saturating_mul(8),
    };
    let mut tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let mut input = BufferQueue::default();
    input.push_back(StrTendril::from(capped));
    let _ = tokenizer.feed(&mut input);
    tokenizer.end();
    sanitise_plain(&tokenizer.sink.out, max_chars)
}

/// Map one input character to its plain-text output, or `None` to drop it.
fn map_char(c: char) -> Option<char> {
    // Whitespace-like controls become a single space rather than vanishing.
    if matches!(
        c,
        '\n' | '\r' | '\t' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    ) {
        return Some(' ');
    }
    // Other C0/C1 controls (and DEL) are dropped.
    if c.is_control() {
        return None;
    }
    // Bidirectional overrides, zero-width and invisible format characters.
    if is_removed_format(c) {
        return None;
    }
    // Space separators (U+00A0 and friends) collapse to a plain space.
    if c.is_whitespace() {
        return Some(' ');
    }
    Some(c)
}

/// Invisible characters that carry no visible glyph and must not survive.
///
/// Variation selectors U+FE00 to U+FE0F are *kept*: emoji need them and they
/// cannot reorder text. Private-use and unassigned code points are kept too —
/// they render as boxes, harmlessly.
fn is_removed_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'                  // soft hyphen
            | '\u{034F}'            // combining grapheme joiner
            | '\u{061C}'            // Arabic letter mark
            | '\u{180E}'            // Mongolian vowel separator
            | '\u{200B}'..='\u{200D}' // zero-width space/non-joiner/joiner
            | '\u{200E}' | '\u{200F}' // LRM/RLM
            | '\u{202A}'..='\u{202E}' // LRE/RLE/PDF/LRO/RLO
            | '\u{2060}'..='\u{2064}' // word joiner, invisible operators
            | '\u{2066}'..='\u{2069}' // LRI/RLI/FSI/PDI
            | '\u{FEFF}'            // zero-width no-break space (BOM)
            | '\u{E0000}'..='\u{E007F}' // tag characters
    )
}

/// The longest prefix of `s` that fits in `max` bytes and ends on a `char`
/// boundary. Slicing on a boundary cannot panic or split a code point.
fn cap_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Collapse runs of spaces to one and trim both ends.
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
    }
    out.trim().to_owned()
}

/// Cut `s` to at most `max_chars` grapheme clusters, appending U+2026 when a
/// cut happens. `max_chars == usize::MAX` never cuts; `0` returns `""`.
fn cut_graphemes(s: &str, max_chars: usize) -> String {
    if max_chars == usize::MAX {
        return s.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    if s.graphemes(true).count() <= max_chars {
        return s.to_owned();
    }
    let mut cut: String = s.graphemes(true).take(max_chars - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// Token sink that collects visible text and discards everything else.
struct TextSink {
    /// Nesting depth of skip elements; text is dropped while it is non-zero.
    skip_depth: u32,
    /// Visible text collected so far (bounded by [`TextSink::max_out_bytes`]).
    out: String,
    /// Stop appending characters once `out` is longer than this.
    max_out_bytes: usize,
}

impl TextSink {
    /// Handle one tag: track skip nesting, and turn block boundaries into a
    /// space. Attributes are never read.
    fn on_tag(&mut self, tag: &Tag) {
        let name: &str = &tag.name;
        let is_skip = SKIP_TAGS.contains(&name);
        match tag.kind {
            TagKind::StartTag => {
                if is_skip {
                    if !tag.self_closing {
                        self.skip_depth = self.skip_depth.saturating_add(1);
                    }
                } else if self.skip_depth == 0 && is_block(name) {
                    self.out.push(' ');
                }
            }
            TagKind::EndTag => {
                if is_skip {
                    self.skip_depth = self.skip_depth.saturating_sub(1);
                } else if self.skip_depth == 0 && is_block(name) {
                    self.out.push(' ');
                }
            }
        }
    }
}

/// Whether `name` is a block-level element.
fn is_block(name: &str) -> bool {
    BLOCK_TAGS.contains(&name)
}

impl TokenSink for TextSink {
    type Handle = ();

    fn process_token(&mut self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        match token {
            Token::CharacterTokens(text) => {
                if self.skip_depth == 0 && self.out.len() <= self.max_out_bytes {
                    self.out.push_str(&text);
                }
            }
            Token::TagToken(tag) => self.on_tag(&tag),
            // Comments, doctypes, processing instructions and null characters
            // contribute no visible text; parse errors are recovered from.
            Token::CommentToken(_)
            | Token::DoctypeToken(_)
            | Token::NullCharacterToken
            | Token::EOFToken
            | Token::ParseError(_) => {}
        }
        TokenSinkResult::Continue
    }
}
