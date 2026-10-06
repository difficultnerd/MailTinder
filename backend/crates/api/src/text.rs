//! Plain-text sanitising for card fields (FD-01 AC2, ASVS V1).

/// FD-01 AC2 `[TUNABLE]`: the longest preview a card carries.
pub const PREVIEW_MAX_CHARS: usize = 300;

/// Characters dropped without a replacement: bidirectional controls and
/// zero-width characters.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200B}'..='\u{200D}'
            | '\u{FEFF}'
    )
}

/// Removes every C0 and C1 control character (tab and newline become spaces),
/// bidi controls (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069),
/// zero-width characters (U+200B to U+200D, U+FEFF), collapses runs of
/// whitespace to one space, trims, then truncates to `max_chars` characters.
///
/// Stripping happens before truncating, and the cut is on `char` boundaries.
#[must_use]
pub fn plain_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut pending_space = false;
    for c in s.chars() {
        if is_invisible(c) {
            continue;
        }
        if c.is_control() {
            // Tab, newline and the other layout controls read as a space; any
            // other C0 or C1 control character is removed outright.
            if matches!(c, '\t' | '\n' | '\r' | '\u{000B}' | '\u{000C}') {
                pending_space = true;
            }
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if count >= max_chars {
            break;
        }
        if pending_space && count > 0 {
            out.push(' ');
            count += 1;
            if count >= max_chars {
                break;
            }
        }
        pending_space = false;
        out.push(c);
        count += 1;
    }
    // A space written just before the limit would leave a trailing space.
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_01_ac2_bidi_and_control_characters_stripped() {
        let hostile =
            "a\u{202E}b\u{200B}c\u{FEFF}d\u{061C}e\u{2066}f\u{2069}g\u{0007}h\u{0085}i\tj\nk";
        assert_eq!(plain_text(hostile, 100), "abcdefghi j k");
    }

    #[test]
    fn fd_01_ac2_whitespace_collapsed_and_trimmed() {
        assert_eq!(plain_text("  a \r\n\t b   c  ", 100), "a b c");
        assert_eq!(plain_text("\u{200B}\n", 10), "");
    }

    #[test]
    fn fd_01_ac2_truncates_on_char_boundaries() {
        let s = "日本語のメール件名";
        assert_eq!(plain_text(s, 3), "日本語");
        assert_eq!(plain_text(s, 0), "");
        assert_eq!(
            plain_text(&"x".repeat(10_000), PREVIEW_MAX_CHARS).len(),
            300
        );
        assert_eq!(plain_text("ab cd", 3), "ab");
    }
}
