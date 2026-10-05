//! Leak scanning: prove that no canary, address or URL reached log output.

/// A leak found by `scan_for_leaks`. Never echo the needle itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub line_no: usize,
    pub needle_index: usize,
}

/// Case-sensitive substring search for each needle, plus the needle lower-cased
/// and its JSON-escaped form. Returns positions only, never the needle.
#[must_use]
pub fn scan_for_leaks(text: &str, needles: &[String]) -> Vec<Leak> {
    let mut found = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        for (idx, needle) in needles.iter().enumerate() {
            let forms = [
                needle.as_str(),
                &needle.to_lowercase(),
                &json_escape(needle),
            ];
            for form in forms {
                if line.contains(form) {
                    found.push(Leak {
                        line_no,
                        needle_index: idx,
                    });
                    break;
                }
            }
        }
    }
    found
}

/// The JSON-escaped form of a string (quotes, backslashes, control chars).
fn json_escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_plain_lowercase_and_escaped_forms() {
        let needles = vec!["CANARY-x".to_owned(), "a@example.com".to_owned()];
        let text =
            "line with CANARY-x\nline with a@example.com\nline with \\\"a@example.com\\\"\nclean";
        let leaks = scan_for_leaks(text, &needles);
        assert_eq!(leaks.len(), 3);
        assert_eq!(leaks[0].line_no, 0);
        assert_eq!(leaks[1].line_no, 1);
        assert_eq!(leaks[2].line_no, 2);
    }

    #[test]
    fn finds_lowercased_needle() {
        let needles = vec!["CANARY-X".to_owned()];
        let leaks = scan_for_leaks("has canary-x here", &needles);
        assert_eq!(leaks.len(), 1);
    }
}
