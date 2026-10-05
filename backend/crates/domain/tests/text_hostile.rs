//! Hostile-content and plain-text tests for the domain text module (T-402).
//!
//! Every case here is a "mail attacker writes this" input: markup, script,
//! bidirectional overrides, zero-width characters and multi-megabyte payloads.
//! The invariant is always the same — output is plain text a `Text` widget can
//! show, no markup survives, nothing is fetched, and length is bounded.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::text::{
    html_to_text, sanitise_plain, HTML_INPUT_MAX_BYTES, PREVIEW_MAX_CHARS, TEXT_INPUT_MAX_BYTES,
};
use proptest::prelude::*;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

/// The manifest is embedded at compile time; the `no I/O` check reads it.
const MANIFEST: &str = include_str!("../Cargo.toml");

/// Every character this module promises never to return.
const FORBIDDEN_CHARS: [char; 26] = [
    '\u{00AD}',
    '\u{034F}',
    '\u{061C}',
    '\u{180E}',
    '\u{200B}',
    '\u{200C}',
    '\u{200D}',
    '\u{200E}',
    '\u{200F}',
    '\u{202A}',
    '\u{202B}',
    '\u{202C}',
    '\u{202D}',
    '\u{202E}',
    '\u{2060}',
    '\u{2061}',
    '\u{2062}',
    '\u{2063}',
    '\u{2064}',
    '\u{2066}',
    '\u{2067}',
    '\u{2068}',
    '\u{2069}',
    '\u{FEFF}',
    '\u{E0001}',
    '\u{E007F}',
];

/// Assert `s` carries no invisible format or control characters.
fn assert_no_invisible(s: &str) {
    for c in FORBIDDEN_CHARS {
        assert!(!s.contains(c), "invisible character {c:?} survived: {s:?}");
    }
    assert!(
        !s.chars().any(char::is_control),
        "a control character survived: {s:?}"
    );
}

/// Assert `s` is safe plain text: no markup and no invisible format characters.
fn assert_plain(s: &str) {
    assert_no_invisible(s);
    assert!(!s.contains('<'), "markup survived sanitisation: {s:?}");
}

/// Random tag soup: only well-formed tag names, so every `<` in the input
/// starts a tag and must therefore be consumed by the tokenizer.
fn tag_soup() -> impl Strategy<Value = String> {
    let names = prop::sample::select(vec![
        "div", "p", "script", "style", "b", "span", "ul", "li", "table", "tr", "td", "br", "h1",
        "h2", "a", "img", "template", "svg", "iframe", "head", "title", "textarea", "select",
    ]);
    let tag = (names, any::<bool>(), any::<bool>()).prop_map(|(name, close, self_closing)| {
        if close {
            format!("</{name}>")
        } else if self_closing {
            format!("<{name}/>")
        } else {
            format!("<{name}>")
        }
    });
    let word = "[A-Za-z0-9 .,!?]{0,12}";
    prop::collection::vec(prop_oneof![tag, word], 0..40).prop_map(|parts| parts.concat())
}

proptest! {
    /// FD-01 AC2: any input gives at most 300 grapheme clusters.
    #[test]
    fn fd_01_ac2_preview_at_most_300_chars(input in ".*") {
        let plain = sanitise_plain(&input, PREVIEW_MAX_CHARS);
        prop_assert!(plain.graphemes(true).count() <= PREVIEW_MAX_CHARS);
        let preview = html_to_text(&input, PREVIEW_MAX_CHARS);
        prop_assert!(preview.graphemes(true).count() <= PREVIEW_MAX_CHARS);
    }

    /// FD-01 AC2 / ASVS V1.3.1: no tag from a random soup survives as markup.
    #[test]
    fn fd_01_ac2_html_stripped_no_tags_survive(soup in tag_soup()) {
        let out = html_to_text(&soup, PREVIEW_MAX_CHARS);
        prop_assert!(!out.contains('<'), "markup survived: {:?} from {:?}", out, soup);
    }
}

/// FD-01 AC2: an image or link attribute never becomes output text.
#[test]
fn fd_01_ac2_no_url_from_img_or_link_attributes() {
    assert_eq!(
        html_to_text(
            "<img src=\"https://tracker.example.com/p.gif\">",
            PREVIEW_MAX_CHARS
        ),
        ""
    );
    assert_eq!(
        html_to_text(
            "<a href=\"https://example.com/x\">Click</a>",
            PREVIEW_MAX_CHARS
        ),
        "Click"
    );
    // `alt` and `title` are attacker-controlled too and must not leak.
    assert!(!html_to_text(
        "<img src=x alt=\"LEAKED-ALT\" title=\"LEAKED-TITLE\">",
        PREVIEW_MAX_CHARS
    )
    .contains("LEAKED"));
}

/// FD-01 AC3: the module is pure — its manifest names no I/O crate.
#[test]
fn fd_01_ac3_text_module_has_no_io() {
    assert!(
        !MANIFEST.contains("reqwest"),
        "domain gained a network dependency"
    );
    assert!(
        !MANIFEST.contains("tracing"),
        "domain gained a logging dependency"
    );
    assert!(
        !(MANIFEST.contains("tokio") && MANIFEST.contains("net")),
        "domain gained tokio with networking"
    );
}

/// ASVS V1.3.1: script and style contents are removed, not rendered.
#[test]
fn asvs_v1_3_1_script_and_style_content_removed() {
    assert_eq!(
        html_to_text(
            "<script>alert(1)</script><style>p{}</style>Hello",
            PREVIEW_MAX_CHARS
        ),
        "Hello"
    );
    assert_eq!(
        html_to_text("<style>p{}</style>Hello", PREVIEW_MAX_CHARS),
        "Hello"
    );
    // An unclosed skip tag hides the rest of the document rather than leaking it.
    assert_eq!(html_to_text("<script>alert(1)", PREVIEW_MAX_CHARS), "");
    // Other skip elements behave the same way.
    let nested = html_to_text(
        "<template><p>hidden</p></template><p>shown</p>",
        PREVIEW_MAX_CHARS,
    );
    assert_eq!(nested, "shown");
}

/// ASVS V1.3.1: malformed markup never yields markup or a live script.
#[test]
fn asvs_v1_3_1_malformed_and_unclosed_tags() {
    // (input, exact expected output)
    let cases: [(&str, &str); 9] = [
        ("<script>alert(1)", ""),
        ("<!-- <p>x -->", ""),
        ("<div><span>ok</div>", "ok"),
        ("</b></i>", ""),
        ("<a href=\"x\">link", "link"),
        ("<p>a</p", "a"),
        ("<![CDATA[<script>alert(1)</script>]]>", "alert(1)]]>"),
        ("<scr<script>ipt>alert(1)</script>", "ipt>alert(1)"),
        ("<style>p{}<script>x</style>", ""),
    ];
    for (input, expected) in cases {
        let out = html_to_text(input, PREVIEW_MAX_CHARS);
        assert_eq!(out, expected, "input {input:?}");
        // Whatever the exact recovery, no markup and no invisible characters.
        assert_plain(&out);
    }
}

/// ASVS V1.3.3: bidirectional and zero-width characters are removed.
#[test]
fn asvs_v1_3_3_bidi_and_zero_width_removed() {
    let hostile = "a\u{202E}b\u{2066}c\u{200B}d\u{FEFF}e\u{061C}f\u{2069}g";
    assert_eq!(sanitise_plain(hostile, PREVIEW_MAX_CHARS), "abcdefg");
    // The same characters survive entity decoding and must still be removed.
    assert_eq!(
        html_to_text("a&#x202E;b&#x2066;c", PREVIEW_MAX_CHARS),
        "abc"
    );
    // Variation selectors are kept: they cannot reorder text and emoji need them.
    assert_eq!(
        sanitise_plain("x\u{FE0F}y", PREVIEW_MAX_CHARS),
        "x\u{FE0F}y"
    );
}

/// ASVS V1.3.3: controls are removed, whitespace controls become spaces.
#[test]
fn asvs_v1_3_3_controls_removed_newlines_become_spaces() {
    assert_eq!(sanitise_plain("a\u{0}b", PREVIEW_MAX_CHARS), "ab");
    assert_eq!(sanitise_plain("a\nb", PREVIEW_MAX_CHARS), "a b");
    assert_eq!(sanitise_plain("a\r\nb", PREVIEW_MAX_CHARS), "a b");
    assert_eq!(sanitise_plain("a\tb", PREVIEW_MAX_CHARS), "a b");
    assert_eq!(sanitise_plain("a\u{7f}b", PREVIEW_MAX_CHARS), "ab");
    assert_eq!(sanitise_plain("a\u{85}b", PREVIEW_MAX_CHARS), "a b");
    assert_eq!(sanitise_plain("a\u{2028}b", PREVIEW_MAX_CHARS), "a b");
    // Runs collapse and the result is trimmed.
    assert_eq!(sanitise_plain("  a \n\t b  ", PREVIEW_MAX_CHARS), "a b");
}

/// ASVS V1.3.3: multi-megabyte input is bounded and fast, with no stack growth.
#[test]
fn asvs_v1_3_3_huge_input_bounded() {
    let flat = "<p>hello</p>".repeat(1_000_000);
    assert!(flat.len() > 10 * 1024 * 1024, "fixture should exceed 10 MB");
    let nested = "<div>".repeat(2_000_000);

    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(200)
    };

    let started = Instant::now();
    let flat_out = html_to_text(&flat, PREVIEW_MAX_CHARS);
    let nested_out = html_to_text(&nested, PREVIEW_MAX_CHARS);
    let elapsed = started.elapsed();

    // Input caps are below the fixture size, so only the cap is tokenised.
    assert!(flat.len() > HTML_INPUT_MAX_BYTES);
    assert!(nested.len() > HTML_INPUT_MAX_BYTES);
    assert!(flat_out.graphemes(true).count() <= PREVIEW_MAX_CHARS);
    assert!(nested_out.graphemes(true).count() <= PREVIEW_MAX_CHARS);
    assert!(
        elapsed < budget,
        "10 MB HTML took {elapsed:?}, over the {budget:?} budget"
    );

    // Plain text is capped too, and cutting is on a character boundary.
    let huge = "a".repeat(10 * 1024 * 1024);
    let multibyte = "é".repeat(5_000_000);
    assert!(multibyte.len() > TEXT_INPUT_MAX_BYTES);
    let plain = sanitise_plain(&huge, PREVIEW_MAX_CHARS);
    assert!(plain.graphemes(true).count() <= PREVIEW_MAX_CHARS);
    let kept = sanitise_plain(&multibyte, usize::MAX);
    assert!(kept.chars().all(|c| c == 'é'));
}

/// Entities are decoded exactly once by the tokenizer.
#[test]
fn text_entities_decoded_once() {
    assert_eq!(html_to_text("&amp;lt;", PREVIEW_MAX_CHARS), "&lt;");
    assert_eq!(html_to_text("&amp;amp;", PREVIEW_MAX_CHARS), "&amp;");
    assert_eq!(html_to_text("&lt;b&gt;", PREVIEW_MAX_CHARS), "<b>");
}

/// The grapheme cut never splits a base character from its combining marks.
#[test]
fn text_grapheme_cut_keeps_emoji_whole() {
    let heart = "\u{2764}\u{FE0F}";
    let input = format!("xxxxx{heart}yyyyy");
    // Cut inside the emoji: the whole cluster is kept.
    let kept = sanitise_plain(&input, 7);
    assert_eq!(kept, format!("xxxxx{heart}\u{2026}"));
    // Cut before it: the whole cluster is dropped, never a lone variation selector.
    let dropped = sanitise_plain(&input, 6);
    assert_eq!(dropped, "xxxxx\u{2026}");
    assert!(!dropped.contains('\u{FE0F}'));
    // A combining mark is never separated from its base letter.
    let combining = "aaaa\u{0301}bbbb";
    let cut = sanitise_plain(combining, 6);
    assert_eq!(cut, "aaaa\u{0301}b\u{2026}");
    assert_eq!(cut.graphemes(true).count(), 6);
}

/// The S10 section 5 "Hostile content" fixtures: subject, display name and body
/// each end up as text, and the preview is at most 300 characters.
#[test]
fn text_hostile_corpus_cases() {
    let subject = "Invoice \u{202E}gnp.exe\u{202C} <script>alert(1)</script>\u{200B}\u{FEFF}";
    let display = "Acme\u{200B}Corp <img src=x onerror=alert(1)>\u{061C}";
    let body = "<html><head><title>x</title></head><body><p>Hello <b>world</b></p>\
                <script>bad()</script><img src=\"https://t.example/p.gif\">\
                Unsubscribe <a href=\"https://e.example/u\">here</a></body></html>";

    let subject_out = sanitise_plain(subject, 998);
    let display_out = sanitise_plain(display, 256);
    let body_out = html_to_text(body, PREVIEW_MAX_CHARS);
    let preview_out = html_to_text(&"long ".repeat(500), PREVIEW_MAX_CHARS);

    for out in [&subject_out, &display_out] {
        // A subject and a display name are text, not HTML: markup-like text is
        // shown as characters, but never as live markup and never invisible.
        assert_no_invisible(out);
    }
    for out in [&body_out, &preview_out] {
        assert_plain(out);
    }
    assert!(subject_out.contains("Invoice"));
    assert!(subject_out.contains("gnp.exe"));
    // The script stays visible text; it is not parsed, fetched or executed.
    assert!(subject_out.contains("script>alert(1)"));
    assert!(display_out.contains("Acme"));
    assert!(display_out.contains("onerror=alert(1)"));
    assert_eq!(body_out, "Hello world Unsubscribe here");
    assert!(!body_out.contains("bad()"));
    assert!(preview_out.graphemes(true).count() <= PREVIEW_MAX_CHARS);

    // Boundary behaviour of the cut.
    assert_eq!(sanitise_plain("abc", 0), "");
    assert_eq!(sanitise_plain("abc", 1), "\u{2026}");
    assert_eq!(sanitise_plain("ab", 2), "ab");
    assert_eq!(sanitise_plain("", 300), "");
}
