//! RFC 9457 problem+json responses.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use funding_arb_auth_service::AuthError;
use uuid::Uuid;

/// Transport wrapper that renders an [`AuthError`] as problem+json.
pub struct ApiError(pub AuthError);

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error = self.0;
        if let AuthError::Internal(detail) = &error {
            tracing::error!(error = %detail, "internal auth failure");
        }
        let status =
            StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::json!({
            "type": "about:blank",
            "title": error.title(),
            "status": status.as_u16(),
            "code": error.code(),
            "request_id": Uuid::new_v4().to_string(),
        });
        (
            status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            serde_json::to_string(&body).unwrap_or_default(),
        )
            .into_response()
    }
}
