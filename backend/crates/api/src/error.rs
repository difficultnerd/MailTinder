//! The API error model: RFC 9457 Problem Details.
//!
//! `ApiError::into_response` cannot see the request, so it returns a response
//! with the status and the error as an extension; a `problem_layer` middleware
//! fills in `request_id` and renders the JSON body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ports::store::StoreError;
use ports::MailError;
use serde::Serialize;
use uuid::Uuid;

use crate::http::request_id::RequestId;

/// Every error the API can return, mapped to an S7 section 4 code and status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    InvalidRequest {
        fields: Vec<String>,
    },
    Unauthenticated,
    CsrfFailed,
    Forbidden,
    StepUpRequired,
    NotFound,
    AppFolderMoveFailed,
    LastMailbox,
    MailboxNeedsSignIn {
        mailbox_id: Option<Uuid>,
    },
    MessageChanged,
    CategoryExists,
    ConsentOutdated,
    ExperimentUnavailable,
    VersionsMixed,
    SnapshotLimit,
    NotAcceptable,
    UndoExpired,
    PayloadTooLarge,
    UnsupportedMediaType,
    RateLimited {
        retry_after_s: u64,
    },
    ProviderError {
        mailbox_id: Option<Uuid>,
    },
    ProviderUnavailable {
        mailbox_id: Option<Uuid>,
        retry_after_s: Option<u64>,
    },
    Internal,
}

impl ApiError {
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::CsrfFailed | Self::Forbidden | Self::StepUpRequired => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::AppFolderMoveFailed
            | Self::LastMailbox
            | Self::MailboxNeedsSignIn { .. }
            | Self::MessageChanged
            | Self::CategoryExists
            | Self::ConsentOutdated
            | Self::ExperimentUnavailable
            | Self::VersionsMixed
            | Self::SnapshotLimit => StatusCode::CONFLICT,
            Self::NotAcceptable => StatusCode::NOT_ACCEPTABLE,
            Self::UndoExpired => StatusCode::GONE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::ProviderError { .. } => StatusCode::BAD_GATEWAY,
            Self::ProviderUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "invalid_request",
            Self::Unauthenticated => "unauthenticated",
            Self::CsrfFailed => "csrf_failed",
            Self::Forbidden => "forbidden",
            Self::StepUpRequired => "step_up_required",
            Self::NotFound => "not_found",
            Self::AppFolderMoveFailed => "app_folder_move_failed",
            Self::LastMailbox => "last_mailbox",
            Self::MailboxNeedsSignIn { .. } => "mailbox_needs_sign_in",
            Self::MessageChanged => "message_changed",
            Self::CategoryExists => "category_exists",
            Self::ConsentOutdated => "consent_outdated",
            Self::ExperimentUnavailable => "experiment_unavailable",
            Self::VersionsMixed => "versions_mixed",
            Self::SnapshotLimit => "snapshot_limit",
            Self::NotAcceptable => "not_acceptable",
            Self::UndoExpired => "undo_expired",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::RateLimited { .. } => "rate_limited",
            Self::ProviderError { .. } => "provider_error",
            Self::ProviderUnavailable { .. } => "provider_unavailable",
            Self::Internal => "internal_error",
        }
    }

    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "Invalid request",
            Self::Unauthenticated => "Not authenticated",
            Self::CsrfFailed => "CSRF check failed",
            Self::Forbidden => "Forbidden",
            Self::StepUpRequired => "Step-up authentication required",
            Self::NotFound => "Not found",
            Self::AppFolderMoveFailed => "App folder move failed",
            Self::LastMailbox => "Cannot remove last mailbox",
            Self::MailboxNeedsSignIn { .. } => "Mailbox needs sign-in",
            Self::MessageChanged => "Message changed",
            Self::CategoryExists => "Category already exists",
            Self::ConsentOutdated => "Consent outdated",
            Self::ExperimentUnavailable => "Experiment unavailable",
            Self::VersionsMixed => "Versions mixed",
            Self::SnapshotLimit => "Snapshot limit reached",
            Self::NotAcceptable => "Not acceptable",
            Self::UndoExpired => "Undo expired",
            Self::PayloadTooLarge => "Payload too large",
            Self::UnsupportedMediaType => "Unsupported media type",
            Self::RateLimited { .. } => "Too many requests",
            Self::ProviderError { .. } => "Provider error",
            Self::ProviderUnavailable { .. } => "Provider unavailable",
            Self::Internal => "Something went wrong",
        }
    }

    /// Set the mailbox id on a provider error (callers fill it in).
    #[must_use]
    pub fn with_mailbox(self, mailbox_id: Uuid) -> Self {
        match self {
            Self::ProviderError { .. } => Self::ProviderError {
                mailbox_id: Some(mailbox_id),
            },
            Self::ProviderUnavailable { retry_after_s, .. } => Self::ProviderUnavailable {
                mailbox_id: Some(mailbox_id),
                retry_after_s,
            },
            Self::MailboxNeedsSignIn { .. } => Self::MailboxNeedsSignIn {
                mailbox_id: Some(mailbox_id),
            },
            other => other,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ApiError {}

impl From<MailError> for ApiError {
    fn from(e: MailError) -> Self {
        match e {
            MailError::Unauthorized => Self::MailboxNeedsSignIn { mailbox_id: None },
            MailError::Forbidden | MailError::Invalid(_) => {
                Self::ProviderError { mailbox_id: None }
            }
            MailError::NotFound => Self::MessageChanged,
            MailError::RateLimited { retry_after_s } => Self::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: Some(retry_after_s),
            },
            MailError::Transient => Self::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: None,
            },
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(_e: StoreError) -> Self {
        Self::Internal
    }
}

/// The RFC 9457 problem body. `request_id` is filled by the problem layer.
#[derive(Serialize)]
pub struct ProblemBody {
    #[serde(rename = "type")]
    pub type_: String,
    pub title: &'static str,
    pub status: u16,
    pub code: &'static str,
    pub request_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let mut resp = Response::new(axum::body::Body::empty());
        *resp.status_mut() = status;
        resp.extensions_mut().insert(self);
        resp
    }
}

/// Render the problem body for a response that carries an `ApiError` extension.
/// Called by the problem layer after the handler, with the request id.
pub fn render_problem(resp: &mut Response, request_id: RequestId) {
    let Some(err) = resp.extensions().get::<ApiError>() else {
        return;
    };
    let body = ProblemBody {
        type_: format!("https://mailtinder.app/problems/{}", err.code()),
        title: err.title(),
        status: err.status().as_u16(),
        code: err.code(),
        request_id: request_id.0,
        mailbox_id: match err {
            ApiError::MailboxNeedsSignIn { mailbox_id }
            | ApiError::ProviderError { mailbox_id }
            | ApiError::ProviderUnavailable { mailbox_id, .. } => *mailbox_id,
            _ => None,
        },
        retry_after_seconds: match err {
            ApiError::RateLimited { retry_after_s } => Some(*retry_after_s),
            ApiError::ProviderUnavailable { retry_after_s, .. } => *retry_after_s,
            _ => None,
        },
        fields: match err {
            ApiError::InvalidRequest { fields } => Some(
                fields
                    .iter()
                    .take(20)
                    .map(|f| f.chars().take(200).collect())
                    .collect(),
            ),
            _ => None,
        },
    };
    let retry_after = body.retry_after_seconds;
    let body = match serde_json::to_vec(&body) {
        Ok(json) => json,
        Err(_) => b"{\"type\":\"https://mailtinder.app/problems/internal_error\",\"title\":\"Something went wrong\",\"status\":500,\"code\":\"internal_error\",\"request_id\":\"00000000-0000-0000-0000-000000000000\"}".to_vec(),
    };
    *resp.body_mut() = axum::body::Body::from(body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/problem+json; charset=utf-8"),
    );
    if let Some(retry) = retry_after {
        if let Ok(v) = axum::http::HeaderValue::from_str(&retry.to_string()) {
            resp.headers_mut()
                .insert(axum::http::header::RETRY_AFTER, v);
        }
    }
}
