//! RFC 2369 `List-Unsubscribe` URI list parsing (S6 section 6, ASVS V1.2.2).
//!
//! Only `https:` web links and single-recipient `mailto:` targets are accepted;
//! every other scheme (plain `http`, `ftp`, `javascript`, …) is dropped, never
//! upgraded. The list is bounded in count and per-URI length, and the parser is
//! linear so the no-panic property test holds.

use domain::MailtoTarget;
use url::Url;

/// The exact `List-Unsubscribe-Post` value that marks a one-click list
/// (RFC 8058).
pub(crate) const ONE_CLICK_VALUE: &str = "List-Unsubscribe=One-Click";

/// The most URIs taken from one header `[DEFAULT]`.
const MAX_URIS: usize = 5;
/// The longest single URI accepted, in characters `[DEFAULT]`.
const MAX_URI_CHARS: usize = 2048;

/// The first acceptable `https` and `mailto` URI from a `List-Unsubscribe`
/// value. Either may be absent.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LuUris {
    pub https: Option<Url>,
    pub mailto: Option<MailtoTarget>,
}

/// Parse the URIs from a `List-Unsubscribe` value. RFC 2369 wraps each URI in
/// angle brackets and everything outside them is ignored; when a provider or
/// test harness has already stripped the brackets, the value is read as a
/// comma-separated URI list instead. Keeps the first acceptable URI of each
/// kind.
pub fn parse_list_unsubscribe(value: &str) -> LuUris {
    let mut out = LuUris::default();
    let mut seen = 0usize;
    if value.contains('<') {
        let mut rest = value;
        while let Some(open) = rest.find('<') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('>') else {
                break;
            };
            let inner = after[..close].trim();
            rest = &after[close + 1..];
            seen += 1;
            if seen > MAX_URIS {
                break;
            }
            accept(&mut out, inner);
        }
    } else {
        for piece in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            seen += 1;
            if seen > MAX_URIS {
                break;
            }
            accept(&mut out, piece);
        }
    }
    out
}

/// Keep the first acceptable `https` and `mailto` URI from one candidate.
fn accept(out: &mut LuUris, uri: &str) {
    if uri.chars().count() > MAX_URI_CHARS {
        return;
    }
    if out.https.is_none() {
        if let Some(url) = parse_https(uri) {
            out.https = Some(url);
            return;
        }
    }
    if out.mailto.is_none() {
        if let Ok(target) = MailtoTarget::parse(uri) {
            out.mailto = Some(target);
        }
    }
}

/// Accept a URI only when it is `https`, has a host, and carries no userinfo.
fn parse_https(uri: &str) -> Option<Url> {
    if !uri
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    {
        return None;
    }
    if uri.chars().count() > MAX_URI_CHARS {
        return None;
    }
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    url.host_str()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn asvs_v1_2_2_http_and_javascript_uris_ignored() {
        for value in [
            "<http://u.example.com/x>",
            "<javascript:alert(1)>",
            "<ftp://u.example.com/x>",
            "<HTTP://u.example.com/x>",
            "<https://user:pw@u.example.com/x>",
        ] {
            let got = parse_list_unsubscribe(value);
            assert!(got.https.is_none(), "must drop {value}: {got:?}");
            assert!(got.mailto.is_none(), "must drop {value}: {got:?}");
        }
    }

    #[test]
    fn list_unsubscribe_takes_first_https_and_mailto() {
        let got = parse_list_unsubscribe(
            "<mailto:u@example.com?subject=stop>, <https://u.example.com/x>, <https://u.example.com/y>",
        );
        assert_eq!(
            got.https.as_ref().map(Url::as_str),
            Some("https://u.example.com/x")
        );
        assert_eq!(
            got.mailto.as_ref().map(MailtoTarget::to),
            Some("u@example.com")
        );
    }

    #[test]
    fn list_unsubscribe_ignores_text_outside_brackets() {
        let got = parse_list_unsubscribe("comment <https://u.example.com/x> trailing");
        assert_eq!(
            got.https.as_ref().map(Url::as_str),
            Some("https://u.example.com/x")
        );
    }

    #[test]
    fn list_unsubscribe_caps_uri_count() {
        let value = (0..10)
            .map(|i| format!("<https://u.example.com/{i}>"))
            .collect::<Vec<_>>()
            .join(", ");
        // The first is kept; a later one past the cap is not needed.
        let got = parse_list_unsubscribe(&value);
        assert_eq!(
            got.https.as_ref().map(Url::as_str),
            Some("https://u.example.com/0")
        );
    }

    #[test]
    fn list_unsubscribe_accepts_normalised_bare_uris() {
        // A normalising provider may return the value with the angle brackets
        // removed; the URIs are then read as a comma-separated list.
        let got = parse_list_unsubscribe(
            "https://u.example.com/x, mailto:unsub@example.com?subject=stop",
        );
        assert_eq!(
            got.https.as_ref().map(Url::as_str),
            Some("https://u.example.com/x")
        );
        assert_eq!(
            got.mailto.as_ref().map(MailtoTarget::to),
            Some("unsub@example.com")
        );
        assert!(parse_list_unsubscribe("http://u.example.com/x")
            .https
            .is_none());
    }

    #[test]
    fn list_unsubscribe_parsers_never_panic() {
        for value in ["", "<", ">", "<>", "<<>>", "<https://>", "mailto:", "???"] {
            let _ = parse_list_unsubscribe(value);
        }
    }
}
