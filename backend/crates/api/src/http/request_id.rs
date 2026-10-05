//! Request ID: a UUID set as the first layer, echoed on every response.

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

/// A request ID, set as a request extension by the first layer.
#[derive(Clone, Copy, Debug)]
pub struct RequestId(pub Uuid);

#[async_trait]
impl<S> FromRequestParts<S> for RequestId {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<RequestId>()
            .copied()
            .ok_or(crate::error::ApiError::Internal)
    }
}
