//! Client IP from the trusted `X-Forwarded-For` position.

use std::net::IpAddr;

use axum::http::HeaderMap;

/// The client IP, or `None` if it could not be determined from the trusted
/// position. `None` is limited as one shared "unknown" bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

/// Split `X-Forwarded-For` on `,`, trim, and take the entry `trusted_hops`
/// positions from the right (1 = the rightmost, the address Google's front end
/// saw). Never use the leftmost entry or `X-Real-IP` (client controlled).
pub fn client_ip(headers: &HeaderMap, trusted_hops: usize) -> ClientIp {
    let Some(value) = headers.get("x-forwarded-for") else {
        return ClientIp(None);
    };
    let Ok(value) = value.to_str() else {
        return ClientIp(None);
    };
    let entries: Vec<&str> = value.split(',').map(str::trim).collect();
    if entries.is_empty() {
        return ClientIp(None);
    }
    if trusted_hops == 0 || entries.len() < trusted_hops {
        return ClientIp(None);
    }
    let idx = entries.len() - trusted_hops;
    let candidate = entries.get(idx).copied().unwrap_or_default();
    match candidate.parse::<IpAddr>() {
        Ok(ip) => ClientIp(Some(ip)),
        Err(_) => ClientIp(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(s: &str) -> Result<HeaderMap, axum::http::header::InvalidHeaderValue> {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", s.parse()?);
        Ok(h)
    }

    #[test]
    fn asvs_v4_1_3_leftmost_forwarded_for_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let h = headers("203.0.113.9, 198.51.100.7")?;
        assert_eq!(client_ip(&h, 1), ClientIp(Some("198.51.100.7".parse()?)));
        Ok(())
    }

    #[test]
    fn missing_header_gives_none() {
        assert_eq!(client_ip(&HeaderMap::new(), 1), ClientIp(None));
    }

    #[test]
    fn unparseable_gives_none() -> Result<(), Box<dyn std::error::Error>> {
        let h = headers("not-an-ip")?;
        assert_eq!(client_ip(&h, 1), ClientIp(None));
        Ok(())
    }
}
