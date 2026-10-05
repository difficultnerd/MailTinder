//! A validated email address (S7 API-ADM-2).
//!
//! The local part is kept as given and the domain is stored lower case, so two
//! spellings of the same provider domain collapse to one key. Internationalised
//! local parts are refused `[DEFAULT]`; IDN domains arrive as `xn--` punycode.

use std::fmt;

/// The longest address kept (RFC 5321 forward-path limit).
pub const ADDRESS_MAX_CHARS: usize = 320;
/// The local part limit (RFC 5321).
const LOCAL_MAX_CHARS: usize = 64;
/// The domain limit (RFC 1035).
const DOMAIN_MAX_CHARS: usize = 253;
/// The DNS label limit.
const LABEL_MAX_CHARS: usize = 63;
/// The characters a dot-atom local part may hold besides letters and digits.
const LOCAL_SYMBOLS: &str = "!#$%&'*+-/=?^_`{|}~.";

/// A validated addr-spec. Debug prints `[address]` so a stray `{:?}` never logs
/// it (S5).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EmailAddress(String);

/// Why an address was refused. One variant: the caller learns only "invalid".
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddressError {
    #[error("address invalid")]
    Invalid,
}

impl EmailAddress {
    /// Parse and normalise an address.
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let trimmed = input.trim_matches(|c: char| c.is_ascii_whitespace());
        if trimmed.is_empty() || trimmed.chars().count() > ADDRESS_MAX_CHARS || !trimmed.is_ascii()
        {
            return Err(AddressError::Invalid);
        }
        let Some((local, domain)) = trimmed.rsplit_once('@') else {
            return Err(AddressError::Invalid);
        };
        // Exactly one `@`: the local part may not hold another.
        if local.contains('@') || !local_valid(local) || !domain_valid(domain) {
            return Err(AddressError::Invalid);
        }
        Ok(Self(format!("{local}@{}", domain.to_ascii_lowercase())))
    }

    /// The normalised address.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The lower-case domain.
    pub fn domain(&self) -> &str {
        self.0.rsplit_once('@').map_or("", |(_, domain)| domain)
    }

    /// Whole address lower-cased: the input to the email lookup HMAC
    /// (T-502b, T-505).
    pub fn lookup_form(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

// The spec mandates that Debug prints `[address]`, so the value is omitted.
impl fmt::Debug for EmailAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[address]")
    }
}

/// A dot-atom local part: 1..=64 characters, no leading, trailing or doubled
/// dot, no quoted form.
fn local_valid(local: &str) -> bool {
    if local.is_empty() || local.chars().count() > LOCAL_MAX_CHARS {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || LOCAL_SYMBOLS.contains(c))
}

/// An LDH domain: at least two labels, each 1..=63, no edge hyphen, and the
/// last label not all digits (so an IPv4-looking literal is refused). IP
/// literals in brackets fail the character check.
fn domain_valid(domain: &str) -> bool {
    if domain.is_empty() || domain.chars().count() > DOMAIN_MAX_CHARS {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    if labels.iter().any(|label| {
        label.is_empty()
            || label.chars().count() > LABEL_MAX_CHARS
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        return false;
    }
    let last = labels[labels.len() - 1];
    !last.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn email_address_trim_accepts_surrounding_whitespace() {
        let address = EmailAddress::parse("  A@Example.COM \t").expect("valid");
        assert_eq!(address.as_str(), "A@example.com");
    }

    #[test]
    fn email_address_rejects_bad_shapes() {
        for bad in [
            "",
            "no-at-sign",
            "a@@example.com",
            "@example.com",
            "a@",
            "a@localhost",
            "a@example",
            ".a@example.com",
            "a.@example.com",
            "a..b@example.com",
            "a@-example.com",
            "a@example-.com",
            "a@exa mple.com",
        ] {
            assert!(EmailAddress::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
