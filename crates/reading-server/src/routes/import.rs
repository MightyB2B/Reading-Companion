//! Getting text into a book: photographs, and documents that are already text.
//!
//! Image work is CPU-bound and runs on a blocking thread. Doing it on the
//! async runtime would stall every other request on the server for the length
//! of a 12-megapixel decode, which is exactly the failure the desktop version
//! had before `spawn_blocking`.

use axum::extract::{Path, State};
use axum::Json;
use reading_core::ingest::SourceKind;
use reading_core::models::Era;
use reading_core::ocr::{self, preprocess};
use reading_core::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth_layer::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct PhotoUpload {
    /// The file, base64-encoded. JSON rather than multipart because the client
    /// is a desktop application reading a file it already has in memory, and
    /// one content type across the whole API is worth more than the third of
    /// a byte base64 costs.
    pub data: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    /// One id per page created — a photograph of an open book yields two.
    pub page_ids: Vec<i64>,
    /// The page to open first.
    pub page_id: i64,
    /// Set when the file was converted on import, e.g. "HEIC" from an iPhone.
    pub converted_from: Option<String>,
    /// True when the photograph was recognised as an open book and split.
    pub was_spread: bool,
}

/// Import a photograph, splitting a spread into two pages.
pub async fn import_photo(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
    Json(body): Json<PhotoUpload>,
) -> ApiResult<Json<ImportResult>> {
    // Proves the book is this caller's before anything is written to disk.
    state.db.get_book(caller.id(), book_id).await?;

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data.as_bytes())
        .map_err(|_| ApiError(AppError::Invalid("that upload was not valid base64".into())))?;

    if !preprocess::is_supported_image(&bytes) {
        return Err(ApiError(AppError::Invalid(
            "that file is not an image this can read".into(),
        )));
    }

    // Decoding, orientation, gutter detection and downscaling are all CPU.
    let prepared = tokio::task::spawn_blocking(move || {
        preprocess::prepare_import_pages(&bytes, &preprocess::PreprocessOptions::default())
    })
    .await
    .map_err(|e| ApiError(AppError::Other(anyhow::anyhow!("import panicked: {e}"))))??;

    let was_spread = prepared.len() > 1;
    let converted_from = prepared[0].converted_from.map(str::to_string);

    // Every half is hashed and checked before anything is written, so a
    // re-imported spread is rejected whole rather than half-added.
    for image in &prepared {
        let hash = hex(&Sha256::digest(&image.archival));
        if let Some((_, page_no)) = state.db.page_with_hash(caller.id(), book_id, &hash).await? {
            return Err(ApiError(AppError::Invalid(match page_no {
                Some(n) => format!("this photograph is already in the book as page {n}"),
                None => "this photograph is already in the book".to_string(),
            })));
        }
    }

    let dir = state.library_dir.join(format!("book-{book_id}"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError(AppError::Io(e)))?;

    // One stamp for the whole photograph, so the two halves of a spread can
    // find each other later to reconcile their page numbers.
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f").to_string();

    let mut page_ids = Vec::new();
    for image in &prepared {
        let hash = hex(&Sha256::digest(&image.archival));
        let side = image.side.map(|s| s.as_str()).unwrap_or("single");
        let base = format!("{stamp}-{side}");

        let archival_path = dir.join(format!("{base}.{}", image.archival_ext));
        let processed_path = dir.join(format!("{base}-proc.jpg"));

        tokio::fs::write(&archival_path, &image.archival)
            .await
            .map_err(|e| ApiError(AppError::Io(e)))?;
        tokio::fs::write(&processed_path, &image.processed)
            .await
            .map_err(|e| ApiError(AppError::Io(e)))?;

        let page_id = state
            .db
            .add_page(
                caller.id(),
                book_id,
                &hash,
                &archival_path.to_string_lossy(),
                &processed_path.to_string_lossy(),
            )
            .await?;

        if image.side.is_some() {
            state
                .db
                .set_page_source_ref(caller.id(), page_id, &format!("{stamp}:{side}"))
                .await?;
        }

        page_ids.push(page_id);
    }

    Ok(Json(ImportResult {
        page_id: page_ids[0],
        page_ids,
        converted_from,
        was_spread,
    }))
}

#[derive(Serialize)]
pub struct TranscriptionResult {
    pub blocks: Vec<reading_core::models::Block>,
    pub page_no: Option<i64>,
    /// detected | manual | unknown — so the client knows whether to ask.
    pub page_no_source: String,
}

/// Transcribe a page and derive its paragraphs.
pub async fn transcribe(
    State(state): State<AppState>,
    caller: Caller,
    Path(page_id): Path<i64>,
) -> ApiResult<Json<TranscriptionResult>> {
    let book_id = state.db.book_id_for_page(caller.id(), page_id).await?;
    let book = state.db.get_book(caller.id(), book_id).await?;

    let page = state
        .db
        .list_pages(caller.id(), book_id)
        .await?
        .into_iter()
        .find(|p| p.id == page_id)
        .ok_or_else(|| ApiError(AppError::NotFound(format!("page {page_id}"))))?;

    let image_path = page
        .image_proc
        .clone()
        .unwrap_or_else(|| page.image_orig.clone());

    let image = tokio::fs::read(&image_path)
        .await
        .map_err(|e| ApiError(AppError::Io(e)))?;

    let settings = state.settings();
    let client = state.ollama();

    state
        .db
        .set_page_status(caller.id(), page_id, "running", None)
        .await?;

    let raw = match client
        .ocr(
            &settings.ocr_model,
            image.clone(),
            &reading_core::ollama::OcrOptions::default(),
            |_| {},
        )
        .await
    {
        Ok(raw) => raw,
        Err(e) => {
            state
                .db
                .set_page_status(caller.id(), page_id, "failed", Some(&e.to_string()))
                .await?;
            return Err(e.into());
        }
    };

    // `orient` can tell a sideways page from an upright one geometrically, but
    // not upright from upside-down — both have the same line spacing. So the
    // transcription is checked against the dictionary and the page turned over
    // if it came out as shapes rather than words.
    let raw = match state.dict.as_deref() {
        Some(dict) if !ocr::legible::reads_as_language(&raw, dict) => {
            match preprocess::turn_page_over(&image_path) {
                Ok(turned) => client
                    .ocr(
                        &settings.ocr_model,
                        turned,
                        &reading_core::ollama::OcrOptions::default(),
                        |_| {},
                    )
                    .await
                    .unwrap_or(raw),
                Err(_) => raw,
            }
        }
        _ => raw,
    };

    // Read off the page rather than assigned by import order: photograph pages
    // out of sequence, or re-shoot a blurry one, and every number after it
    // would otherwise be wrong.
    let (page_no, source) = ocr::page_number::detect(
        &client,
        &settings.text_model,
        &settings.vision_model,
        &raw,
        Some(image),
    )
    .await;

    let era = Era::from_str_lossy(&book.era);
    let oracle = state
        .dict
        .as_deref()
        .map(|d| d as &dyn ocr::segment::WordOracle);
    let blocks = ocr::blocks_from_raw(&raw, &book.title, era.normalization(), oracle);

    state
        .db
        .save_ocr_result(caller.id(), page_id, &settings.ocr_model, &raw, &blocks)
        .await?;
    state
        .db
        .set_page_number(caller.id(), page_id, page_no, source.as_str())
        .await?;

    Ok(Json(TranscriptionResult {
        blocks: state.db.list_blocks(caller.id(), page_id).await?,
        page_no,
        page_no_source: source.as_str().to_string(),
    }))
}

#[derive(Deserialize)]
pub struct DocumentImport {
    /// A file path on the server, or a URL.
    pub source: String,
}

#[derive(Serialize)]
pub struct DocumentResult {
    pub pages_added: usize,
    pub blocks_added: usize,
    pub first_page_id: Option<i64>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub kind: String,
}

/// Import an EPUB, a PDF, or a web page.
///
/// No OCR: these are already text, and a vision model would be slower and
/// worse at reading them.
pub async fn import_document(
    State(state): State<AppState>,
    caller: Caller,
    Path(book_id): Path<i64>,
    Json(body): Json<DocumentImport>,
) -> ApiResult<Json<DocumentResult>> {
    let book = state.db.get_book(caller.id(), book_id).await?;

    if state
        .db
        .has_source(caller.id(), book_id, &body.source)
        .await?
    {
        return Err(ApiError(AppError::Invalid(
            "that document is already in this book".into(),
        )));
    }

    let kind = SourceKind::from_source(&body.source);
    let source = body.source.clone();

    let document = match kind {
        SourceKind::Web => reading_core::ingest::html::fetch(&source).await?,
        SourceKind::Epub => {
            let path = std::path::PathBuf::from(&source);
            tokio::task::spawn_blocking(move || reading_core::ingest::epub_source::read(&path))
                .await
                .map_err(|e| ApiError(AppError::Other(anyhow::anyhow!("import panicked: {e}"))))??
        }
        SourceKind::Pdf => {
            let path = std::path::PathBuf::from(&source);
            tokio::task::spawn_blocking(move || reading_core::ingest::pdf_source::read(&path))
                .await
                .map_err(|e| ApiError(AppError::Other(anyhow::anyhow!("import panicked: {e}"))))??
        }
        SourceKind::Photo => {
            return Err(ApiError(AppError::Invalid(
                "that looks like a photograph; import it as a page".into(),
            )))
        }
    };

    let era = Era::from_str_lossy(&book.era);

    // Segmentation first, insertion second, and the dictionary borrow confined
    // to this block. `&dyn WordOracle` carries no Send bound, so holding one
    // across an await would make this whole handler's future non-Send and
    // axum would refuse it — a compile error that reads as a mysterious
    // "trait not satisfied" on the route rather than as anything to do with
    // the dictionary.
    let segmented: Vec<(Option<String>, Option<i64>, Vec<reading_core::models::DraftBlock>)> = {
        let oracle = state
            .dict
            .as_deref()
            .map(|d| d as &dyn ocr::segment::WordOracle);

        document
            .pages
            .iter()
            .filter_map(|page| {
                // The same segmentation a transcription gets: it settles
                // headings, normalises archaic spelling for older texts, and
                // repairs hyphens.
                let joined = page.paragraphs.join("\n\n");
                let mut blocks =
                    ocr::blocks_from_raw(&joined, &book.title, era.normalization(), oracle);

                // A chapter title from the source is a heading, not something
                // to ask the reader to summarise.
                if let Some(label) = &page.label {
                    blocks.insert(
                        0,
                        reading_core::models::DraftBlock {
                            kind: reading_core::models::BlockKind::Heading,
                            text_raw: label.clone(),
                            text_norm: label.clone(),
                        },
                    );
                }

                if blocks.is_empty() {
                    None
                } else {
                    Some((page.label.clone(), page.page_no, blocks))
                }
            })
            .collect()
    };

    let mut first_page_id = None;
    let mut blocks_added = 0usize;
    let mut pages_added = 0usize;

    for (label, page_no, blocks) in &segmented {
        let page_id = state
            .db
            .add_text_page(
                caller.id(),
                book_id,
                kind.as_str(),
                label.as_deref(),
                &body.source,
                *page_no,
                blocks,
            )
            .await?;

        first_page_id.get_or_insert(page_id);
        blocks_added += blocks.len();
        pages_added += 1;
    }

    if pages_added == 0 {
        return Err(ApiError(AppError::Invalid(
            "no readable text came out of that document".into(),
        )));
    }

    Ok(Json(DocumentResult {
        pages_added,
        blocks_added,
        first_page_id,
        title: document.title,
        author: document.author,
        kind: kind.as_str().to_string(),
    }))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
