//! Gmail error bodies to [`MailError`] mapping, shared by every M4 task.
//!
//! Provider error text never reaches a `MailError` value (ASVS V16.5.1): the
//! `Invalid` reason is a fixed string chosen here, never Gmail's `message`.

use ports::MailError;
use time::format_description::well_known::Rfc2822;
use time::OffsetDateTime;

/// The `Retry-After` used when Gmail sends none `[DEFAULT]`.
pub const DEFAULT_RETRY_AFTER_S: u64 = 10;
/// The upper clamp so a bad header cannot park a mailbox for hours `[DEFAULT]`.
pub const MAX_RETRY_AFTER_S: u64 = 300;

/// The `403` reasons that mean "slow down", not "not allowed".
const RATE_REASONS: [&str; 4] = [
    "rateLimitExceeded",
    "userRateLimitExceeded",
    "dailyLimitExceeded",
    "quotaExceeded",
];

/// Map a Gmail HTTP failure to a [`MailError`].
pub fn map_gmail_error(
    status: u16,
    retry_after: Option<&str>,
    body: &[u8],
    now: OffsetDateTime,
) -> MailError {
    let reasons = parse_reasons(body);
    match status {
        401 => MailError::Unauthorized,
        403 => {
            if reasons.iter().any(|r| RATE_REASONS.contains(&r.as_str())) {
                MailError::RateLimited {
                    retry_after_s: retry_after_seconds(retry_after, now),
                }
            } else {
                MailError::Forbidden
            }
        }
        429 => MailError::RateLimited {
            retry_after_s: retry_after_seconds(retry_after, now),
        },
        404 | 410 => MailError::NotFound,
        400 => MailError::Invalid("gmail_bad_request".to_owned()),
        409 => MailError::Invalid("gmail_conflict".to_owned()),
        412 => MailError::Invalid("gmail_precondition".to_owned()),
        _ => MailError::Transient,
    }
}

/// The `retry_after_s` for a rate-limited response, clamped to `1..=MAX`.
pub fn retry_after_seconds(header: Option<&str>, now: OffsetDateTime) -> u64 {
    let seconds = header
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok().or_else(|| http_date_seconds(s, now)))
        .unwrap_or(DEFAULT_RETRY_AFTER_S);
    seconds.clamp(1, MAX_RETRY_AFTER_S)
}

/// Parse an HTTP-date `Retry-After` into seconds from `now`, or `None`.
fn http_date_seconds(value: &str, now: OffsetDateTime) -> Option<u64> {
    let normalised = match value.rsplit_once(' ') {
        Some((head, "GMT" | "UT")) => format!("{head} +0000"),
        _ => value.to_owned(),
    };
    let parsed = OffsetDateTime::parse(&normalised, &Rfc2822).ok()?;
    let diff = (parsed - now).whole_seconds();
    u64::try_from(diff.max(0)).ok()
}

/// The `errors[].reason` strings from a Gmail error body; empty when unparsable.
fn parse_reasons(body: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    value
        .get("error")
        .and_then(|e| e.get("errors"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("reason").and_then(|r| r.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use time::macros::datetime;

    fn body(reason: &str) -> Vec<u8> {
        format!(
            r#"{{"error":{{"code":403,"status":"PERMISSION_DENIED","errors":[{{"reason":"{reason}"}}]}}}}"#
        )
        .into_bytes()
    }

    fn now() -> OffsetDateTime {
        datetime!(2026-10-05 00:00 UTC)
    }

    #[test]
    fn gmail_error_401_maps_to_unauthorized() {
        assert_eq!(
            map_gmail_error(401, None, b"{}", now()),
            MailError::Unauthorized
        );
    }

    #[test]
    fn gmail_error_403_rate_reason_maps_to_rate_limited() {
        assert_eq!(
            map_gmail_error(403, Some("30"), &body("rateLimitExceeded"), now()),
            MailError::RateLimited { retry_after_s: 30 }
        );
    }

    #[test]
    fn gmail_error_403_permission_maps_to_forbidden() {
        assert_eq!(
            map_gmail_error(403, None, &body("insufficientPermissions"), now()),
            MailError::Forbidden
        );
    }

    #[test]
    fn gmail_error_429_retry_after_seconds_passed_through() {
        assert_eq!(
            map_gmail_error(429, Some("30"), b"{}", now()),
            MailError::RateLimited { retry_after_s: 30 }
        );
    }

    #[test]
    fn gmail_error_429_http_date_retry_after_uses_clock() {
        // 4 minutes after the injected `now`.
        assert_eq!(
            map_gmail_error(429, Some("Mon, 05 Oct 2026 00:04:00 GMT"), b"{}", now()),
            MailError::RateLimited { retry_after_s: 240 }
        );
    }

    #[test]
    fn gmail_error_500_maps_to_transient() {
        assert_eq!(
            map_gmail_error(500, None, b"{}", now()),
            MailError::Transient
        );
    }

    #[test]
    fn gmail_retry_after_clamped() {
        // Too large: clamped down to MAX.
        assert_eq!(
            map_gmail_error(429, Some("9999"), b"{}", now()),
            MailError::RateLimited {
                retry_after_s: MAX_RETRY_AFTER_S
            }
        );
        // Unparsable: the default.
        assert_eq!(
            map_gmail_error(429, Some("soon"), b"{}", now()),
            MailError::RateLimited {
                retry_after_s: DEFAULT_RETRY_AFTER_S
            }
        );
        // In the past: clamped up to 1.
        assert_eq!(
            map_gmail_error(429, Some("Mon, 05 Oct 2026 00:00:00 GMT"), b"{}", now()),
            MailError::RateLimited { retry_after_s: 1 }
        );
    }

    #[test]
    fn gmail_error_400_409_412_use_fixed_reasons() {
        assert_eq!(
            map_gmail_error(400, None, &body("invalidArgument"), now()),
            MailError::Invalid("gmail_bad_request".to_owned())
        );
        assert_eq!(
            map_gmail_error(409, None, &body("alreadyExists"), now()),
            MailError::Invalid("gmail_conflict".to_owned())
        );
        assert_eq!(
            map_gmail_error(412, None, &body("failedPrecondition"), now()),
            MailError::Invalid("gmail_precondition".to_owned())
        );
        assert_eq!(
            map_gmail_error(404, None, b"{}", now()),
            MailError::NotFound
        );
        assert_eq!(
            map_gmail_error(410, None, b"{}", now()),
            MailError::NotFound
        );
    }

    /// ASVS V16.5.1: Gmail's `message` text never appears in the `MailError`.
    #[test]
    fn asvs_v16_5_1_gmail_error_text_not_in_mail_error() {
        let canary = "CANARY-provider-message-4f2a";
        let body = format!(
            r#"{{"error":{{"code":400,"message":"{canary}","status":"INVALID_ARGUMENT","errors":[{{"reason":"invalidArgument","message":"{canary}"}}]}}}}"#
        );
        for status in [400u16, 401, 403, 404, 409, 410, 412, 429, 500, 503] {
            let err = map_gmail_error(status, None, body.as_bytes(), now());
            assert!(
                !format!("{err:?}").contains(canary),
                "provider text leaked for {status}"
            );
            assert!(
                !format!("{err}").contains(canary),
                "provider text leaked for {status}"
            );
        }
    }
}
