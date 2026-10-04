//! Gmail-shaped error bodies for `fake-google`.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// The GRPC status name for an HTTP status.
fn grpc_name(status: u16) -> &'static str {
    match status {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        409 => "ALREADY_EXISTS",
        429 => "RESOURCE_EXHAUSTED",
        500 => "INTERNAL",
        503 => "UNAVAILABLE",
        _ => "UNKNOWN",
    }
}

/// A Gmail-shaped error response.
pub struct GmailError {
    pub status: u16,
    pub reason: String,
    pub retry_after: Option<String>,
}

impl GmailError {
    pub fn new(status: u16, reason: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.into(),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after: impl Into<String>) -> Self {
        self.retry_after = Some(retry_after.into());
        self
    }
}

impl IntoResponse for GmailError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let message = match self.status {
            400 => "Invalid request",
            401 => "Request had invalid authentication credentials",
            403 => "Insufficient permissions",
            404 => "Resource not found",
            409 => "Resource already exists",
            429 => "Quota exceeded",
            500 => "Internal error",
            503 => "Service unavailable",
            _ => "Error",
        };
        let body = json!({
            "error": {
                "code": self.status,
                "message": message,
                "status": grpc_name(self.status),
                "errors": [{
                    "domain": "global",
                    "reason": self.reason,
                    "message": message,
                }],
            }
        });
        let mut resp = (status, Json(body)).into_response();
        if let Some(ra) = self.retry_after {
            resp.headers_mut().insert(
                "Retry-After",
                HeaderValue::from_str(&ra).unwrap_or_else(|_| HeaderValue::from_static("10")),
            );
        }
        resp
    }
}
