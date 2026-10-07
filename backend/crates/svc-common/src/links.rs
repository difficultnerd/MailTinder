//! `safe_link`: the one sanitiser for a URL that leaves the server on a Needs
//! Attention item (T-704, S6 6, S7 5.8).
//!
//! Only an `https` URL with a host, no username or password, no ASCII control
//! characters or whitespace and at most [`MAX_LINK_CHARS`] characters is
//! accepted. Everything else becomes `None` (ASVS V1.2.2), so a `javascript:`,
//! `data:` or plain `http:` link can never be stored on an item and therefore
//! never handed to the app to open.

use url::Url;

/// `[DEFAULT]` longer links are almost always tracking payloads.
pub const MAX_LINK_CHARS: usize = 2048;

/// Returns `Some` only for: scheme `https`, a host, no username or password, at
/// most [`MAX_LINK_CHARS`] characters, and no ASCII control characters or
/// whitespace anywhere in the input.
///
/// The input is checked before it is parsed: the URL parser silently strips
/// leading and trailing whitespace and removes tab, newline and carriage return
/// anywhere, so those must be refused here or they would be accepted.
pub fn safe_link(raw: &str) -> Option<Url> {
    if raw.len() > MAX_LINK_CHARS {
        return None;
    }
    if raw
        .chars()
        .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    // A host is required: `https` is a special scheme, so the parser always
    // derives one, and this refuses anything it could not.
    url.host()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASVS V1.2.2: only `https` links are stored on an item; `javascript:`,
    /// `data:`, `http:`, a userinfo URL, a too-long URL and a URL carrying
    /// whitespace or a control character are all dropped.
    #[test]
    fn asvs_v1_2_2_only_https_links_stored() -> Result<(), Box<dyn std::error::Error>> {
        let kept = safe_link("https://example.com/u").ok_or("an https link is kept")?;
        assert_eq!(kept.scheme(), "https");

        for raw in [
            "http://example.com/u",
            "javascript:alert(1)",
            "data:text/html,x",
            "https://user:pw@example.com/",
            "https://example.com/a b",
            "https://example.com/a\nb",
            "https://example.com/a\tb",
        ] {
            assert!(safe_link(raw).is_none(), "{raw} must be refused");
        }

        let prefix = "https://example.com/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_LINK_CHARS - prefix.len()));
        assert_eq!(at_limit.len(), MAX_LINK_CHARS);
        assert!(safe_link(&at_limit).is_some(), "at the cap is kept");

        let over_limit = format!("{prefix}{}", "a".repeat(MAX_LINK_CHARS - prefix.len() + 1));
        assert_eq!(over_limit.len(), MAX_LINK_CHARS + 1);
        assert!(safe_link(&over_limit).is_none(), "over the cap is refused");
        Ok(())
    }
}
