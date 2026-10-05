//! Security and caching headers on every response, including errors.

use axum::http::HeaderValue;
use axum::response::Response;

/// Apply the security and anti-caching headers to a response, overwriting any
/// existing value (never appended twice). Also strips any `Access-Control-*`
/// header a handler might have added (no CORS is ever sent).
pub fn apply_security_headers(resp: &mut Response) {
    let headers = resp.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert("pragma", HeaderValue::from_static("no-cache"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    // Remove any CORS headers a handler might have set.
    let cors_keys: Vec<_> = headers
        .keys()
        .filter(|k| {
            k.as_str()
                .to_ascii_lowercase()
                .starts_with("access-control-")
        })
        .cloned()
        .collect();
    for k in cors_keys {
        headers.remove(k);
    }
}
