//! Synthetic mail corpus and fixture loader (T-204).
//!
//! Loads [`manifest.toml`](crate::manifest) (embedded via `include_str!`), turns
//! each case into RFC 5322 `.eml` bytes plus a spread of plain headers, and
//! exposes the expected outcome and canary tokens used by the leak tests (S10
//! section 5 and 7.3). Agents add cases by editing the TOML, never by
//! hand-writing MIME.

use domain::{HeaderFacts, MailtoTarget, MessageClass, UnsubscribeOptions};
use serde::Deserialize;
use time::{format_description::well_known::Rfc2822, OffsetDateTime};
use url::Url;

use crate::mailbox::state::SeedMessage;

pub mod build;

pub const MANIFEST: &str = include_str!("../../fixtures/mail/manifest.toml");

/// S10 section 5 rule and T-204's only permitted suffixes (RFC 2606).
pub const RESERVED_DOMAIN_SUFFIXES: [&str; 5] = [
    "example.com",
    "example.net",
    "example.org",
    ".test",
    ".invalid",
];

/// DKIM coverage scenario; drives the `h=` list and the `Authentication-Results`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DkimScenario {
    /// One-click: `h=` has `List-Unsubscribe` and `List-Unsubscribe-Post`; AR `dkim=pass`.
    PassCoversBoth,
    /// `h=` has `List-Unsubscribe` only; AR `dkim=pass`.
    PassCoversListUnsub,
    /// Signature passes but `h=` lacks the unsubscribe headers.
    PassNotCovering,
    /// `d=` is the ESP domain (`example.net`), pass; `dmarc=fail` for the From domain.
    EspPassDmarcFail,
    /// AR `dkim=fail`.
    Fail,
    /// No `DKIM-Signature`; AR `dkim=none`.
    Unsigned,
    /// Trusted AR says `dkim=none`; a second, lower AR from the sender claims pass.
    ForgedLowerAr,
}

/// Expected unsubscribe method for a corpus case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedMethod {
    OneClick,
    Mailto,
    NeedsAttentionHttps,
    NeedsAttentionHttp,
    None,
}

/// The expected classification outcome for a case.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Expected {
    pub class: MessageClass,
    pub bulk_score_min: u8,
    pub bulk_score_max: u8,
    pub method: ExpectedMethod,
    /// Short reason words, e.g. "one-click list".
    pub reason: String,
    pub from_authenticated: bool,
}

/// A single corpus case as written in the manifest.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseSpec {
    /// kebab-case, unique.
    pub id: String,
    /// S10 5 "Source" column, e.g. "E1", "SR-01 AC3".
    pub source: String,
    pub from_display: String,
    /// Reserved domains only.
    pub from_address: String,
    pub reply_to: Option<String>,
    pub subject: String,
    pub list_id: Option<String>,
    /// Full header value, e.g. `"<https://u.example.com/x>, <mailto:u@example.com?subject=stop>"`.
    pub list_unsubscribe: Option<String>,
    /// Adds `List-Unsubscribe-Post: List-Unsubscribe=One-Click`.
    #[serde(default)]
    pub one_click_post: bool,
    pub feedback_id: Option<String>,
    pub precedence: Option<String>,
    pub auto_submitted: Option<String>,
    /// e.g. ESP fingerprint headers.
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    /// Defaults to `Unsigned` (no signature, AR `dkim=none`).
    #[serde(default = "unsigned_dkim")]
    pub dkim: DkimScenario,
    /// Defaults to the From domain.
    pub dkim_domain: Option<String>,
    pub body_text: String,
    pub body_html: Option<String>,
    /// Adds `<img src="https://img.example.com/p.gif">` to the HTML part.
    #[serde(default)]
    pub remote_images: bool,
    /// Repeat `body_text` N times (default 1); hostile case.
    #[serde(default = "one")]
    pub body_repeat: u32,
    /// Adds `X-Pad` headers to reach this size; hostile 1 MB case.
    #[serde(default)]
    pub header_padding_bytes: u32,
    /// RFC 3339.
    pub date: String,
    pub labels: Vec<String>,
    pub expected: Expected,
}

fn one() -> u32 {
    1
}

fn unsigned_dkim() -> DkimScenario {
    DkimScenario::Unsigned
}

/// A fully built corpus case: the spec plus the `.eml` bytes and plain headers.
#[derive(Clone, Debug)]
pub struct CorpusCase {
    pub spec: CaseSpec,
    pub eml: Vec<u8>,
    /// The headers in message order (top first), used by `FakeMailbox`.
    pub headers: Vec<(String, String)>,
}

impl CorpusCase {
    /// Build the `FakeMailbox` seed. `list_unsubscribe` is `Some` only for
    /// `OneClick` or `Mailto` outcomes (DKIM-covered). Facts come from `expected`
    /// and the spec headers.
    pub fn seed_message(&self) -> SeedMessage {
        let s = &self.spec;
        let from_addr = s.from_address.clone();
        let from_domain = from_addr
            .rsplit_once('@')
            .map(|(_, d)| d.to_lowercase())
            .unwrap_or_default();
        let unsubscribe = self.expected_unsubscribe();
        let lu_present = s
            .list_unsubscribe
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty());
        let reply_to_domain = s
            .reply_to
            .as_deref()
            .and_then(|v| v.rsplit_once('@').map(|(_, d)| d.to_lowercase()));
        let display_name_spoof = has_domain(&s.from_display, &from_domain);

        let facts = HeaderFacts {
            list_unsubscribe: unsubscribe,
            list_unsubscribe_present: lu_present,
            list_id: s.list_id.as_deref().map(normalise_list_id),
            feedback_id: s.feedback_id.clone(),
            precedence_bulk: s
                .precedence
                .as_deref()
                .is_some_and(|p| matches!(p.to_lowercase().as_str(), "bulk" | "list" | "junk")),
            auto_submitted: s
                .auto_submitted
                .as_deref()
                .is_some_and(|v| v.to_lowercase() != "no"),
            from_authenticated: s.expected.from_authenticated,
            esp_hint: None,
            is_reply_or_thread: false,
            reply_to_mismatch: reply_to_domain.is_some() && reply_to_domain != Some(from_domain),
            display_name_spoof,
        };

        let date = parse_rfc3339(&s.date).unwrap_or(T0);
        SeedMessage {
            from_display: canary(s, "display", &s.from_display),
            from_address: from_addr,
            subject: canary(s, "subject", &s.subject),
            raw_headers: self.headers.clone(),
            facts,
            preview_text: format!("CANARY-{}-body", s.id),
            internal_date: date,
            labels: s.labels.clone(),
        }
    }

    fn expected_unsubscribe(&self) -> Option<UnsubscribeOptions> {
        let s = &self.spec;
        let lu = s.list_unsubscribe.as_deref()?;
        let mut out = UnsubscribeOptions::default();
        let (https, mailto) = parse_lu_uris(lu);
        match s.expected.method {
            ExpectedMethod::OneClick => out.one_click_https = https,
            ExpectedMethod::Mailto => out.mailto = mailto,
            ExpectedMethod::NeedsAttentionHttps => out.https = https,
            ExpectedMethod::NeedsAttentionHttp | ExpectedMethod::None => return None,
        }
        if out.one_click_https.is_none() && out.https.is_none() && out.mailto.is_none() {
            None
        } else {
            Some(out)
        }
    }

    /// Every canary planted in this case (display, subject, body).
    pub fn canaries(&self) -> Vec<String> {
        vec![
            canary(&self.spec, "display", &self.spec.from_display),
            canary(&self.spec, "subject", &self.spec.subject),
            format!("CANARY-{}-body", self.spec.id),
        ]
    }
}

/// The loaded corpus.
#[derive(Clone, Debug, Default)]
pub struct Corpus {
    pub cases: Vec<CorpusCase>,
}

impl Corpus {
    pub fn case(&self, id: &str) -> Option<&CorpusCase> {
        self.cases.iter().find(|c| c.spec.id == id)
    }

    pub fn all_canaries(&self) -> Vec<String> {
        self.cases.iter().flat_map(CorpusCase::canaries).collect()
    }

    pub fn all_addresses(&self) -> Vec<String> {
        let mut out = Vec::new();
        for c in &self.cases {
            out.push(c.spec.from_address.clone());
            if let Some(r) = &c.spec.reply_to {
                out.push(r.clone());
            }
        }
        out
    }

    pub fn all_urls(&self) -> Vec<String> {
        let mut out = Vec::new();
        for c in &self.cases {
            if let Some(lu) = &c.spec.list_unsubscribe {
                for uri in lu.split(',').filter_map(|t| {
                    let t = t.trim();
                    t.strip_prefix('<').and_then(|t| t.strip_suffix('>'))
                }) {
                    out.push(uri.to_owned());
                }
            }
            if c.spec.remote_images {
                out.push("https://img.example.com/p.gif".to_owned());
            }
        }
        out
    }
}

/// Parse the manifest and build every case. Rejects duplicate IDs.
pub fn load() -> Result<Corpus, String> {
    #[derive(Deserialize)]
    struct Manifest {
        // `[[case]]` maps to a field named `case`.
        case: Vec<CaseSpec>,
    }
    let m: Manifest = toml::from_str(MANIFEST).map_err(|e| format!("manifest: {e}"))?;
    let mut seen = std::collections::HashSet::new();
    let mut cases = Vec::new();
    for spec in m.case {
        if !seen.insert(spec.id.clone()) {
            return Err(format!("duplicate case id: {}", spec.id));
        }
        let (eml, headers) = build::build_case(&spec);
        cases.push(CorpusCase { spec, eml, headers });
    }
    Ok(Corpus { cases })
}

fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
}

/// RFC 5322 date from an RFC 3339 string; falls back to `T0` on parse failure.
pub(crate) fn rfc5322_date(spec: &CaseSpec) -> String {
    parse_rfc3339(&spec.date)
        .ok()
        .and_then(|dt| dt.format(&Rfc2822).ok())
        .unwrap_or_else(|| "Tue, 1 Jan 2026 00:00:00 +0000".to_owned())
}

/// The canary token for a field.
pub(crate) fn canary(spec: &CaseSpec, field: &str, value: &str) -> String {
    format!("{value} CANARY-{}-{field}", spec.id)
}

fn normalise_list_id(v: &str) -> String {
    let v = v.trim();
    match (v.starts_with('<'), v.ends_with('>')) {
        (true, true) => v[1..v.len() - 1].to_lowercase(),
        _ => v.to_lowercase(),
    }
}

fn has_domain(display: &str, domain: &str) -> bool {
    let d = domain.to_lowercase();
    display.to_lowercase().contains(&d)
}

/// Parse the first `https` and `mailto` URI out of a `List-Unsubscribe` value.
fn parse_lu_uris(v: &str) -> (Option<Url>, Option<MailtoTarget>) {
    let mut https = None;
    let mut mailto = None;
    for tok in v.split(',') {
        let tok = tok.trim();
        let inner = tok
            .strip_prefix('<')
            .and_then(|t| t.strip_suffix('>'))
            .map_or_else(|| tok.to_owned(), str::to_owned);
        if https.is_none() && inner.starts_with("https://") {
            if let Ok(u) = Url::parse(&inner) {
                if u.has_host() && u.scheme() == "https" {
                    https = Some(u);
                }
            }
        } else if mailto.is_none() && inner.starts_with("mailto:") {
            let target = &inner["mailto:".len()..];
            let (to, subj) = match target.split_once('?') {
                Some((t, q)) => {
                    let s = q.split('&').find_map(|p| {
                        let (k, v) = p.split_once('=').unwrap_or((p, ""));
                        (k == "subject").then(|| v.to_owned())
                    });
                    (t, s)
                }
                None => (target, None),
            };
            if let Ok(t) = MailtoTarget::new(to, subj.as_deref(), None) {
                mailto = Some(t);
            }
        }
    }
    (https, mailto)
}

pub(crate) const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;
