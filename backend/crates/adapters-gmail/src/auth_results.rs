//! DKIM header coverage check (T-406).
//!
//! Replaces the T-401 fail-closed stub. Gmail already verified DKIM, so this
//! module trusts only the topmost `Authentication-Results` header when its
//! authserv-id is [`TRUSTED_AUTHSERV_ID`], then matches each `dkim=pass` result
//! against the message's `DKIM-Signature` headers to decide which unsubscribe
//! options a message may use and whether its `From` is authenticated. Every
//! unparsable input fails closed.

use domain::UnsubscribeOptions;

use crate::headers::RawHeaders;
use crate::list_unsubscribe::{parse_list_unsubscribe, ONE_CLICK_VALUE};

/// Gmail's receiving server; only its `Authentication-Results` is trusted
/// `[TUNABLE]` only if Google changes it.
pub const TRUSTED_AUTHSERV_ID: &str = "mx.google.com";

/// Gmail reports the first 8 characters of `b=`.
pub const MIN_HEADER_B_CHARS: usize = 8;

/// What the adapter knows about authentication and unsubscribe headers.
pub struct AuthAssessment {
    /// DKIM-covered unsubscribe options; `None` when nothing qualifies.
    pub unsubscribe: Option<UnsubscribeOptions>,
    /// A `List-Unsubscribe` header exists at all, covered or not (CL-01 AC2,
    /// SR-01 AC3).
    pub list_unsubscribe_present: bool,
    /// DKIM-aligned `From` or Gmail DMARC pass.
    pub from_authenticated: bool,
}

/// One `method[/version]=result` result with its `key.value` properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResInfo {
    pub method: String,
    pub result: String,
    /// `(key, value)` pairs, keys lower case.
    pub props: Vec<(String, String)>,
}

/// A parsed `Authentication-Results` header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthResults {
    pub authserv_id: String,
    pub results: Vec<ResInfo>,
}

/// A parsed `DKIM-Signature` header (only the tags the check needs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DkimSig {
    pub d: String,
    pub s: String,
    pub i_domain: String,
    pub h: Vec<String>,
    pub b: String,
}

/// Assess the authentication headers for one message.
pub fn assess_auth(h: &RawHeaders, from_domain: &str) -> AuthAssessment {
    let lu_count = h.count("List-Unsubscribe");
    let lup_count = h.count("List-Unsubscribe-Post");
    let list_unsubscribe_present = lu_count > 0;

    // Only the first (topmost) Authentication-Results can be Gmail's own, and
    // only when its authserv-id matches exactly. A sender can write a fake one
    // lower down (UN-02 AC2).
    let trusted = h
        .first("Authentication-Results")
        .and_then(parse_authentication_results)
        .filter(|ar| ar.authserv_id.eq_ignore_ascii_case(TRUSTED_AUTHSERV_ID));

    let Some(ar) = trusted else {
        return AuthAssessment {
            unsubscribe: None,
            list_unsubscribe_present,
            from_authenticated: false,
        };
    };

    let sigs: Vec<DkimSig> = h
        .all("DKIM-Signature")
        .filter_map(parse_dkim_signature)
        .collect();

    let mut passing: Vec<Passing> = Vec::new();
    for res in &ar.results {
        if res.method == "dkim" && res.result == "pass" {
            if let Some(group) = match_signatures(res, &sigs) {
                passing.push(group);
            }
        }
    }

    let from_authenticated = from_authenticated(&ar, &passing, from_domain);

    let lu_covered = passing.iter().any(|group| {
        group
            .members
            .iter()
            .all(|sig| covers(sig, "list-unsubscribe"))
    });
    let one_click_covered = lup_count == 1
        && h.first("List-Unsubscribe-Post")
            .is_some_and(|v| v.trim() == ONE_CLICK_VALUE)
        && passing
            .iter()
            .any(|group| !group.ambiguous && group.members.first().is_some_and(covers_both));

    let unsubscribe = if lu_count != 1 || !lu_covered {
        None
    } else {
        let uris = parse_list_unsubscribe(h.first("List-Unsubscribe").unwrap_or_default());
        let mut options = UnsubscribeOptions::default();
        if one_click_covered {
            options.one_click_https = uris.https;
        } else {
            options.https = uris.https;
        }
        options.mailto = uris.mailto;
        if options.one_click_https.is_none() && options.https.is_none() && options.mailto.is_none()
        {
            None
        } else {
            Some(options)
        }
    };

    AuthAssessment {
        unsubscribe,
        list_unsubscribe_present,
        from_authenticated,
    }
}

/// A `dkim=pass` result's candidates: exactly one signature, or an ambiguous
/// group (every member must cover a header for the group to count).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Passing {
    members: Vec<DkimSig>,
    ambiguous: bool,
}

/// The first property with `key` on a result, if any.
fn prop<'a>(res: &'a ResInfo, key: &str) -> Option<&'a str> {
    res.props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Find the signatures a `dkim=pass` result refers to. `None` when it refers to
/// no signature.
fn match_signatures(res: &ResInfo, sigs: &[DkimSig]) -> Option<Passing> {
    let header_d = prop(res, "header.d")
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty());
    let domain = match &header_d {
        Some(d) => d.clone(),
        None => {
            let header_i = prop(res, "header.i")?;
            let (_, d) = header_i.rsplit_once('@')?;
            let d = d.trim().to_ascii_lowercase();
            if d.is_empty() {
                return None;
            }
            d
        }
    };

    let mut candidates: Vec<DkimSig> = sigs
        .iter()
        .filter(|sig| match &header_d {
            Some(d) => sig.d == *d,
            None => sig.i_domain == domain,
        })
        .cloned()
        .collect();

    if let Some(header_s) = prop(res, "header.s")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        candidates.retain(|sig| sig.s.eq_ignore_ascii_case(header_s));
    }
    if let Some(header_b) = prop(res, "header.b") {
        if header_b.chars().count() >= MIN_HEADER_B_CHARS {
            candidates.retain(|sig| sig.b.starts_with(header_b));
        }
    }

    match candidates.len() {
        0 => None,
        1 => Some(Passing {
            members: candidates,
            ambiguous: false,
        }),
        _ => Some(Passing {
            members: candidates,
            ambiguous: true,
        }),
    }
}

/// Whether a signature covers a header name (already lower case).
fn covers(sig: &DkimSig, name: &str) -> bool {
    sig.h.iter().any(|n| n == name)
}

/// Whether a signature covers both unsubscribe headers.
fn covers_both(sig: &DkimSig) -> bool {
    covers(sig, "list-unsubscribe") && covers(sig, "list-unsubscribe-post")
}

/// The `From` is authenticated when Gmail's DMARC passed for it, or a single
/// passing signature is DKIM-aligned by organisational domain.
fn from_authenticated(ar: &AuthResults, passing: &[Passing], from_domain: &str) -> bool {
    if from_domain.is_empty() {
        return false;
    }
    let dmarc = ar.results.iter().any(|res| {
        res.method == "dmarc"
            && res.result == "pass"
            && prop(res, "header.from").is_some_and(|d| d.trim().eq_ignore_ascii_case(from_domain))
    });
    if dmarc {
        return true;
    }
    passing.iter().any(|group| {
        !group.ambiguous
            && group
                .members
                .first()
                .is_some_and(|sig| aligned(&sig.d, from_domain))
    })
}

/// Relaxed alignment by organisational domain, falling back to an exact match
/// when the public suffix list does not know a domain.
fn aligned(sig_domain: &str, from_domain: &str) -> bool {
    match (psl::domain_str(sig_domain), psl::domain_str(from_domain)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => sig_domain.eq_ignore_ascii_case(from_domain),
    }
}

/// Strip RFC 5322 comments and keep quoted strings as is. `None` on unbalanced
/// parentheses or quotes.
pub(crate) fn strip_comments(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut depth: u32 = 0;
    let mut in_quote = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quote {
            out.push(c);
            match c {
                '\\' => out.push(chars.next().unwrap_or(' ')),
                '"' => in_quote = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' if depth == 0 => {
                in_quote = true;
                out.push(c);
            }
            '(' => {
                depth += 1;
                out.push(' ');
            }
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                out.push(' ');
            }
            '\\' if depth > 0 => {
                chars.next();
            }
            _ if depth > 0 => out.push(' '),
            _ => out.push(c),
        }
    }
    if depth != 0 || in_quote {
        None
    } else {
        Some(out)
    }
}

/// Parse an `Authentication-Results` header (RFC 8601). `None` when the header
/// cannot be parsed at all.
pub(crate) fn parse_authentication_results(value: &str) -> Option<AuthResults> {
    let cleaned = strip_comments(value)?;
    let pieces = split_outside_quotes(&cleaned, ';');
    let authserv_id = pieces.first()?.split_whitespace().next()?.to_owned();
    if authserv_id.is_empty() {
        return None;
    }
    let mut results = Vec::new();
    for piece in &pieces[1..] {
        let trimmed = piece.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            continue;
        }
        if let Some(res) = parse_res_piece(trimmed) {
            results.push(res);
        }
    }
    Some(AuthResults {
        authserv_id,
        results,
    })
}

/// Parse one `method[/version]=result key.value=...` piece.
fn parse_res_piece(piece: &str) -> Option<ResInfo> {
    let tokens = tokenize(piece);
    let first = tokens.first()?;
    let (key, result) = first.split_once('=')?;
    let method = key.split('/').next()?.trim().to_ascii_lowercase();
    if method.is_empty() {
        return None;
    }
    let result = result.trim().to_ascii_lowercase();
    let mut props = Vec::new();
    for token in &tokens[1..] {
        if let Some((k, v)) = token.split_once('=') {
            let k = k.trim().to_ascii_lowercase();
            if k.contains('.') {
                props.push((k, unquote(v.trim())));
            }
        }
    }
    Some(ResInfo {
        method,
        result,
        props,
    })
}

/// Parse a `DKIM-Signature` header (RFC 6376 section 3.2). `None` when the
/// signature is invalid; invalid signatures are dropped silently by the caller.
pub(crate) fn parse_dkim_signature(value: &str) -> Option<DkimSig> {
    let cleaned: String = value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    let mut tags: Vec<(String, String)> = Vec::new();
    for piece in cleaned.split(';') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (tag, val) = piece.split_once('=')?;
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() || tags.iter().any(|(t, _)| *t == tag) {
            return None;
        }
        tags.push((tag, val.trim().to_owned()));
    }
    let get = |name: &str| {
        tags.iter()
            .find(|(t, _)| t == name)
            .map(|(_, v)| v.as_str())
    };

    if get("v")?.trim() != "1" {
        return None;
    }
    let d = get("d")?.trim().to_ascii_lowercase();
    let s = get("s")?.trim().to_ascii_lowercase();
    if d.is_empty() || s.is_empty() {
        return None;
    }
    let b: String = get("b")?.chars().filter(|c| !c.is_whitespace()).collect();
    if b.is_empty() {
        return None;
    }
    let h: Vec<String> = get("h")?
        .split(':')
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    if !h.iter().any(|n| n == "from") {
        return None;
    }
    let i_domain = match get("i") {
        Some(i) => {
            let (_, domain) = i.rsplit_once('@')?;
            let domain = domain.trim().to_ascii_lowercase();
            if domain != d && !domain.ends_with(&format!(".{d}")) {
                return None;
            }
            domain
        }
        None => d.clone(),
    };
    Some(DkimSig {
        d,
        s,
        i_domain,
        h,
        b,
    })
}

/// Split on `delim` outside quoted strings, respecting `\` escapes.
fn split_outside_quotes(s: &str, delim: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quote = false;
            }
        } else if c == '"' {
            in_quote = true;
        } else if c == delim {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// Split on whitespace outside quoted strings, keeping the quotes.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_quote {
            cur.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quote = false;
            }
        } else if c == '"' {
            in_quote = true;
            cur.push(c);
        } else if c.is_ascii_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Remove one pair of surrounding quotes, if present.
fn unquote(v: &str) -> String {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 && bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"') {
        v[1..v.len() - 1].to_owned()
    } else {
        v.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn raw(pairs: &[(&str, &str)]) -> RawHeaders {
        RawHeaders(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    /// A `DKIM-Signature` value with the tags the check reads.
    fn sig(d: &str, s: &str, h: &str, b: &str) -> String {
        format!("v=1; a=rsa-sha256; c=relaxed/relaxed; d={d}; s={s}; h={h}; bh=Zmh4; b={b}")
    }

    const H_BOTH: &str = "from:to:subject:date:list-unsubscribe:list-unsubscribe-post";
    const H_LU: &str = "from:to:subject:date:list-unsubscribe";
    const H_NONE: &str = "from:to:subject:date";

    #[test]
    fn un_02_ac1_one_click_covered_by_passing_signature() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12; dmarc=pass header.from=example.com",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        let opts = a.unsubscribe.expect("one-click options");
        assert_eq!(
            opts.one_click_https.as_ref().map(url::Url::as_str),
            Some("https://u.example.com/x")
        );
        assert!(opts.https.is_none());
        assert!(opts.mailto.is_none());
        assert!(a.from_authenticated);
    }

    #[test]
    fn un_02_ac1_one_click_needs_same_signature_for_both_headers() {
        // One signature covers List-Unsubscribe, another covers the Post
        // header; they must not combine (RFC 8058).
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.d=example.com header.s=s1 header.b=AAAA1111; dkim=pass header.d=example.com header.s=s2 header.b=BBBB2222",
            ),
            ("DKIM-Signature", &sig("example.com", "s1", H_LU, "AAAA1111zz")),
            (
                "DKIM-Signature",
                &sig("example.com", "s2", "from:to:subject:date:list-unsubscribe-post", "BBBB2222zz"),
            ),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        let opts = a.unsubscribe.expect("https options");
        assert!(opts.one_click_https.is_none());
        assert_eq!(
            opts.https.as_ref().map(url::Url::as_str),
            Some("https://u.example.com/x")
        );
    }

    #[test]
    fn un_02_ac1_post_value_must_be_exact() {
        // The value is trimmed, then compared exactly; case and any extra text
        // disqualify one-click.
        for value in [
            "List-Unsubscribe=one-click",
            "one-click",
            "List-Unsubscribe=One-Click now",
        ] {
            let h = raw(&[
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
                ),
                ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.com/x>"),
                ("List-Unsubscribe-Post", value),
            ]);
            let opts = assess_auth(&h, "example.com").unsubscribe.expect("options");
            assert!(
                opts.one_click_https.is_none(),
                "value {value:?} must not one-click"
            );
            assert!(opts.https.is_some(), "value {value:?} keeps the https link");
        }
    }

    #[test]
    fn un_02_ac2_headers_not_in_h_tag_give_no_options() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_NONE, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(a.list_unsubscribe_present);
        assert!(a.unsubscribe.is_none());
    }

    #[test]
    fn un_02_ac2_failing_dkim_gives_no_options() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=fail header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(a.unsubscribe.is_none());
        assert!(!a.from_authenticated);
    }

    #[test]
    fn un_02_ac2_fake_authentication_results_below_gmail_ignored() {
        let h = raw(&[
            ("Authentication-Results", "mx.google.com; dkim=none header.i=@news.example.com"),
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        assert!(assess_auth(&h, "example.com").unsubscribe.is_none());
    }

    #[test]
    fn un_02_ac2_first_ar_not_gmail_gives_no_options() {
        let h = raw(&[
            (
                "Authentication-Results",
                "example.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(a.unsubscribe.is_none());
        assert!(!a.from_authenticated);
    }

    #[test]
    fn un_02_ac2_two_list_unsubscribe_headers_give_no_options() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://evil.example.net/x>"),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(a.list_unsubscribe_present);
        assert!(a.unsubscribe.is_none());
    }

    #[test]
    fn un_02_ac2a_esp_domain_signature_dmarc_fail_still_one_click() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@example.net header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com",
            ),
            ("DKIM-Signature", &sig("example.net", "sel1", H_BOTH, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.net/x>"),
            ("List-Unsubscribe-Post", ONE_CLICK_VALUE),
        ]);
        let a = assess_auth(&h, "example.com");
        let opts = a.unsubscribe.expect("one-click from ESP signature");
        assert!(opts.one_click_https.is_some());
        assert!(!a.from_authenticated);
    }

    #[test]
    fn un_03_ac3_mailto_needs_dkim_cover() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<mailto:unsub@example.com?subject=stop>"),
        ]);
        let opts = assess_auth(&h, "example.com")
            .unsubscribe
            .expect("mailto option");
        assert_eq!(
            opts.mailto.as_ref().map(domain::MailtoTarget::to),
            Some("unsub@example.com")
        );
        assert!(opts.one_click_https.is_none());
        assert!(opts.https.is_none());
    }

    #[test]
    fn un_04_ac6_https_only_link_from_covered_header() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
        ]);
        let opts = assess_auth(&h, "example.com")
            .unsubscribe
            .expect("https option");
        assert_eq!(
            opts.https.as_ref().map(url::Url::as_str),
            Some("https://u.example.com/x")
        );
        assert!(opts.one_click_https.is_none());
    }

    #[test]
    fn cl_01_ac2_uncovered_header_present_but_no_options() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_NONE, "AbCdEf12zzzz")),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
        ]);
        let a = assess_auth(&h, "example.com");
        assert!(a.list_unsubscribe_present);
        assert!(a.unsubscribe.is_none());
    }

    #[test]
    fn sr_01_ac6_spoofed_from_not_authenticated() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@example.net header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com",
            ),
            ("DKIM-Signature", &sig("example.net", "sel1", H_NONE, "AbCdEf12zzzz")),
        ]);
        assert!(!assess_auth(&h, "example.com").from_authenticated);
    }

    #[test]
    fn pb_01_ac4_relaxed_alignment_subdomain_signature_authenticates() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@mail.example.com header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com",
            ),
            ("DKIM-Signature", &sig("mail.example.com", "sel1", H_NONE, "AbCdEf12zzzz")),
        ]);
        assert!(assess_auth(&h, "example.com").from_authenticated);
    }

    #[test]
    fn asvs_v1_2_2_http_and_javascript_uris_ignored() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12",
            ),
            ("DKIM-Signature", &sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
            (
                "List-Unsubscribe",
                "<http://u.example.com/x>, <javascript:alert(1)>, <https://u.example.com/ok>",
            ),
        ]);
        let opts = assess_auth(&h, "example.com").unsubscribe.expect("options");
        assert_eq!(
            opts.https.as_ref().map(url::Url::as_str),
            Some("https://u.example.com/ok")
        );
    }

    #[test]
    fn dkim_ar_comments_and_quotes_parsed() {
        let value = "mx.google.com (top; with ; and (nested)); \
             dkim=pass header.d=example.com header.s=sel1 header.b=AbCdEf12 \
             (reason; ignored) header.i=@example.com; \
             spf=pass smtp.mailfrom=\"a;b@example.com\"";
        let cleaned = strip_comments(value).expect("balanced");
        assert!(!cleaned.contains("nested"));
        let ar = parse_authentication_results(value).expect("parses");
        assert_eq!(ar.authserv_id, "mx.google.com");
        assert_eq!(ar.results.len(), 2);
        let dkim = &ar.results[0];
        assert_eq!(dkim.method, "dkim");
        assert_eq!(dkim.result, "pass");
        assert_eq!(prop(dkim, "header.d"), Some("example.com"));
        assert_eq!(prop(dkim, "header.b"), Some("AbCdEf12"));
        assert_eq!(prop(dkim, "header.i"), Some("@example.com"));
        // The `;` inside the quoted string did not split the spf piece.
        assert_eq!(
            prop(&ar.results[1], "smtp.mailfrom"),
            Some("a;b@example.com")
        );
    }

    #[test]
    fn dkim_ar_unbalanced_comments_fail_closed() {
        assert!(strip_comments("a (b").is_none());
        assert!(strip_comments("a ) b").is_none());
        assert!(strip_comments("a \" b").is_none());
        assert!(parse_authentication_results("mx.google.com; dkim=pass (oops").is_none());
    }

    #[test]
    fn dkim_ambiguous_signatures_need_all_to_cover() {
        // Two signatures match the result; one covers List-Unsubscribe, the
        // other does not, so the group covers nothing.
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.d=example.com header.s=s1 header.b=AbCdEf12",
            ),
            (
                "DKIM-Signature",
                &sig("example.com", "s1", H_LU, "AbCdEf12zzzz"),
            ),
            (
                "DKIM-Signature",
                &sig("example.com", "s1", H_NONE, "AbCdEf12yyyy"),
            ),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
        ]);
        assert!(assess_auth(&h, "example.com").unsubscribe.is_none());
    }

    #[test]
    fn dkim_header_b_prefix_selects_signature() {
        let h = raw(&[
            (
                "Authentication-Results",
                "mx.google.com; dkim=pass header.d=example.com header.s=s1 header.b=AbCdEf12",
            ),
            (
                "DKIM-Signature",
                &sig("example.com", "s1", H_LU, "AbCdEf12zzzz"),
            ),
            (
                "DKIM-Signature",
                &sig("example.com", "s1", H_NONE, "ZZZZ9999yyyy"),
            ),
            ("List-Unsubscribe", "<https://u.example.com/x>"),
        ]);
        let opts = assess_auth(&h, "example.com").unsubscribe.expect("options");
        assert!(opts.https.is_some());
    }

    #[test]
    fn dkim_signature_duplicate_tag_invalid() {
        assert!(parse_dkim_signature(
            "v=1; d=example.com; d=example.net; s=s1; h=from; b=AbCdEf12"
        )
        .is_none());
        assert!(parse_dkim_signature("v=1; d=example.com; s=s1; h=from; b=AbCdEf12").is_some());
    }

    #[test]
    fn dkim_signature_i_outside_d_invalid() {
        let bad = "v=1; d=example.com; s=s1; i=@example.net; h=from; b=AbCdEf12";
        assert!(parse_dkim_signature(bad).is_none());
        let sub = "v=1; d=example.com; s=s1; i=@mail.example.com; h=from; b=AbCdEf12";
        let parsed = parse_dkim_signature(sub).expect("subdomain i is valid");
        assert_eq!(parsed.i_domain, "mail.example.com");
    }

    #[test]
    fn dkim_signature_missing_required_tag_invalid() {
        assert!(parse_dkim_signature("v=1; d=example.com; s=s1; h=from").is_none());
        assert!(parse_dkim_signature("v=2; d=example.com; s=s1; h=from; b=AbCdEf12").is_none());
        assert!(parse_dkim_signature("v=1; d=example.com; s=s1; h=to; b=AbCdEf12").is_none());
    }

    #[test]
    fn dkim_parsers_never_panic() {
        // A deterministic byte generator; the point is that random-ish input to
        // every parser returns rather than panics.
        let alphabet: &[u8] = b"ab; =\"()\\@.:<>-*01dkimspf";
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let len = (next() % 64) as usize;
            let s: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize] as char)
                .collect();
            let _ = strip_comments(&s);
            let _ = parse_authentication_results(&s);
            let _ = parse_dkim_signature(&s);
            let h = raw(&[
                ("Authentication-Results", s.as_str()),
                ("DKIM-Signature", s.as_str()),
                ("List-Unsubscribe", s.as_str()),
                ("List-Unsubscribe-Post", s.as_str()),
            ]);
            let a = assess_auth(&h, "example.com");
            let _ = (
                a.unsubscribe,
                a.list_unsubscribe_present,
                a.from_authenticated,
            );
        }
    }
}
