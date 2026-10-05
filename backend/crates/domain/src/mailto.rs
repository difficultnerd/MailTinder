//! Strict `mailto:` parsing (S6 section 6, ASVS V1.3.11).
//!
//! A `mailto:` List-Unsubscribe target names one recipient plus an optional
//! subject and body and nothing else; every other field, a duplicate, a control
//! character or bad percent encoding is refused. `MailtoTarget` itself and its
//! checked `new` come from T-101, so `parse` ends by calling `new` and those
//! checks stay the last line of defence.

use percent_encoding::percent_decode_str;

use crate::address::EmailAddress;
use crate::message::MailtoTarget;

/// The whole URI limit.
pub const MAILTO_MAX_CHARS: usize = 2048;
/// The subject limit `[DEFAULT]`.
pub const MAILTO_SUBJECT_MAX_CHARS: usize = 255;
/// The body limit `[DEFAULT]`.
pub const MAILTO_BODY_MAX_CHARS: usize = 2000;
/// The text sent for an absent subject or body `[DEFAULT]` (RFC 2369 practice).
pub const MAILTO_DEFAULT_TEXT: &str = "unsubscribe";

/// Why a `mailto:` target was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MailtoError {
    #[error("not a mailto uri")]
    NotMailto,
    #[error("too long")]
    TooLong,
    #[error("bad address")]
    Address,
    #[error("more than one recipient")]
    MultipleRecipients,
    #[error("header field not allowed")]
    FieldNotAllowed,
    #[error("duplicate field")]
    DuplicateField,
    #[error("bad percent encoding")]
    Encoding,
    #[error("control character")]
    ControlCharacter,
}

impl MailtoTarget {
    /// Parse a `mailto:` URI into a single-recipient target.
    pub fn parse(uri: &str) -> Result<MailtoTarget, MailtoError> {
        if uri.chars().count() > MAILTO_MAX_CHARS {
            return Err(MailtoError::TooLong);
        }
        let uri = strip_angle_brackets(uri);
        let Some(head) = uri.get(..7) else {
            return Err(MailtoError::NotMailto);
        };
        let Some(rest) = uri.get(7..) else {
            return Err(MailtoError::NotMailto);
        };
        if !head.eq_ignore_ascii_case("mailto:") {
            return Err(MailtoError::NotMailto);
        }

        let (path_raw, query_raw) = match rest.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (rest, None),
        };
        let path = percent_decode(path_raw).ok_or(MailtoError::Encoding)?;
        if path.contains(',') {
            return Err(MailtoError::MultipleRecipients);
        }
        if path.is_empty() {
            return Err(MailtoError::Address);
        }
        let to = EmailAddress::parse(&path).map_err(|_| MailtoError::Address)?;

        let mut subject: Option<String> = None;
        let mut body: Option<String> = None;
        if let Some(query) = query_raw {
            for piece in query.split('&').filter(|piece| !piece.is_empty()) {
                let Some((name_raw, value_raw)) = piece.split_once('=') else {
                    return Err(MailtoError::FieldNotAllowed);
                };
                let name = percent_decode(name_raw)
                    .ok_or(MailtoError::Encoding)?
                    .to_ascii_lowercase();
                let value = percent_decode(value_raw).ok_or(MailtoError::Encoding)?;
                match name.as_str() {
                    "subject" => {
                        if subject.is_some() {
                            return Err(MailtoError::DuplicateField);
                        }
                        subject = Some(value);
                    }
                    "body" => {
                        if body.is_some() {
                            return Err(MailtoError::DuplicateField);
                        }
                        body = Some(value);
                    }
                    _ => return Err(MailtoError::FieldNotAllowed),
                }
            }
        }

        for text in [&subject, &body].into_iter().flatten() {
            if text.chars().any(char::is_control) {
                return Err(MailtoError::ControlCharacter);
            }
        }
        if subject
            .as_ref()
            .is_some_and(|s| s.chars().count() > MAILTO_SUBJECT_MAX_CHARS)
        {
            return Err(MailtoError::TooLong);
        }
        if body
            .as_ref()
            .is_some_and(|b| b.chars().count() > MAILTO_BODY_MAX_CHARS)
        {
            return Err(MailtoError::TooLong);
        }

        MailtoTarget::new(to.as_str(), subject.as_deref(), body.as_deref())
            .map_err(|_| MailtoError::Address)
    }
}

/// Strip the surrounding `<` `>` of the List-Unsubscribe syntax, if present.
fn strip_angle_brackets(uri: &str) -> &str {
    if let Some(inner) = uri.strip_prefix('<') {
        if let Some(inner) = inner.strip_suffix('>') {
            return inner;
        }
    }
    uri
}

/// Strict percent decoding: every `%` must start two hex digits and the result
/// must be valid UTF-8. `+` is a literal plus (RFC 6068, not form encoding).
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            hex_value(bytes[i + 1])?;
            hex_value(bytes[i + 2])?;
            i += 3;
        } else {
            i += 1;
        }
    }
    percent_decode_str(input)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// The value of one hex digit.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn mailto_parse_simple_target() {
        let target = MailtoTarget::parse("mailto:User@Example.ORG").expect("valid");
        assert_eq!(target.to(), "User@example.org");
        assert_eq!(target.subject(), None);
        assert_eq!(target.body(), None);
    }

    #[test]
    fn mailto_percent_encoding_is_strict_utf8() {
        assert_eq!(
            MailtoTarget::parse("mailto:a@b.com?body=%FF").unwrap_err(),
            MailtoError::Encoding
        );
        assert_eq!(
            MailtoTarget::parse("mailto:a@b.com?body=%2").unwrap_err(),
            MailtoError::Encoding
        );
    }

    #[test]
    fn mailto_requires_exactly_one_equals_in_fields() {
        assert_eq!(
            MailtoTarget::parse("mailto:a@b.com?subject").unwrap_err(),
            MailtoError::FieldNotAllowed
        );
    }
}
