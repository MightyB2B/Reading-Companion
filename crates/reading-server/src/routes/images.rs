//! Serving page photographs.
//!
//! The one place the server hands back bytes from disk, and therefore the one
//! place a path traversal could matter. Two things prevent it: the path comes
//! from the database rather than the request, and it is checked to be inside
//! the library directory before anything is read.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use reading_core::AppError;

use crate::auth_layer::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The image for a page, if it is the caller's.
pub async fn page_image(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
) -> ApiResult<Response> {
    // Listing through the owning book is the authorisation: a page belonging
    // to someone else is simply not in this list.
    let book_id = state.db.book_id_for_page(caller.id(), page_id).await?;
    let page = state
        .db
        .list_pages(caller.id(), book_id)
        .await?
        .into_iter()
        .find(|p| p.id == page_id)
        .ok_or_else(|| ApiError(AppError::NotFound(format!("page {page_id}"))))?;

    // The full-resolution copy is what the reader wants to look at; the
    // bounded one exists for the model.
    let path = std::path::PathBuf::from(&page.image_orig);

    // Belt and braces. The stored path should always be inside the library,
    // but "should always" is not a security property — a row written by an
    // older version, or by hand, must not be able to read /etc/passwd.
    let base = state
        .library_dir
        .canonicalize()
        .map_err(|e| ApiError(AppError::Io(e)))?;
    let resolved = path
        .canonicalize()
        .map_err(|_| ApiError(AppError::NotFound(format!("image for page {page_id}"))))?;

    if !resolved.starts_with(&base) {
        tracing::warn!(page = page_id, path = %resolved.display(), "image path outside the library");
        return Err(ApiError(AppError::NotFound(format!(
            "image for page {page_id}"
        ))));
    }

    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(|_| ApiError(AppError::NotFound(format!("image for page {page_id}"))))?;

    let mime = match resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("tif") | Some("tiff") => "image/tiff",
        // Everything the importer writes is JPEG, including what it converts
        // HEIC into, so this is the common case rather than a fallback.
        _ => "image/jpeg",
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            // Private: this is one person's photograph of one page, and a
            // shared cache holding it would be handing it to the next reader.
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        bytes,
    )
        .into_response())
}
