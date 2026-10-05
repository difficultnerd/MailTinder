//! Strict JSON input and output.

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{FromRef, FromRequest, FromRequestParts, State};
use axum::http::{header, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

/// A JSON request body, strictly validated: content type, size, and unknown
/// fields are refused. `T` must derive `Deserialize` with `deny_unknown_fields`.
pub struct ApiJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let State(app_state) =
            <State<AppState> as FromRequestParts<S>>::from_request_parts(&mut parts, state)
                .await
                .map_err(|_| ApiError::Internal)?;
        let req = Request::from_parts(parts, body);
        let max_body = app_state.config.max_body_bytes;

        // Content type must be application/json (optionally with charset).
        let method = req.method().clone();
        if matches!(
            method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        ) {
            let ct = req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let base = ct.split(';').next().unwrap_or("").trim();
            let allowed = ct.eq_ignore_ascii_case("application/json")
                || (base.eq_ignore_ascii_case("application/json")
                    && ct.split_once(';').is_some_and(|(_, param)| {
                        param.trim().eq_ignore_ascii_case("charset=utf-8")
                    }));
            if !allowed {
                return Err(ApiError::UnsupportedMediaType);
            }
        }

        // Content-Length check first.
        if let Some(len) = req.headers().get(header::CONTENT_LENGTH) {
            if let Ok(len_str) = len.to_str() {
                if let Ok(len) = len_str.parse::<usize>() {
                    if len > max_body {
                        return Err(ApiError::PayloadTooLarge);
                    }
                }
            }
        }

        let bytes = axum::body::to_bytes(req.into_body(), max_body)
            .await
            .map_err(|_| ApiError::PayloadTooLarge)?;
        if bytes.len() > max_body {
            return Err(ApiError::PayloadTooLarge);
        }

        let mut de = serde_json::Deserializer::from_slice(&bytes);
        match serde_path_to_error::deserialize(&mut de) {
            Ok(value) => Ok(ApiJson(value)),
            Err(err) => {
                let path = err.path().to_string();
                let pointer = if path.is_empty() {
                    String::new()
                } else {
                    format!(
                        "/{}",
                        path.split('.')
                            .map(|part| part.replace('~', "~0").replace('/', "~1"))
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                };
                Err(ApiError::InvalidRequest {
                    fields: vec![pointer],
                })
            }
        }
    }
}

/// A JSON response body with the correct content type.
pub fn json_ok<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let Ok(body) = serde_json::to_vec(body) else {
        return ApiError::Internal.into_response();
    };
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp
}

impl<T: IntoResponse> IntoResponse for ApiJson<T> {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
