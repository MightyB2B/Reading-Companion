//! Books, pages, blocks, summaries, and vocabulary.
//!
//! Every handler here takes a [`Caller`], and every engine call takes that
//! caller's id. There is no path through this module that reaches a library
//! without naming whose it is.

use axum::extract::{Path, State};
use axum::Json;
use reading_core::models::{Block, Book, BookStats, Page, SentenceSummary, SpineEntry, VocabEntry};
use serde::Deserialize;

use crate::auth_layer::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

// --- Books ------------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewBook {
    pub title: String,
    pub author: Option<String>,
    #[serde(default = "default_era")]
    pub era: String,
}

fn default_era() -> String {
    "modern".to_string()
}

pub async fn list_books(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<Book>>> {
    Ok(Json(state.db.list_books(caller.id()).await?))
}

pub async fn create_book(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<NewBook>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = state
        .db
        .create_book(
            caller.id(),
            &body.title,
            body.author.as_deref(),
            &body.era,
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn get_book(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<Book>> {
    Ok(Json(state.db.get_book(caller.id(), book_id).await?))
}

pub async fn book_stats(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<BookStats>> {
    Ok(Json(state.db.book_stats(caller.id(), book_id).await?))
}

/// Delete a book, its pages, and the images on disk.
///
/// The rows go by cascade; the files do not, and leaving multi-megabyte
/// photographs behind for a book the reader deleted is how a library quietly
/// fills a disk.
pub async fn delete_book(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<axum::http::StatusCode> {
    // Ownership is proved by this call; everything after it is cleanup.
    state.db.get_book(caller.id(), book_id).await?;
    state.db.delete_book(caller.id(), book_id).await?;

    let dir = state.library_dir.join(format!("book-{book_id}"));
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            // The book is gone from the library either way, so this is a
            // disk-space problem rather than a failed request.
            tracing::warn!(book = book_id, error = %e, "could not remove book directory");
        }
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LastPage {
    pub page_id: Option<i64>,
}

pub async fn set_last_page(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
    Json(body): Json<LastPage>,
) -> ApiResult<axum::http::StatusCode> {
    state
        .db
        .set_last_page(caller.id(), book_id, body.page_id)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn resume_page(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let page = state.db.resume_page(caller.id(), book_id).await?;
    Ok(Json(serde_json::json!({ "page_id": page })))
}

// --- Pages ------------------------------------------------------------------

pub async fn list_pages(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<Vec<Page>>> {
    Ok(Json(state.db.list_pages(caller.id(), book_id).await?))
}

pub async fn delete_page(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
) -> ApiResult<axum::http::StatusCode> {
    let files = state.db.delete_page(caller.id(), page_id).await?;
    for path in files {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(path = %path, error = %e, "could not remove page image");
        }
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PageNumber {
    pub page_no: Option<i64>,
}

pub async fn set_page_number(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
    Json(body): Json<PageNumber>,
) -> ApiResult<axum::http::StatusCode> {
    state
        .db
        .set_page_number(caller.id(), page_id, body.page_no, "manual")
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn adjacent_pages(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let next = state.db.next_page_id(caller.id(), page_id).await?;
    let previous = state.db.previous_page_id(caller.id(), page_id).await?;
    Ok(Json(serde_json::json!({
        "next": next,
        "previous": previous,
    })))
}

// --- Blocks -----------------------------------------------------------------

pub async fn list_blocks(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
) -> ApiResult<Json<Vec<Block>>> {
    Ok(Json(state.db.list_blocks(caller.id(), page_id).await?))
}

#[derive(Deserialize)]
pub struct BlockEdit {
    pub text: String,
}

pub async fn edit_block(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
    Json(body): Json<BlockEdit>,
) -> ApiResult<axum::http::StatusCode> {
    state
        .db
        .edit_block(caller.id(), block_id, &body.text)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct BlockKindChange {
    pub kind: String,
}

pub async fn set_block_kind(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
    Json(body): Json<BlockKindChange>,
) -> ApiResult<axum::http::StatusCode> {
    state
        .db
        .set_block_kind(caller.id(), block_id, &body.kind)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn delete_block(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
) -> ApiResult<axum::http::StatusCode> {
    state.db.delete_block(caller.id(), block_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// --- Summaries --------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewSummary {
    pub sentence: String,
    /// Null for a summary of the whole paragraph.
    #[serde(default)]
    pub sentence_ordinal: Option<i64>,
    #[serde(default)]
    pub self_checked: bool,
}

pub async fn save_summary(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
    Json(body): Json<NewSummary>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = state
        .db
        .save_summary(
            caller.id(),
            block_id,
            body.sentence_ordinal,
            &body.sentence,
            body.self_checked,
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn sentence_summaries(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
) -> ApiResult<Json<Vec<SentenceSummary>>> {
    Ok(Json(
        state.db.sentence_summaries(caller.id(), block_id).await?,
    ))
}

pub async fn summary_spine(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<Vec<SpineEntry>>> {
    Ok(Json(state.db.summary_spine(caller.id(), book_id).await?))
}

// --- Paragraphs across a page break -----------------------------------------

#[derive(serde::Serialize)]
pub struct ParagraphContext {
    /// The paragraph, stitched with its continuation on the adjacent page.
    pub text: String,
    pub spans_pages: bool,
    pub continued_from_page: Option<i64>,
    pub continues_on_page: Option<i64>,
    /// Runs on, but the page finishing it has not been imported.
    pub incomplete: bool,
}

/// A paragraph plus whatever continues it on the neighbouring page.
///
/// The reader must never be asked to summarise half a thought, so a paragraph
/// broken by a page break is put back together before it is shown.
pub async fn paragraph_context(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
) -> ApiResult<Json<ParagraphContext>> {
    use reading_core::ocr::segment::{ends_mid_sentence, starts_mid_sentence, stitch_across_pages};

    let block = state.db.get_block(caller.id(), block_id).await?;

    // The neighbours are gathered first, because the dictionary borrow used
    // for stitching cannot be held across an await.
    let opens_mid = starts_mid_sentence(&block.text_norm);
    let breaks_off = ends_mid_sentence(&block.text_norm);

    let mut tail_before = None;
    let mut continued_from_page = None;
    if opens_mid {
        let is_first_prose = state
            .db
            .edge_paragraph(caller.id(), block.page_id, true)
            .await?
            .is_some_and(|b| b.id == block_id);

        if is_first_prose {
            if let Some(prev) = state.db.previous_page_id(caller.id(), block.page_id).await? {
                if let Some(t) = state.db.edge_paragraph(caller.id(), prev, false).await? {
                    if ends_mid_sentence(&t.text_norm) {
                        tail_before = Some(t.text_norm);
                        continued_from_page = Some(prev);
                    }
                }
            }
        }
    }

    let mut head_after = None;
    let mut continues_on_page = None;
    let mut incomplete = false;
    if breaks_off {
        let is_last_prose = state
            .db
            .edge_paragraph(caller.id(), block.page_id, false)
            .await?
            .is_some_and(|b| b.id == block_id);

        if is_last_prose {
            match state.db.next_page_id(caller.id(), block.page_id).await? {
                Some(next) => {
                    if let Some(h) = state.db.edge_paragraph(caller.id(), next, true).await? {
                        if starts_mid_sentence(&h.text_norm) {
                            head_after = Some(h.text_norm);
                            continues_on_page = Some(next);
                        }
                    }
                }
                // The page that finishes this thought has not been imported.
                None => incomplete = true,
            }
        }
    }

    let text = {
        let oracle = state
            .dict
            .as_deref()
            .map(|d| d as &dyn reading_core::ocr::segment::WordOracle);

        let mut text = block.text_norm.clone();
        if let Some(tail) = &tail_before {
            text = stitch_across_pages(tail, &text, oracle);
        }
        if let Some(head) = &head_after {
            text = stitch_across_pages(&text, head, oracle);
        }
        text
    };

    Ok(Json(ParagraphContext {
        spans_pages: continued_from_page.is_some() || continues_on_page.is_some(),
        text,
        continued_from_page,
        continues_on_page,
        incomplete,
    }))
}

// --- Outline ----------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct OutlineHeading {
    pub block_id: i64,
    pub text: String,
    pub level: String,
}

#[derive(serde::Serialize)]
pub struct OutlinePage {
    pub page_id: i64,
    pub page_no: Option<i64>,
    pub ocr_status: String,
    pub source_kind: String,
    /// First line of prose, for recognising a page at a glance.
    pub preview: String,
    pub headings: Vec<OutlineHeading>,
    pub paragraphs: i64,
    pub summarised: i64,
}

/// The book's structure, built from its headings.
pub async fn book_outline(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<Vec<OutlinePage>>> {
    use reading_core::ocr::outline::{fold_titles, heading_level, preview, Heading, HeadingLevel};

    let pages = state.db.list_pages(caller.id(), book_id).await?;
    let mut out = Vec::with_capacity(pages.len());

    for page in pages {
        let blocks = state.db.list_blocks(caller.id(), page.id).await?;

        // A heading matching the page's declared division came from the
        // document's own table of contents, so it *is* structure and its level
        // should not be re-guessed from its wording. "The First Book of Moses:
        // Called Genesis" reads as an ordinary title and would otherwise rank
        // below the numbered sections inside it.
        let declared = page.source_ref.as_deref().map(str::trim);

        // Folded so that a chapter's title joins the marker above it: books
        // set "CHAPTER I." and "ON METHOD." on separate lines, and they are
        // one chapter.
        let headings: Vec<OutlineHeading> = fold_titles(
            blocks
                .iter()
                .filter(|b| b.kind == "heading")
                .map(|b| {
                    let guessed = heading_level(&b.text_norm);
                    let level = match declared {
                        Some(d) if d == b.text_norm.trim() && guessed > HeadingLevel::Chapter => {
                            HeadingLevel::Chapter
                        }
                        _ => guessed,
                    };
                    Heading {
                        block_id: b.id,
                        text: b.text_norm.clone(),
                        level,
                    }
                })
                .collect(),
        )
        .into_iter()
        .map(|h| OutlineHeading {
            block_id: h.block_id,
            text: h.text,
            level: h.level.as_str().to_string(),
        })
        .collect();

        let paragraph_ids: Vec<i64> = blocks
            .iter()
            .filter(|b| b.kind == "paragraph")
            .map(|b| b.id)
            .collect();

        let mut summarised = 0i64;
        for id in &paragraph_ids {
            if state
                .db
                .latest_summary(caller.id(), *id)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                summarised += 1;
            }
        }

        out.push(OutlinePage {
            page_id: page.id,
            page_no: page.page_no,
            ocr_status: page.ocr_status.clone(),
            source_kind: page.source_kind.clone(),
            // The first line of prose, not of the page: a running head or a
            // chapter number tells you nothing about which page this is.
            preview: blocks
                .iter()
                .find(|b| b.kind == "paragraph")
                .map(|b| preview(&b.text_norm, 70))
                .unwrap_or_default(),
            headings,
            paragraphs: paragraph_ids.len() as i64,
            summarised,
        });
    }

    Ok(Json(out))
}

// --- Vocabulary -------------------------------------------------------------

pub async fn vocabulary(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
) -> ApiResult<Json<Vec<VocabEntry>>> {
    Ok(Json(state.db.vocabulary(caller.id(), book_id).await?))
}
