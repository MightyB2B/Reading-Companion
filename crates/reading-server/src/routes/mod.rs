//! The HTTP surface.
//!
//! Routes are grouped by what they touch rather than by verb, and the naming
//! is deliberately boring: `/api/books/:id/pages` says what it is without a
//! document.

pub mod accounts;
pub mod images;
pub mod import;
pub mod library;
pub mod models;
pub mod study;

use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // Open. Everything else takes a Caller, which cannot be constructed
    // without a valid token — so this list is the complete set of endpoints
    // reachable by an anonymous request, and it is short on purpose.
    let public = Router::new()
        .route("/api/health", get(health))
        .route("/api/setup-state", get(accounts::needs_setup))
        .route("/api/register", post(accounts::register))
        .route("/api/sign-in", post(accounts::sign_in));

    let authenticated = Router::new()
        .route("/api/me", get(accounts::me))
        .route("/api/sign-out", post(accounts::sign_out))
        // Books
        .route("/api/books", get(library::list_books).post(library::create_book))
        .route("/api/books/{book_id}", get(library::get_book).delete(library::delete_book))
        .route("/api/books/{book_id}/stats", get(library::book_stats))
        .route("/api/books/{book_id}/last-page", put(library::set_last_page))
        .route("/api/books/{book_id}/resume", get(library::resume_page))
        .route("/api/books/{book_id}/spine", get(library::summary_spine))
        .route("/api/books/{book_id}/outline", get(library::book_outline))
        .route("/api/books/{book_id}/vocabulary", get(library::vocabulary))
        // Pages
        .route("/api/books/{book_id}/pages", get(library::list_pages))
        .route("/api/books/{book_id}/pages/photo", post(import::import_photo))
        .route("/api/books/{book_id}/documents", post(import::import_document))
        .route("/api/pages/{page_id}/transcribe", post(import::transcribe))
        .route("/api/pages/{page_id}", delete(library::delete_page))
        .route("/api/pages/{page_id}/number", put(library::set_page_number))
        .route("/api/pages/{page_id}/adjacent", get(library::adjacent_pages))
        .route("/api/pages/{page_id}/blocks", get(library::list_blocks))
        .route("/api/pages/{page_id}/image", get(images::page_image))
        // Blocks
        .route("/api/blocks/{block_id}", put(library::edit_block).delete(library::delete_block))
        .route("/api/blocks/{block_id}/kind", put(library::set_block_kind))
        .route("/api/blocks/{block_id}/summary", post(library::save_summary))
        .route("/api/blocks/{block_id}/sentences", get(library::sentence_summaries))
        .route("/api/blocks/{block_id}/context", get(library::paragraph_context))
        .route("/api/blocks/{block_id}/critique", post(study::critique))
        .route("/api/blocks/{block_id}/exemplar", get(study::exemplar))
        // Dictionary
        .route("/api/dictionary/look-up", post(study::look_up_word))
        .route("/api/dictionary/in-context", post(study::word_in_context))
        // Configuration
        .route("/api/settings", get(models::get_settings).put(models::save_settings))
        .route("/api/ollama/status", get(models::check_ollama))
        .route("/api/ollama/models", get(models::list_models));

    public.merge(authenticated).with_state(state)
}

/// Unauthenticated on purpose: a health check that needs credentials cannot
/// be used by the thing that restarts the server.
async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "service": "reading-server",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
