//! Turning engine errors into HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use reading_core::AppError;
use serde::Serialize;

/// What the client receives. One field, so there is nothing to destructure.
#[derive(Serialize)]
pub struct ErrorBody {
    pub error: String,
}

pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            // The engine reports "not found" for a row that exists but belongs
            // to someone else, and that distinction must not reappear here as
            // a 403. Both are 404: telling them apart would confirm the id.
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, self.0.to_string()),

            AppError::Invalid(m) => {
                // "not signed in" is the one Invalid that is really an
                // authentication failure, and the client needs to tell it
                // apart to know it should prompt for a sign-in.
                if m == "not signed in" {
                    (StatusCode::UNAUTHORIZED, m.clone())
                } else {
                    (StatusCode::BAD_REQUEST, m.clone())
                }
            }

            AppError::OllamaUnreachable { .. } | AppError::OllamaStatus { .. } => {
                (StatusCode::BAD_GATEWAY, self.0.to_string())
            }

            // Everything else is ours, and the details are for the log rather
            // than the client: an internal error message is a description of
            // the inside of the server.
            other => {
                tracing::error!(error = %other, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "something went wrong".to_string(),
                )
            }
        };

        (status, Json(ErrorBody { error: message })).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
