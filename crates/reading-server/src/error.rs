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

/// A refusal that is about timing rather than the request.
///
/// Carried separately because it needs a `Retry-After` header, which the
/// engine's error type has no way to express — and should not, since waiting
/// is a property of this transport rather than of the library.
pub struct RetryAfter {
    pub seconds: u64,
    pub message: String,
}

impl ApiError {
    pub fn retry_after(seconds: u64, message: String) -> Self {
        ApiError(AppError::Other(anyhow::anyhow!(RetryAfterMarker {
            seconds,
            message,
        })))
    }
}

/// Smuggled through `AppError::Other` so the throttle needs no change to the
/// engine's error type. Recognised on the way out by downcasting.
#[derive(Debug)]
struct RetryAfterMarker {
    seconds: u64,
    message: String,
}

impl std::fmt::Display for RetryAfterMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RetryAfterMarker {}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Checked first: a throttled request is not a server fault and must
        // not be logged as one or reported as "something went wrong".
        if let AppError::Other(inner) = &self.0 {
            if let Some(retry) = inner.downcast_ref::<RetryAfterMarker>() {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, retry.seconds.to_string())],
                    Json(ErrorBody {
                        error: retry.message.clone(),
                    }),
                )
                    .into_response();
            }
        }

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
