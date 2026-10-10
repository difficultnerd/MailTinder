//! In-memory model input minimisation. No message content is logged or stored.
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use whatlang::Lang;

pub const INPUT_VERSION: &str = "1";
pub const MAX_TEXT_WORDS: usize = 500;
pub const MAX_TEXT_CHARS: usize = 3000;
pub const MODEL_TEXT_FETCH_CHARS: usize = 4000;
pub const URL_PLACEHOLDER: &str = "[url]";
pub const EMAIL_PLACEHOLDER: &str = "[email]";
pub const NUMBER_PLACEHOLDER: &str = "[number]";

/// The shared persisted bucket type, also re-exported by `ports`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextTokensBucket {
    #[serde(rename = "<100")]
    Under100,
    #[serde(rename = "100-300")]
    T100To300,
    #[serde(rename = ">300")]
    Over300,
}

pub struct InputFacts {
    pub text_tokens_bucket: TextTokensBucket,
    pub lang_is_english: bool,
}

struct Patterns {
    scheme: Regex,
    www: Regex,
    host: Regex,
    email_quoted: Regex,
    email_literal: Regex,
    email_at: Regex,
    email: Regex,
    number: Regex,
}

impl Patterns {
    fn compile() -> Result<Self, regex::Error> {
        Ok(Self {
            scheme: Regex::new(
                r#"(?i)(?:https?|ftp|mailto|javascript|data|file|tel):[^\s<>"'()\[\]{}]+"#,
            )?,
            www: Regex::new(r#"(?i)www\.[^\s<>"'()\[\]{}]+"#)?,
            host: Regex::new(r"(?i)\b[a-z0-9-]+(\.[a-z0-9-]+)+/\S*")?,
            // Quoted local parts (which may contain spaces), address literals
            // and any remaining `@` flanked by non-whitespace. The regex crate
            // is linear-time, so no upper bound is needed.
            email_quoted: Regex::new(r#""[^"]*"@\S+"#)?,
            email_literal: Regex::new(r"\S*@\[[^\]]+\]")?,
            email_at: Regex::new(r"\S*@\S+")?,
            email: Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)+")?,
            number: Regex::new(r"\d(?:[ \-.]?\d){5,}")?,
        })
    }
}

/// Remove invisible characters, then replace URLs, addresses and long numbers.
/// Normalising whitespace before matching also prevents a second redaction
/// from discovering a spaced number that the final whitespace collapse creates.
pub fn redact(input: &str) -> String {
    static PATTERNS: OnceLock<Result<Patterns, regex::Error>> = OnceLock::new();
    let Ok(patterns) = PATTERNS.get_or_init(Patterns::compile) else {
        tracing::error!(event = "redact_regex_error");
        return String::new();
    };
    let clean: String = input
        .chars()
        .filter_map(|c| match c {
            '\n' | '\t' => Some(' '),
            '\u{0}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{61c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => None,
            _ => Some(c),
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = patterns.scheme.replace_all(&clean, URL_PLACEHOLDER);
    let clean = patterns.www.replace_all(&clean, URL_PLACEHOLDER);
    let clean = patterns.host.replace_all(&clean, URL_PLACEHOLDER);
    let clean = patterns.email_quoted.replace_all(&clean, EMAIL_PLACEHOLDER);
    let clean = patterns
        .email_literal
        .replace_all(&clean, EMAIL_PLACEHOLDER);
    let clean = patterns.email_at.replace_all(&clean, EMAIL_PLACEHOLDER);
    let clean = patterns.email.replace_all(&clean, EMAIL_PLACEHOLDER);
    let clean = patterns.number.replace_all(&clean, NUMBER_PLACEHOLDER);
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cut words first, then Unicode characters (never splitting UTF-8).
pub fn truncate_words(input: &str) -> String {
    input
        .split_whitespace()
        .take(MAX_TEXT_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect()
}

pub fn word_count(input: &str) -> usize {
    input.split_whitespace().count()
}

/// Ceiling of Unicode character count divided by four, saturating at `u32`.
pub fn approx_tokens(rendered: &str) -> u32 {
    u32::try_from(rendered.chars().count().div_ceil(4)).unwrap_or(u32::MAX)
}

pub fn text_tokens_bucket(words: usize) -> TextTokensBucket {
    match words {
        0..=99 => TextTokensBucket::Under100,
        100..=300 => TextTokensBucket::T100To300,
        _ => TextTokensBucket::Over300,
    }
}

pub fn is_english(input: &str) -> bool {
    whatlang::detect(input).is_some_and(|info| info.lang() == Lang::Eng && info.is_reliable())
}

/// Only the domain is allowed; never return a sender's local part.
///
/// Fail closed: the whole input must be whitespace- and control-free and the
/// part after the last `@` must be a syntactically valid hostname, otherwise
/// the empty string is returned (no address or domain is logged).
pub fn from_domain(address: &str) -> String {
    if address.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return String::new();
    }
    let Some((_, domain)) = address.rsplit_once('@') else {
        return String::new();
    };
    let domain = domain.to_lowercase();
    if is_valid_hostname(&domain) {
        domain
    } else {
        String::new()
    }
}

/// 1 to 253 characters, dot-separated labels of 1 to 63 ASCII letters, digits
/// or hyphens, none starting or ending with a hyphen, at least one dot.
fn is_valid_hostname(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 || !domain.contains('.') {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn exp_2_regexes_compile() -> Result<(), regex::Error> {
        super::Patterns::compile()?;
        Ok(())
    }
}
