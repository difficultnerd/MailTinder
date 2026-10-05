//! Raw headers, `From` parsing and the header-to-[`HeaderFacts`] builder.
//!
//! Header order and duplicates are kept exactly as Gmail returned them: T-406
//! relies on the first `Authentication-Results` being Gmail's own and on
//! counting `List-Unsubscribe` instances.

use domain::text::sanitise_plain;
use domain::HeaderFacts;

use crate::auth_results::assess_auth;

/// The headers requested for `format=metadata`, in this order.
pub const METADATA_HEADERS: [&str; 15] = [
    "From",
    "Subject",
    "Date",
    "List-Unsubscribe",
    "List-Unsubscribe-Post",
    "List-Id",
    "Feedback-ID",
    "Precedence",
    "Auto-Submitted",
    "Authentication-Results",
    "DKIM-Signature",
    "In-Reply-To",
    "References",
    "Reply-To",
    "Return-Path",
];

/// Provider hints for the bulk badge reason only, never a decision input
/// `[DEFAULT]`.
pub const ESP_HINTS: [(&str, &str); 8] = [
    ("mcsv.net", "mailchimp"),
    ("list-manage.com", "mailchimp"),
    ("sendgrid.net", "sendgrid"),
    ("mailgun.org", "mailgun"),
    ("amazonses.com", "amazon_ses"),
    ("exacttarget.com", "salesforce"),
    ("hubspotemail.net", "hubspot"),
    ("constantcontact.com", "constant_contact"),
];

/// The longest `From` address kept, in characters.
const ADDRESS_MAX_CHARS: usize = 320;
/// The longest normalised `List-Id` or `Feedback-ID`, in characters.
const KEY_MAX_CHARS: usize = 255;

/// Header name and value in message order, exactly as Gmail returned them.
pub struct RawHeaders(pub Vec<(String, String)>);

impl RawHeaders {
    /// Every value for `name`, case-insensitive, in message order.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.0
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The first value for `name`, case-insensitive, or `None`.
    pub fn first(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// How many headers are named `name`, case-insensitive.
    pub fn count(&self, name: &str) -> usize {
        self.0
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .count()
    }
}

/// A parsed `From` header.
#[derive(Clone, PartialEq, Eq)]
pub struct ParsedFrom {
    pub display: String,
    pub address: String,
}

/// Parse a `From` value, taking the first single address (or a group's first
/// member). `None` when there is no usable address.
pub fn parse_from(value: &str) -> Option<ParsedFrom> {
    let list = mailparse::addrparse(value).ok()?;
    let single = list.iter().find_map(|addr| match addr {
        mailparse::MailAddr::Single(s) => Some(s),
        mailparse::MailAddr::Group(g) => g.addrs.first(),
    })?;
    let address = single.addr.trim().to_owned();
    if address.is_empty() || address.chars().count() > ADDRESS_MAX_CHARS {
        return None;
    }
    let display = decode_header_text(single.display_name.as_deref().unwrap_or(""));
    Some(ParsedFrom { display, address })
}

/// Decode an RFC 2047 encoded word if present, then make it safe plain text
/// (T-402): control, bidirectional and zero-width characters removed.
pub fn decode_header_text(value: &str) -> String {
    let decoded = if value.contains("=?") && value.contains("?=") {
        rfc2047_decoder::decode(value.as_bytes()).unwrap_or_else(|_| value.to_owned())
    } else {
        value.to_owned()
    };
    // No length cut here: the caller bounds the field (subject 998, name 256).
    sanitise_plain(&decoded, usize::MAX)
}

/// Build the [`HeaderFacts`] a card needs from the raw headers.
pub fn build_header_facts(h: &RawHeaders, from: &ParsedFrom) -> HeaderFacts {
    let list_id = h.first("List-Id").and_then(normalise_list_id);
    let feedback_id = h
        .first("Feedback-ID")
        .map(|v| truncate(v.trim(), KEY_MAX_CHARS))
        .filter(|s| !s.is_empty());
    let precedence_bulk = h.first("Precedence").is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "bulk" | "list" | "junk"
        )
    });
    let auto_submitted = h
        .first("Auto-Submitted")
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("no"));
    let is_reply_or_thread = h.first("In-Reply-To").is_some() || h.first("References").is_some();
    let esp_hint = esp_hint(h);
    let from_domain = from
        .address
        .rsplit_once('@')
        .map(|(_, d)| d.to_ascii_lowercase())
        .unwrap_or_default();
    let auth = assess_auth(h, &from_domain);
    HeaderFacts {
        list_unsubscribe: auth.unsubscribe,
        list_unsubscribe_present: auth.list_unsubscribe_present,
        list_id,
        feedback_id,
        precedence_bulk,
        auto_submitted,
        from_authenticated: auth.from_authenticated,
        esp_hint,
        is_reply_or_thread,
        reply_to_mismatch: false,
        display_name_spoof: false,
    }
}

/// The text inside the last `<...>` of a `List-Id`, lower-cased and bounded.
fn normalise_list_id(value: &str) -> Option<String> {
    let inner = match (value.rfind('<'), value.rfind('>')) {
        (Some(open), Some(close)) if open < close => &value[open + 1..close],
        _ => value,
    };
    let normalised = inner.trim().to_ascii_lowercase();
    (!normalised.is_empty()).then(|| truncate(&normalised, KEY_MAX_CHARS))
}

/// The first matching ESP hint from `List-Id`, `Feedback-ID` or `Return-Path`.
fn esp_hint(h: &RawHeaders) -> Option<String> {
    let mut haystacks: Vec<String> = Vec::new();
    for name in ["List-Id", "Feedback-ID", "Return-Path"] {
        for value in h.all(name) {
            haystacks.push(value.to_ascii_lowercase());
        }
    }
    ESP_HINTS
        .iter()
        .find(|(needle, _)| haystacks.iter().any(|s| s.contains(needle)))
        .map(|(_, key)| (*key).to_owned())
}

/// The first `max` characters of `s`.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]

    use super::*;

    fn raw(pairs: &[(&str, &str)]) -> RawHeaders {
        RawHeaders(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    fn from(address: &str) -> ParsedFrom {
        ParsedFrom {
            display: String::new(),
            address: address.to_owned(),
        }
    }

    #[test]
    fn gmail_from_parse_display_and_address() {
        let cases = [
            (
                "Sender Name <a@example.com>",
                Some(("Sender Name", "a@example.com")),
            ),
            ("a@example.com", Some(("", "a@example.com"))),
            ("<b@example.com>", Some(("", "b@example.com"))),
            (
                "\"Quoted, Name\" <c@example.com>",
                Some(("Quoted, Name", "c@example.com")),
            ),
            ("undisclosed-recipients:;", None),
            ("", None),
        ];
        for (input, expected) in cases {
            let got = parse_from(input).map(|p| (p.display, p.address));
            match expected {
                Some((d, a)) => {
                    let (gd, ga) = got.unwrap_or_else(|| panic!("no parse for {input}"));
                    assert_eq!((gd.as_str(), ga.as_str()), (d, a), "input {input}");
                }
                None => assert!(got.is_none(), "input {input} should not parse"),
            }
        }
    }

    #[test]
    fn gmail_from_parse_group_takes_first_member() {
        let parsed = parse_from("Team: First <f@example.com>, Second <s@example.com>;")
            .expect("group parses");
        assert_eq!(parsed.address, "f@example.com");
    }

    #[test]
    fn gmail_from_parse_decodes_encoded_word_display() {
        let parsed = parse_from("=?utf-8?q?Ren=C3=A9?= <r@example.com>").expect("parses");
        assert_eq!(parsed.display, "René");
        assert_eq!(parsed.address, "r@example.com");
    }

    #[test]
    fn raw_headers_preserve_message_order_and_duplicates() {
        let h = raw(&[
            ("DKIM-Signature", "one"),
            ("Authentication-Results", "a"),
            ("DKIM-Signature", "two"),
            ("Authentication-Results", "b"),
        ]);
        assert_eq!(
            h.all("dkim-signature").collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(
            h.all("Authentication-Results").collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(h.count("DKIM-Signature"), 2);
        assert_eq!(h.first("dkim-signature"), Some("one"));
    }

    /// The precedence table: only bulk, list and junk count as bulk.
    fn cases() -> Vec<(&'static [(&'static str, &'static str)], bool, bool, bool)> {
        vec![
            (&[("Precedence", "bulk")], true, false, false),
            (&[("Precedence", "List")], true, false, false),
            (&[("Precedence", "junk")], true, false, false),
            (&[("Precedence", "first-class")], false, false, false),
        ]
    }

    #[test]
    fn gmail_facts_precedence_auto_submitted_reply() {
        let f = from("sender@example.com");
        for (headers, bulk, auto, reply) in cases() {
            let facts = build_header_facts(&raw(headers), &f);
            assert_eq!(facts.precedence_bulk, bulk, "{headers:?}");
            assert_eq!(facts.auto_submitted, auto, "{headers:?}");
            assert_eq!(facts.is_reply_or_thread, reply, "{headers:?}");
        }
        let facts = build_header_facts(&raw(&[("Auto-Submitted", "auto-replied")]), &f);
        assert!(facts.auto_submitted);
        let facts = build_header_facts(&raw(&[("Auto-Submitted", "no")]), &f);
        assert!(!facts.auto_submitted);
        let facts = build_header_facts(&raw(&[("References", "<x@y>")]), &f);
        assert!(facts.is_reply_or_thread);
    }

    #[test]
    fn gmail_facts_list_id_feedback_id_and_esp_hint() {
        let f = from("news@example.com");
        let facts = build_header_facts(
            &raw(&[
                ("List-Id", "Newsletter <List.Example.COM>"),
                ("Feedback-ID", " 123:campaign "),
                ("Return-Path", "<bounce@sendgrid.net>"),
            ]),
            &f,
        );
        assert_eq!(facts.list_id.as_deref(), Some("list.example.com"));
        assert_eq!(facts.feedback_id.as_deref(), Some("123:campaign"));
        assert_eq!(facts.esp_hint.as_deref(), Some("sendgrid"));
        assert!(!facts.list_unsubscribe_present);
        assert!(facts.list_unsubscribe.is_none());
        assert!(!facts.from_authenticated);
    }

    #[test]
    fn gmail_facts_list_unsubscribe_presence_counts_duplicates() {
        let f = from("news@example.com");
        let facts = build_header_facts(
            &raw(&[
                ("List-Unsubscribe", "<https://x.example/u>"),
                ("List-Unsubscribe", "<mailto:u@example.com>"),
            ]),
            &f,
        );
        assert!(facts.list_unsubscribe_present);
    }
}
