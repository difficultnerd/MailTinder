#![allow(clippy::expect_used, clippy::too_many_lines, clippy::unwrap_used)]

//! Coverage tests for the api crate's error model, rate limiter, and headers.

use api::config::ApiConfig;
use api::error::ApiError;
use api::http::client_ip::client_ip;
use api::http::headers::apply_security_headers;
use api::limits::{self, LimitSubject, RateLimiter};
use api::state::AppState;
use axum::http::{Method, StatusCode};
use axum::response::IntoResponse;
use obs::Sensitive;
use ports::MailError;
use std::sync::Arc;
use testkit::fake_ports;

fn config() -> ApiConfig {
    ApiConfig::new(
        "https://mailtinder.test".into(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )
    .expect("valid origin")
}

#[test]
fn error_status_code_title_for_every_variant() {
    let cases: Vec<(ApiError, StatusCode, &str, &str)> = vec![
        (
            ApiError::InvalidRequest {
                fields: vec!["/x".into()],
            },
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid request",
        ),
        (
            ApiError::Unauthenticated,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Not authenticated",
        ),
        (
            ApiError::CsrfFailed,
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "CSRF check failed",
        ),
        (
            ApiError::Forbidden,
            StatusCode::FORBIDDEN,
            "forbidden",
            "Forbidden",
        ),
        (
            ApiError::StepUpRequired,
            StatusCode::FORBIDDEN,
            "step_up_required",
            "Step-up authentication required",
        ),
        (
            ApiError::NotFound,
            StatusCode::NOT_FOUND,
            "not_found",
            "Not found",
        ),
        (
            ApiError::AppFolderMoveFailed,
            StatusCode::CONFLICT,
            "app_folder_move_failed",
            "App folder move failed",
        ),
        (
            ApiError::LastMailbox,
            StatusCode::CONFLICT,
            "last_mailbox",
            "Cannot remove last mailbox",
        ),
        (
            ApiError::MailboxNeedsSignIn { mailbox_id: None },
            StatusCode::CONFLICT,
            "mailbox_needs_sign_in",
            "Mailbox needs sign-in",
        ),
        (
            ApiError::MessageChanged,
            StatusCode::CONFLICT,
            "message_changed",
            "Message changed",
        ),
        (
            ApiError::CategoryExists,
            StatusCode::CONFLICT,
            "category_exists",
            "Category already exists",
        ),
        (
            ApiError::ConsentOutdated,
            StatusCode::CONFLICT,
            "consent_outdated",
            "Consent outdated",
        ),
        (
            ApiError::ExperimentUnavailable,
            StatusCode::CONFLICT,
            "experiment_unavailable",
            "Experiment unavailable",
        ),
        (
            ApiError::VersionsMixed,
            StatusCode::CONFLICT,
            "versions_mixed",
            "Versions mixed",
        ),
        (
            ApiError::SnapshotLimit,
            StatusCode::CONFLICT,
            "snapshot_limit",
            "Snapshot limit reached",
        ),
        (
            ApiError::NotAcceptable,
            StatusCode::NOT_ACCEPTABLE,
            "not_acceptable",
            "Not acceptable",
        ),
        (
            ApiError::UndoExpired,
            StatusCode::GONE,
            "undo_expired",
            "Undo expired",
        ),
        (
            ApiError::PayloadTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Payload too large",
        ),
        (
            ApiError::UnsupportedMediaType,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Unsupported media type",
        ),
        (
            ApiError::RateLimited { retry_after_s: 5 },
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many requests",
        ),
        (
            ApiError::ProviderError { mailbox_id: None },
            StatusCode::BAD_GATEWAY,
            "provider_error",
            "Provider error",
        ),
        (
            ApiError::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: None,
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_unavailable",
            "Provider unavailable",
        ),
        (
            ApiError::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Something went wrong",
        ),
    ];
    for (err, status, code, title) in cases {
        assert_eq!(err.status(), status, "status for {code}");
        assert_eq!(err.code(), code);
        assert_eq!(err.title(), title);
    }
}

#[test]
fn error_from_mail_error_mapping() {
    assert_eq!(
        ApiError::from(MailError::Unauthorized),
        ApiError::MailboxNeedsSignIn { mailbox_id: None }
    );
    assert_eq!(
        ApiError::from(MailError::Forbidden),
        ApiError::ProviderError { mailbox_id: None }
    );
    assert_eq!(
        ApiError::from(MailError::NotFound),
        ApiError::MessageChanged
    );
    assert_eq!(
        ApiError::from(MailError::RateLimited { retry_after_s: 3 }),
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: Some(3)
        }
    );
    assert_eq!(
        ApiError::from(MailError::Transient),
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None
        }
    );
    assert_eq!(
        ApiError::from(MailError::Invalid("x".into())),
        ApiError::ProviderError { mailbox_id: None }
    );
}

#[test]
fn error_with_mailbox_sets_id() {
    let id = uuid::Uuid::new_v4();
    assert_eq!(
        ApiError::ProviderError { mailbox_id: None }.with_mailbox(id),
        ApiError::ProviderError {
            mailbox_id: Some(id)
        }
    );
    assert_eq!(
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: Some(1)
        }
        .with_mailbox(id),
        ApiError::ProviderUnavailable {
            mailbox_id: Some(id),
            retry_after_s: Some(1)
        }
    );
    // Non-provider errors are unchanged.
    assert_eq!(ApiError::NotFound.with_mailbox(id), ApiError::NotFound);
}

#[test]
fn error_into_response_sets_status_and_extension() {
    let resp = ApiError::NotFound.into_response();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(resp.extensions().get::<ApiError>().is_some());
}

#[tokio::test]
async fn rate_limiter_memory_fixed_window() {
    let (ports, _) = fake_ports();
    let limiter = RateLimiter::new(
        ports.store.clone(),
        ports.clock.clone(),
        Sensitive::new(b"k".to_vec()),
    );
    let id = api::http::request_id::RequestId(uuid::Uuid::new_v4());
    let user = domain::UserId(uuid::Uuid::new_v4());
    // FEED allows 30 per 60s.
    for _ in 0..30 {
        assert!(limiter
            .check(&limits::policies::FEED, LimitSubject::User(&user), id)
            .await
            .is_ok());
    }
    let err = limiter
        .check(&limits::policies::FEED, LimitSubject::User(&user), id)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::RateLimited { .. }));
}

#[tokio::test]
async fn rate_limiter_firestore_policy() {
    let (ports, _) = fake_ports();
    let limiter = RateLimiter::new(
        ports.store.clone(),
        ports.clock.clone(),
        Sensitive::new(b"k".to_vec()),
    );
    let id = api::http::request_id::RequestId(uuid::Uuid::new_v4());
    let ip = api::http::client_ip::ClientIp(Some("203.0.113.5".parse().unwrap()));
    // SIGN_IN_IP allows 20 per 600s.
    for _ in 0..20 {
        assert!(limiter
            .check(&limits::policies::SIGN_IN_IP, LimitSubject::Ip(&ip), id)
            .await
            .is_ok());
    }
    let err = limiter
        .check(&limits::policies::SIGN_IN_IP, LimitSubject::Ip(&ip), id)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::RateLimited { .. }));
}

#[test]
fn default_policy_maps_methods() {
    assert_eq!(
        limits::default_policy(&Method::GET).map(|p| p.name),
        Some("reads")
    );
    assert_eq!(
        limits::default_policy(&Method::POST).map(|p| p.name),
        Some("writes")
    );
    assert_eq!(
        limits::default_policy(&Method::PUT).map(|p| p.name),
        Some("writes")
    );
    assert_eq!(
        limits::default_policy(&Method::PATCH).map(|p| p.name),
        Some("writes")
    );
    assert_eq!(
        limits::default_policy(&Method::DELETE).map(|p| p.name),
        Some("writes")
    );
    assert!(limits::default_policy(&Method::OPTIONS).is_none());
}

#[test]
fn apply_limit_headers_sets_ratelimit_headers() {
    let info = limits::LimitInfo {
        policy: &limits::policies::FEED,
        remaining: 29,
        reset_s: 60,
    };
    let mut resp = StatusCode::OK.into_response();
    limits::apply_limit_headers(&mut resp, &info);
    assert!(resp.headers().contains_key("ratelimit-policy"));
    assert!(resp.headers().contains_key("ratelimit"));
}

#[test]
fn client_ip_error_paths() {
    // Unparseable and missing give None.
    let mut h = axum::http::HeaderMap::new();
    h.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
    assert_eq!(client_ip(&h, 1), api::http::client_ip::ClientIp(None));
    assert_eq!(
        client_ip(&axum::http::HeaderMap::new(), 1),
        api::http::client_ip::ClientIp(None)
    );
}

#[test]
fn security_headers_applied_and_cors_stripped() {
    let mut resp = StatusCode::OK.into_response();
    resp.headers_mut().insert(
        "access-control-allow-origin",
        "https://evil.test".parse().unwrap(),
    );
    apply_security_headers(&mut resp);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    assert_eq!(resp.headers()["referrer-policy"], "no-referrer");
    assert!(resp.headers().contains_key("content-security-policy"));
    assert!(resp.headers().contains_key("strict-transport-security"));
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
}

#[test]
fn app_state_builds() {
    let (ports, _) = fake_ports();
    let state = api::app_state(Arc::new(ports), Arc::new(config()));
    let _: AppState = state;
}
