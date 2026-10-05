//! One-click URL and host checks (pure; run before any DNS).

use url::{Host, Url};

use ports::EgressError;

/// The only port a one-click target may use.
pub const ONE_CLICK_PORT: u16 = 443;
/// The longest one-click URL accepted.
pub const MAX_URL_LEN: usize = 2048;
/// Host names refused regardless of how they resolve.
pub const REFUSED_NAMES: [&str; 3] = ["localhost", "metadata.google.internal", "metadata"];
/// Host name suffixes refused regardless of how they resolve.
pub const REFUSED_SUFFIXES: [&str; 6] = [
    ".localhost",
    ".internal",
    ".local",
    ".localdomain",
    ".home.arpa",
    ".lan",
];

/// Checks a one-click URL before any DNS: returns the lower-cased host without
/// a trailing dot.
///
/// # Errors
///
/// Returns `EgressError` for any URL that is not an https, credential-free,
/// port-443 request to a globally routable domain host.
pub fn check_one_click_url(url: &Url) -> Result<String, EgressError> {
    if url.as_str().len() > MAX_URL_LEN {
        return Err(EgressError::HostNotAllowed);
    }
    if url.scheme() != "https" {
        return Err(EgressError::SchemeNotAllowed);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(EgressError::CredentialsInUrl);
    }
    match url.port() {
        None | Some(ONE_CLICK_PORT) => {}
        Some(_) => return Err(EgressError::PortNotAllowed),
    }
    match url.host() {
        Some(Host::Ipv4(_) | Host::Ipv6(_)) => Err(EgressError::IpLiteralHost),
        Some(Host::Domain(d)) => {
            let lower = d.to_ascii_lowercase();
            let stripped = lower.strip_suffix('.').unwrap_or(&lower);
            check_host_name(stripped)?;
            Ok(stripped.to_owned())
        }
        None => Err(EgressError::HostNotAllowed),
    }
}

/// Refuses reserved and single-label host names.
pub fn check_host_name(name: &str) -> Result<(), EgressError> {
    for reserved in REFUSED_NAMES {
        if name == reserved {
            return Err(EgressError::HostNotAllowed);
        }
    }
    for suffix in REFUSED_SUFFIXES {
        if name.ends_with(suffix) {
            return Err(EgressError::HostNotAllowed);
        }
    }
    if !name.contains('.') {
        return Err(EgressError::HostNotAllowed);
    }
    Ok(())
}
