//! Tauri IPC surface.
//!
//! Commands are thin: they translate between the frontend and the modules that
//! hold the real logic. Anything worth testing lives in `ocr`, `coach`, or
//! `db`, not here.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::coach;
use crate::db::{Db, SpineEntry};
use crate::dict::{ContextualSense, Dictionary, Lookup};
use crate::error::{AppError, Result};
use crate::models::{Block, Book, Era, Page};
use crate::ocr::segment::WordOracle;
use crate::ocr::{self, preprocess};
use crate::ollama::{
    OcrOptions, OllamaClient, DEFAULT_OCR_MODEL, DEFAULT_TEXT_MODEL, DEFAULT_VISION_MODEL,
};

pub struct AppState {
    pub db: Arc<Db>,
    /// Behind a lock because the address is configurable: pointing the app at
    /// a different machine replaces the client rather than restarting.
    pub ollama: std::sync::RwLock<OllamaClient>,
    pub settings: std::sync::Mutex<Settings>,
    /// Where page images live.
    pub library_dir: PathBuf,
    /// Absent when `scripts/build-dictionary.mjs` has not been run. The app
    /// still works without it: hyphenation falls back to a case heuristic and
    /// word lookup is unavailable, rather than the whole thing failing to start.
    pub dict: Option<Arc<Dictionary>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Where Ollama is. Not necessarily this machine: inference can be
    /// offloaded to a home server or a rented box with a real GPU, which for
    /// a laptop with 8GB of VRAM is the difference between a 4B model and a
    /// 30B one.
    pub ollama_host: String,
    /// Bearer token for a hosted server. Empty for a local Ollama, which
    /// wants no authentication at all.
    pub ollama_api_key: String,
    pub ocr_model: String,
    pub text_model: String,
    /// Reads the page number off the image when the transcription has none.
    pub vision_model: String,
    /// How long the server holds a model after a request — "30m", "2h", or
    /// "-1" to never unload. Sent with every call, because a value on the
    /// request overrides the server's own `OLLAMA_KEEP_ALIVE`.
    pub keep_alive: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ollama_host: crate::ollama::DEFAULT_HOST.to_string(),
            ollama_api_key: String::new(),
            ocr_model: DEFAULT_OCR_MODEL.to_string(),
            text_model: DEFAULT_TEXT_MODEL.to_string(),
            vision_model: DEFAULT_VISION_MODEL.to_string(),
            keep_alive: crate::ollama::DEFAULT_KEEP_ALIVE.to_string(),
        }
    }
}

impl Settings {
    /// Read from the database, falling back to the defaults for anything
    /// unset — which is every key on a fresh install.
    pub fn load(db: &Db) -> Self {
        let mut settings = Settings::default();
        let get = |key: &str| db.get_setting(key).ok().flatten();

        if let Some(v) = get("ollama_host") {
            settings.ollama_host = v;
        }
        if let Some(v) = get("ollama_api_key") {
            settings.ollama_api_key = v;
        }
        if let Some(v) = get("ocr_model") {
            settings.ocr_model = v;
        }
        if let Some(v) = get("text_model") {
            settings.text_model = v;
        }
        if let Some(v) = get("vision_model") {
            settings.vision_model = v;
        }
        if let Some(v) = get("keep_alive") {
            settings.keep_alive = v;
        }
        settings
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("ollama_host", &self.ollama_host)?;
        db.set_setting("ollama_api_key", &self.ollama_api_key)?;
        db.set_setting("ocr_model", &self.ocr_model)?;
        db.set_setting("text_model", &self.text_model)?;
        db.set_setting("vision_model", &self.vision_model)?;
        db.set_setting("keep_alive", &self.keep_alive)?;
        Ok(())
    }

    /// Tidy an address typed by a person.
    ///
    /// `192.168.1.50:11434` and `http://192.168.1.50:11434/` should both work;
    /// requiring the scheme and forbidding a trailing slash would be a
    /// pointless way to fail.
    pub fn normalise_host(input: &str) -> String {
        let trimmed = input.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return crate::ollama::DEFAULT_HOST.to_string();
        }
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return trimmed.to_string();
        }
        // A bare host or host:port means plain HTTP, as Ollama serves.
        format!("http://{trimmed}")
    }
}

impl AppState {
    fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// A snapshot of the client. Cloning is cheap — `reqwest::Client` is an
    /// Arc internally — and it avoids holding the lock across an await.
    fn ollama(&self) -> OllamaClient {
        self.ollama
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaStatus {
    pub reachable: bool,
    pub models: Vec<String>,
    pub ocr_model_ready: bool,
    pub text_model_ready: bool,
    pub message: String,
}

/// Checked at startup so a missing Ollama produces one clear message rather
/// than a confusing failure on the reader's first page.
#[tauri::command]
pub async fn check_ollama(state: State<'_, AppState>) -> Result<OllamaStatus> {
    let settings = state.settings();
    match state.ollama().health().await {
        Ok(models) => {
            let has = |want: &str| {
                models
                    .iter()
                    .any(|m| m == want || m.split(':').next() == Some(want))
            };
            let ocr_ready = has(&settings.ocr_model);
            let text_ready = has(&settings.text_model);
            let mut missing = Vec::new();
            if !ocr_ready {
                missing.push(settings.ocr_model.clone());
            }
            if !text_ready {
                missing.push(settings.text_model.clone());
            }
            let message = if missing.is_empty() {
                "Ollama is running and both models are installed.".to_string()
            } else {
                format!(
                    "Ollama is running, but these models are missing: {}. Install with: ollama pull {}",
                    missing.join(", "),
                    missing.join(" && ollama pull ")
                )
            };
            Ok(OllamaStatus {
                reachable: true,
                models,
                ocr_model_ready: ocr_ready,
                text_model_ready: text_ready,
                message,
            })
        }
        Err(e) => Ok(OllamaStatus {
            reachable: false,
            models: Vec::new(),
            ocr_model_ready: false,
            text_model_ready: false,
            message: auth_hint(&e).unwrap_or_else(|| e.to_string()),
        }),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings())
}

/// The defaults, so the settings window can offer to restore them.
#[tauri::command]
pub fn default_settings() -> Settings {
    Settings::default()
}

/// Save settings, rebuilding the Ollama client if the server changed.
///
/// Pointing the app at another machine takes effect immediately — the client
/// is replaced rather than the app restarted, so a reader can move inference
/// to a home server and carry on reading.
#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    let cleaned = Settings {
        ollama_host: Settings::normalise_host(&settings.ollama_host),
        ollama_api_key: settings.ollama_api_key.trim().to_string(),
        ocr_model: non_empty(&settings.ocr_model, DEFAULT_OCR_MODEL),
        text_model: non_empty(&settings.text_model, DEFAULT_TEXT_MODEL),
        vision_model: non_empty(&settings.vision_model, DEFAULT_VISION_MODEL),
        keep_alive: non_empty(&settings.keep_alive, crate::ollama::DEFAULT_KEEP_ALIVE),
    };
    cleaned.save(&state.db)?;

    let previous = state.settings();
    // Key and keep-alive are both baked into the client, so any of the three
    // changing means it has to be rebuilt.
    let server_changed = previous.ollama_host != cleaned.ollama_host
        || previous.ollama_api_key != cleaned.ollama_api_key
        || previous.keep_alive != cleaned.keep_alive;
    *state.settings.lock().unwrap_or_else(|e| e.into_inner()) = cleaned.clone();

    if server_changed {
        *state.ollama.write().unwrap_or_else(|e| e.into_inner()) =
            OllamaClient::new(cleaned.ollama_host.clone(), Some(&cleaned.ollama_api_key))
                .with_keep_alive(&cleaned.keep_alive);
    }
    Ok(cleaned)
}

/// The models installed on a server, for choosing between rather than typing.
///
/// Takes the address and key rather than using the saved ones, so the settings
/// window can list what is on a machine the reader has typed but not yet
/// committed to. Falls back to the configured server when no address is given.
#[tauri::command]
pub async fn list_models(
    state: State<'_, AppState>,
    host: Option<String>,
    api_key: Option<String>,
) -> Result<Vec<crate::ollama::ModelInfo>> {
    let client = match host {
        Some(h) if !h.trim().is_empty() => {
            OllamaClient::new(Settings::normalise_host(&h), api_key.as_deref())
        }
        _ => state.ollama(),
    };
    client.installed_models().await
}

/// Try a server without committing to it, so the settings window can say
/// whether it is really there before the reader saves.
#[tauri::command]
pub async fn test_ollama_host(host: String, api_key: Option<String>) -> Result<OllamaStatus> {
    let host = Settings::normalise_host(&host);
    let client = OllamaClient::new(host.clone(), api_key.as_deref());

    Ok(match client.health().await {
        Ok(models) => OllamaStatus {
            message: format!("Reached {host} — {} models installed.", models.len()),
            reachable: true,
            ocr_model_ready: true,
            text_model_ready: true,
            models,
        },
        Err(e) => OllamaStatus {
            reachable: false,
            models: Vec::new(),
            ocr_model_ready: false,
            text_model_ready: false,
            message: auth_hint(&e).unwrap_or_else(|| e.to_string()),
        },
    })
}

/// Say what a rejected request actually means.
///
/// The server is plainly reachable when it answers 401 or 403, so reporting it
/// as unreachable would send the reader to check their network when the
/// problem is the key.
fn auth_hint(e: &crate::error::AppError) -> Option<String> {
    match e {
        crate::error::AppError::OllamaStatus { status: 401, .. } => Some(
            "The server answered, but rejected the API key. Check the key, or clear it if the server does not want one."
                .to_string(),
        ),
        crate::error::AppError::OllamaStatus { status: 403, .. } => Some(
            "The server answered, but refused this key. It may lack access to the models, or have expired."
                .to_string(),
        ),
        _ => None,
    }
}

fn non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[tauri::command]
pub fn list_books(state: State<'_, AppState>) -> Result<Vec<Book>> {
    state.db.list_books()
}

#[tauri::command]
pub fn create_book(
    state: State<'_, AppState>,
    title: String,
    author: Option<String>,
    era: String,
) -> Result<i64> {
    if title.trim().is_empty() {
        return Err(AppError::Invalid("a book needs a title".into()));
    }
    state
        .db
        .create_book(title.trim(), author.as_deref(), &era)
}

/// What a book contains, so a deletion prompt can say what it costs.
#[tauri::command]
pub fn book_stats(state: State<'_, AppState>, book_id: i64) -> Result<crate::db::BookStats> {
    state.db.book_stats(book_id)
}

/// Delete a book, its pages, and its images.
///
/// The rows cascade from `books`; the photographs do not, and leaving several
/// hundred megabytes behind for a book the reader deleted would be its own
/// kind of bug.
#[tauri::command]
pub fn delete_book(state: State<'_, AppState>, book_id: i64) -> Result<()> {
    state.db.delete_book(book_id)?;

    // Built from the library root and the id, never from stored text, so this
    // cannot be pointed anywhere else.
    let book_dir = state.library_dir.join(format!("book-{book_id}"));
    if book_dir.is_dir() {
        // The rows are already gone; a file that will not delete should not
        // leave the book half-removed.
        let _ = std::fs::remove_dir_all(&book_dir);
    }
    Ok(())
}

/// Remember where the reader stopped, so closing the app keeps their place.
#[tauri::command]
pub fn set_last_page(
    state: State<'_, AppState>,
    book_id: i64,
    page_id: Option<i64>,
) -> Result<()> {
    state.db.set_last_page(book_id, page_id)
}

/// Which page to open a book at.
#[tauri::command]
pub fn resume_page(state: State<'_, AppState>, book_id: i64) -> Result<Option<i64>> {
    state.db.resume_page(book_id)
}

#[tauri::command]
pub fn list_pages(state: State<'_, AppState>, book_id: i64) -> Result<Vec<Page>> {
    state.db.list_pages(book_id)
}

#[tauri::command]
pub fn list_blocks(state: State<'_, AppState>, page_id: i64) -> Result<Vec<Block>> {
    state.db.list_blocks(page_id)
}

#[tauri::command]
pub fn edit_block(state: State<'_, AppState>, block_id: i64, text: String) -> Result<()> {
    state.db.edit_block(block_id, text.trim())
}

/// Remove a transcription artefact.
#[tauri::command]
pub fn delete_block(state: State<'_, AppState>, block_id: i64) -> Result<()> {
    state.db.delete_block(block_id)
}

/// Reclassify a block — prose, heading, quotation.
#[tauri::command]
pub fn set_block_kind(state: State<'_, AppState>, block_id: i64, kind: String) -> Result<()> {
    state.db.set_block_kind(block_id, &kind)
}

/// SHA-256 of the archival image, hex encoded.
///
/// Taken over the archival bytes rather than the source file, so the same
/// photograph is recognised whatever it was named or whichever format it
/// arrived in.
fn image_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    /// One id per page created. A photograph of an open book yields two.
    pub page_ids: Vec<i64>,
    /// The page to open: the left half of a spread, or the only page.
    pub page_id: i64,
    /// Set when the file had to be converted (an iPhone HEIC, a scanner TIFF),
    /// so the UI can tell the reader rather than silently changing their file.
    pub converted_from: Option<String>,
    /// True when the photograph was recognised as an open book and split.
    pub was_spread: bool,
}

/// A step in a long-running job, reported to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct JobProgress {
    /// Machine-readable stage, for choosing an icon.
    pub stage: String,
    /// What to actually show the reader.
    pub detail: String,
    pub step: u32,
    pub total: u32,
}

pub fn emit_progress(app: &AppHandle, stage: &str, detail: &str, step: u32, total: u32) {
    let _ = app.emit(
        "job-progress",
        JobProgress {
            stage: stage.to_string(),
            detail: detail.to_string(),
            step,
            total,
        },
    );
}

/// Import a photograph: store a viewable full-resolution copy, write a
/// bounded copy for the model, and register the page as awaiting OCR.
///
/// HEIC — what every iPhone produces by default — is transcoded to JPEG here.
/// A WebView cannot render HEIC, so without this the reader would import a
/// page and then be unable to see it.
///
/// Async, and the work happens on a blocking thread. As a plain synchronous
/// command this ran on the main thread, and decoding a 12-megapixel HEIC,
/// resizing it, and re-encoding two JPEGs froze the window long enough for
/// Windows to mark it "not responding".
#[tauri::command]
pub async fn import_page(
    app: AppHandle,
    state: State<'_, AppState>,
    book_id: i64,
    source_path: String,
) -> Result<ImportResult> {
    let db = state.db.clone();
    let library_dir = state.library_dir.clone();

    tauri::async_runtime::spawn_blocking(move || {
        import_page_blocking(&app, &db, &library_dir, book_id, source_path)
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("import failed to run: {e}")))?
}

fn import_page_blocking(
    app: &AppHandle,
    db: &Db,
    library_dir: &std::path::Path,
    book_id: i64,
    source_path: String,
) -> Result<ImportResult> {
    emit_progress(app, "reading", "Reading the file", 1, 4);
    let bytes = std::fs::read(&source_path)?;

    // A photograph of an open book becomes two pages, one per leaf.
    emit_progress(app, "preparing", "Preparing the image", 2, 4);
    let imported =
        preprocess::prepare_import_pages(&bytes, &preprocess::PreprocessOptions::default())
            .map_err(|e| match e {
                AppError::Invalid(msg) => AppError::Invalid(format!("{source_path}: {msg}")),
                other => other,
            })?;

    let was_spread = imported.len() > 1;
    let converted_from = imported[0].converted_from.map(str::to_string);

    if was_spread {
        emit_progress(
            app,
            "splitting",
            "Open book detected — splitting at the spine",
            2,
            4,
        );
    }

    // Hash every half and check them all before writing anything, so a
    // re-imported spread is rejected whole rather than half-added.
    emit_progress(app, "checking", "Checking for a duplicate", 3, 4);
    let hashes: Vec<String> = imported.iter().map(|i| image_hash(&i.archival)).collect();
    for hash in &hashes {
        if let Some(existing) = db.page_with_hash(book_id, hash)? {
            return Err(AppError::Invalid(match existing.1 {
                Some(n) => format!("You have already imported this photograph — it is page {n}."),
                None => "You have already imported this photograph.".to_string(),
            }));
        }
    }

    emit_progress(app, "saving", "Saving to your library", 4, 4);
    let book_dir = library_dir.join(format!("book-{book_id}"));
    std::fs::create_dir_all(&book_dir)?;

    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let mut page_ids = Vec::with_capacity(imported.len());

    for (image, hash) in imported.iter().zip(&hashes) {
        let suffix = image.side.map(|s| format!("-{}", s.as_str())).unwrap_or_default();

        // This copy stands in for the photograph itself. It matters because
        // the OCR model silently modernises archaic spelling and ligatures, so
        // the image — not the transcription — is the record of what was printed.
        let orig = book_dir.join(format!("{stamp}{suffix}-orig.{}", image.archival_ext));
        std::fs::write(&orig, &image.archival)?;

        let proc_path = book_dir.join(format!("{stamp}{suffix}-proc.jpg"));
        std::fs::write(&proc_path, &image.processed)?;

        let page_id = db.add_page(
            book_id,
            hash,
            &orig.to_string_lossy(),
            &proc_path.to_string_lossy(),
        )?;

        // Record which photograph this half came from and which side it is.
        // Facing pages are consecutive, and knowing the pairing is what lets a
        // misread folio be corrected from its partner.
        if let Some(side) = image.side {
            db.set_page_source_ref(page_id, &format!("{stamp}:{}", side.as_str()))?;
        }
        page_ids.push(page_id);
    }

    Ok(ImportResult {
        page_id: page_ids[0],
        page_ids,
        converted_from,
        was_spread,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrProgress {
    pub page_id: i64,
    pub chunk: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentImport {
    pub pages_added: usize,
    pub blocks_added: usize,
    pub first_page_id: Option<i64>,
    /// Title the document declared, offered so the reader can name the book.
    pub title: Option<String>,
    pub author: Option<String>,
    pub kind: String,
}

/// Import an EPUB, a PDF, or a web page.
///
/// These sources arrive as text, so they skip the vision model entirely and go
/// straight to paragraph blocks. Everything after that — the sentence pass,
/// the coach, the dictionary — is identical to a photographed page.
#[tauri::command]
pub async fn import_document(
    app: AppHandle,
    state: State<'_, AppState>,
    book_id: i64,
    source: String,
) -> Result<DocumentImport> {
    use crate::ingest::{html, SourceKind};

    let kind = SourceKind::from_source(&source);
    if kind == SourceKind::Photo {
        return Err(AppError::Invalid(
            "that looks like an image — use 'Add a page photo' instead".into(),
        ));
    }

    if state.db.has_source(book_id, &source)? {
        return Err(AppError::Invalid(
            "you have already imported this into this book".into(),
        ));
    }

    emit_progress(&app, "reading", describe(kind), 1, 3);

    // Fetching is async; file parsing is CPU-bound and goes to a blocking
    // thread so the window stays responsive.
    let document = match kind {
        SourceKind::Web => html::fetch(&source).await?,
        _ => {
            let path = std::path::PathBuf::from(&source);
            tauri::async_runtime::spawn_blocking(move || match kind {
                SourceKind::Epub => crate::ingest::epub_source::read(&path),
                SourceKind::Pdf => crate::ingest::pdf_source::read(&path),
                _ => unreachable!("photo and web handled above"),
            })
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("import failed to run: {e}")))??
        }
    };

    emit_progress(&app, "segmenting", "Finding the paragraphs", 2, 3);

    let book = state.db.get_book(book_id)?;
    let era = Era::from_str_lossy(&book.era);
    let oracle = state.dict.as_deref().map(|d| d as &dyn WordOracle);

    let mut pages_added = 0;
    let mut blocks_added = 0;
    let mut first_page_id = None;

    for page in &document.pages {
        // Run the same segmentation as a transcription: it settles headings,
        // normalises archaic spelling for older texts, and repairs hyphens.
        let joined = page.paragraphs.join("\n\n");
        let mut blocks = ocr::blocks_from_raw(&joined, &book.title, era.normalization(), oracle);

        // A chapter title from the source is a heading, not something to
        // summarise.
        if let Some(label) = &page.label {
            blocks.insert(
                0,
                crate::models::DraftBlock {
                    kind: crate::models::BlockKind::Heading,
                    text_raw: label.clone(),
                    text_norm: label.clone(),
                },
            );
        }
        if blocks.is_empty() {
            continue;
        }

        let page_id = state.db.add_text_page(
            book_id,
            kind.as_str(),
            page.label.as_deref(),
            &source,
            page.page_no,
            &blocks,
        )?;
        first_page_id.get_or_insert(page_id);
        pages_added += 1;
        blocks_added += blocks.len();
    }

    if pages_added == 0 {
        return Err(AppError::Invalid(
            "nothing readable was found in that document".into(),
        ));
    }

    emit_progress(&app, "saving", "Saving to your library", 3, 3);

    Ok(DocumentImport {
        pages_added,
        blocks_added,
        first_page_id,
        title: document.title,
        author: document.author,
        kind: kind.as_str().to_string(),
    })
}

fn describe(kind: crate::ingest::SourceKind) -> &'static str {
    use crate::ingest::SourceKind::*;
    match kind {
        Epub => "Opening the ebook",
        Pdf => "Reading the PDF",
        Web => "Fetching the page",
        Photo => "Reading the file",
    }
}

/// Transcribe a page and segment it into paragraph blocks.
///
/// Emits `ocr-progress` as text arrives so the reader watches the page appear
/// rather than a spinner.
#[tauri::command]
pub async fn run_ocr(
    app: AppHandle,
    state: State<'_, AppState>,
    page_id: i64,
    book_id: i64,
) -> Result<Vec<Block>> {
    let settings = state.settings();
    let client = state.ollama();
    let book = state.db.get_book(book_id)?;
    let era = Era::from_str_lossy(&book.era);

    let page = state
        .db
        .list_pages(book_id)?
        .into_iter()
        .find(|p| p.id == page_id)
        .ok_or_else(|| AppError::NotFound(format!("page {page_id}")))?;

    let image_path = page.image_proc.unwrap_or(page.image_orig);
    let image = std::fs::read(&image_path)?;
    // Kept for the folio reader, which may need to look at the page after the
    // transcription has consumed the original buffer.
    let image_for_folio = image.clone();

    state.db.set_page_status(page_id, "running", None)?;
    emit_progress(&app, "transcribing", "Reading the page", 1, 3);

    let raw = client
        .ocr(
            &settings.ocr_model,
            image,
            &OcrOptions::default(),
            |chunk| {
                let _ = app.emit(
                    "ocr-progress",
                    OcrProgress {
                        page_id,
                        chunk: chunk.to_string(),
                    },
                );
            },
        )
        .await;

    let mut raw = match raw {
        Ok(r) => r,
        Err(e) => {
            state.db.set_page_status(page_id, "failed", Some(&e.to_string()))?;
            return Err(e);
        }
    };

    // Orientation proposes a quarter turn but cannot tell which way up the
    // result is. This is where that gets checked: an upside-down page does not
    // transcribe badly, it transcribes to strings that are not words, and the
    // dictionary can say so. One retry, only when the evidence says the page
    // came out as nonsense.
    if let Some(dict) = state.dict.as_deref() {
        if !ocr::legible::reads_as_language(&raw, dict) {
            emit_progress(&app, "transcribing", "That came out garbled — turning the page over", 1, 3);

            if let Ok(turned) = preprocess::turn_page_over(&image_path) {
                let retry = client
                    .ocr(&settings.ocr_model, turned, &OcrOptions::default(), |chunk| {
                        let _ = app.emit(
                            "ocr-progress",
                            OcrProgress {
                                page_id,
                                chunk: chunk.to_string(),
                            },
                        );
                    })
                    .await;

                // Keep the better of the two readings.
                if let Ok(retry) = retry {
                    if ocr::legible::reads_as_language(&retry, dict) {
                        raw = retry;
                    }
                }
            }
        }
    }

    // The dictionary settles the two genuinely ambiguous decisions in
    // segmentation: whether a line-break hyphen joins a split word or a real
    // compound, and whether an `f` is really a misread long-s. Without it both
    // fall back to heuristics.
    // Read the page number off the page itself. Import order is unreliable:
    // photograph pages out of sequence or re-shoot a blurry one and every
    // number after it is wrong.
    emit_progress(&app, "numbering", "Finding the page number", 2, 3);
    let (page_no, source) = ocr::page_number::detect(
        &client,
        &settings.text_model,
        &settings.vision_model,
        &raw,
        // The image is the last resort, for pages whose folio the OCR model
        // simply declines to transcribe.
        Some(image_for_folio),
    )
    .await;
    state
        .db
        .set_page_number(page_id, page_no, source.as_str())?;

    // Facing pages from one photograph are consecutive. Once both halves have
    // been read, that constraint corrects a misread folio — on a real import
    // the right-hand page read as 1 when it was 3, sorting it ahead of its own
    // left-hand page and under the wrong chapter.
    if let Some((sibling_id, sibling_no, side)) = state.db.spread_sibling(page_id)? {
        let (left_id, left_no, right_id, right_no) = if side == "left" {
            (page_id, page_no, sibling_id, sibling_no)
        } else {
            (sibling_id, sibling_no, page_id, page_no)
        };

        let used = state
            .db
            .page_numbers_excluding(book_id, &[left_id, right_id])?;
        let (fixed_left, fixed_right) =
            ocr::page_number::reconcile_facing(left_no, right_no, &used);

        if fixed_left != left_no {
            state
                .db
                .set_page_number(left_id, fixed_left, source_for(fixed_left))?;
        }
        if fixed_right != right_no {
            state
                .db
                .set_page_number(right_id, fixed_right, source_for(fixed_right))?;
        }
    }

    emit_progress(&app, "segmenting", "Finding the paragraphs", 3, 3);
    let oracle = state.dict.as_deref().map(|d| d as &dyn WordOracle);
    let blocks = ocr::blocks_from_raw(&raw, &book.title, era.normalization(), oracle);
    state
        .db
        .save_ocr_result(page_id, &settings.ocr_model, &raw, &blocks)?;

    state.db.list_blocks(page_id)
}

/// Delete a page, its blocks, and its image files.
#[tauri::command]
pub fn delete_page(state: State<'_, AppState>, page_id: i64) -> Result<()> {
    for path in state.db.delete_page(page_id)? {
        // The row is already gone; a file that cannot be removed should not
        // fail the operation.
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Set a page number by hand, for a page that carries none or was misread.
#[tauri::command]
pub fn set_page_number(
    state: State<'_, AppState>,
    page_id: i64,
    page_no: Option<i64>,
) -> Result<()> {
    let source = if page_no.is_some() { "manual" } else { "unknown" };
    state.db.set_page_number(page_id, page_no, source)
}

#[derive(Debug, Clone, Serialize)]
pub struct CritiqueResult {
    pub summary_id: i64,
    pub feedback: coach::Feedback,
}

/// Save the reader's sentence and return coaching on it.
///
/// The sentence is stored before the model is consulted, so a coaching failure
/// never costs the reader their work.
#[tauri::command]
pub async fn critique_summary(
    state: State<'_, AppState>,
    block_id: i64,
    sentence: String,
    self_checked: bool,
) -> Result<CritiqueResult> {
    let sentence = sentence.trim().to_string();
    if sentence.is_empty() {
        return Err(AppError::Invalid("write a sentence first".into()));
    }

    let block = state.db.get_block(block_id)?;
    let summary_id = state.db.save_summary(block_id, None, &sentence, self_checked)?;

    let settings = state.settings();
    let client = state.ollama();
    let era = era_for_block(&state, &block)?;

    // Judge the summary against the whole paragraph, including the part that
    // sits on the neighbouring page. Marking a reader down for missing a point
    // made in text they were never shown would be worse than no coaching.
    let paragraph = build_paragraph_context(&state, block_id)?;

    let assessment = coach::critique(
        &client,
        &settings.text_model,
        &paragraph.text,
        &sentence,
        era,
    )
    .await?;

    let r = &assessment.rubric;
    state.db.save_critique(
        summary_id,
        r.verdict.as_str(),
        (r.covers.subject, r.covers.main_action, r.covers.reason_or_result),
        r.problem.as_str(),
        &r.steering_question,
        &settings.text_model,
    )?;

    Ok(CritiqueResult {
        summary_id,
        feedback: assessment.feedback,
    })
}

/// A worked example, only ever on explicit request.
#[tauri::command]
pub async fn get_exemplar(state: State<'_, AppState>, block_id: i64) -> Result<coach::Exemplar> {
    let block = state.db.get_block(block_id)?;
    let era = era_for_block(&state, &block)?;
    let settings = state.settings();
    let client = state.ollama();
    let paragraph = build_paragraph_context(&state, block_id)?;
    coach::exemplar(&client, &settings.text_model, &paragraph.text, era).await
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlineHeading {
    pub block_id: i64,
    pub text: String,
    /// chapter | section | minor
    pub level: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlinePage {
    pub page_id: i64,
    pub page_no: Option<i64>,
    pub ocr_status: String,
    pub source_kind: String,
    /// First line of prose, for recognising a page at a glance.
    pub preview: String,
    pub headings: Vec<OutlineHeading>,
    /// Paragraphs on the page, and how many the reader has summarised.
    pub paragraphs: i64,
    pub summarised: i64,
}

/// The book's structure, for navigation.
///
/// Assembled from the headings already found during segmentation rather than
/// from a table of contents, because a photographed page has none and an
/// EPUB's is usually a navigation document that extraction discarded as
/// furniture.
#[tauri::command]
pub fn book_outline(state: State<'_, AppState>, book_id: i64) -> Result<Vec<OutlinePage>> {
    use crate::ocr::outline::{fold_titles, heading_level, preview, Heading, HeadingLevel};

    let pages = state.db.list_pages(book_id)?;
    let mut out = Vec::with_capacity(pages.len());

    for page in pages {
        let blocks = state.db.list_blocks(page.id)?;

        // Fold each chapter's title into the marker above it: books set
        // "CHAPTER I." and "ON METHOD." on separate lines, and they are one
        // chapter.
        // A heading matching the page's declared division came from the
        // document's own table of contents, so it *is* structure — its level
        // should not be re-guessed from its wording. "The First Book of Moses:
        // Called Genesis" reads as an ordinary title and would otherwise rank
        // below the numbered sections inside it.
        let declared = page.source_ref.as_deref().map(str::trim);

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

        let paragraphs: Vec<&crate::models::Block> =
            blocks.iter().filter(|b| b.kind == "paragraph").collect();

        let summarised = paragraphs
            .iter()
            .filter(|b| {
                state
                    .db
                    .latest_summary(b.id)
                    .ok()
                    .flatten()
                    .is_some()
            })
            .count() as i64;

        out.push(OutlinePage {
            page_id: page.id,
            page_no: page.page_no,
            ocr_status: page.ocr_status,
            source_kind: page.source_kind,
            preview: paragraphs
                .first()
                .map(|b| preview(&b.text_norm, 70))
                .unwrap_or_default(),
            headings,
            paragraphs: paragraphs.len() as i64,
            summarised,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn summary_spine(state: State<'_, AppState>, book_id: i64) -> Result<Vec<SpineEntry>> {
    state.db.summary_spine(book_id)
}

// ---- dictionary --------------------------------------------------------

fn dictionary(state: &State<'_, AppState>) -> Result<Arc<Dictionary>> {
    state.dict.clone().ok_or_else(|| {
        AppError::NotFound(
            "the dictionary has not been built. Run: node scripts/build-dictionary.mjs".into(),
        )
    })
}

/// Layer one: instant, offline, no model involved.
///
/// Returns `None` rather than guessing when the word cannot be resolved. A
/// missing answer is far better than a confident wrong one.
#[tauri::command]
pub fn look_up_word(state: State<'_, AppState>, word: String) -> Result<Option<Lookup>> {
    dictionary(&state)?.lookup(&word)
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextResult {
    pub lookup: Lookup,
    pub in_context: ContextualSense,
}

/// Layer two: what the word means *in this sentence*.
///
/// The model chooses among senses already retrieved from the dictionary, so it
/// selects and paraphrases rather than defining from memory.
#[tauri::command]
pub async fn word_in_context(
    state: State<'_, AppState>,
    word: String,
    sentence: String,
    block_id: Option<i64>,
) -> Result<ContextResult> {
    let dict = dictionary(&state)?;
    let lookup = dict
        .lookup(&word)?
        .ok_or_else(|| AppError::NotFound(format!("\"{word}\" is not in the dictionary")))?;

    let settings = state.settings();
    let client = state.ollama();
    let in_context = crate::dict::sense_in_context(
        &client,
        &settings.text_model,
        &lookup.lemma,
        &sentence,
        &lookup.senses,
    )
    .await?;

    // Record it so the vocabulary list can be built from real encounters,
    // each with the sentence it was met in.
    if let Some(block_id) = block_id {
        if let Ok(book_id) = state
            .db
            .get_block(block_id)
            .and_then(|b| state.db.book_id_for_page(b.page_id))
        {
            let _ = state.db.record_lookup(
                book_id,
                block_id,
                &word,
                &lookup.lemma,
                &sentence,
                &in_context.plain_meaning,
            );
        }
    }

    Ok(ContextResult { lookup, in_context })
}

#[tauri::command]
pub fn vocabulary(state: State<'_, AppState>, book_id: i64) -> Result<Vec<crate::db::VocabEntry>> {
    state.db.vocabulary(book_id)
}

/// Frontend mirror of the one-sentence rule, so the check is identical on both
/// sides rather than reimplemented in TypeScript.
#[tauri::command]
pub fn is_multiple_sentences(text: String) -> bool {
    coach::counts_as_multiple_sentences(&text)
}

#[derive(Debug, Clone, Serialize)]
pub struct SentenceView {
    pub ordinal: i64,
    pub text: String,
    /// The reader's note on this sentence, if they have written one.
    pub note: Option<String>,
}

/// The sentences of a paragraph, with any notes already written against them.
///
/// This is the tier that comes before compressing the whole paragraph: working
/// through a long periodic sentence one clause at a time is where an older
/// book actually becomes legible.
#[tauri::command]
pub fn block_sentences(state: State<'_, AppState>, block_id: i64) -> Result<Vec<SentenceView>> {
    // The stitched paragraph, so a sentence broken across the page break is
    // presented whole rather than as two fragments.
    let paragraph = build_paragraph_context(&state, block_id)?;
    let notes = state.db.sentence_summaries(block_id)?;

    Ok(coach::split_sentences(&paragraph.text)
        .into_iter()
        .enumerate()
        .map(|(i, text)| SentenceView {
            ordinal: i as i64,
            text,
            note: notes
                .iter()
                .find(|n| n.ordinal == i as i64)
                .map(|n| n.text.clone()),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct ParagraphContext {
    /// The paragraph to actually work on: stitched with its continuation on
    /// the neighbouring page where one exists.
    pub text: String,
    pub spans_pages: bool,
    /// Set when this paragraph began on the previous page.
    pub continued_from_page: Option<i64>,
    /// Set when it finishes on the next page.
    pub continues_on_page: Option<i64>,
    /// It runs on, but the page that finishes it has not been imported. The
    /// reader is looking at a fragment and should be told so rather than asked
    /// to summarise half a thought.
    pub incomplete: bool,
}

/// Assemble a paragraph across a page break.
///
/// A page ends where the paper runs out, not where the argument does. Asking
/// the reader for a one-sentence summary of "endeavor to bring all the facts
/// of revelation into systematic order" is asking them to compress half a
/// sentence — the method cannot work on a fragment.
///
/// Stitching is computed on demand rather than stored, so it stays correct
/// when a page is re-transcribed, renumbered, or deleted.
#[tauri::command]
pub fn paragraph_context(state: State<'_, AppState>, block_id: i64) -> Result<ParagraphContext> {
    build_paragraph_context(&state, block_id)
}

fn build_paragraph_context(
    state: &State<'_, AppState>,
    block_id: i64,
) -> Result<ParagraphContext> {
    use crate::ocr::segment::{ends_mid_sentence, starts_mid_sentence, stitch_across_pages};

    let block = state.db.get_block(block_id)?;
    let oracle = state.dict.as_deref().map(|d| d as &dyn WordOracle);

    let mut text = block.text_norm.clone();
    let mut continued_from_page = None;
    let mut continues_on_page = None;
    let mut incomplete = false;

    // Backwards: does this paragraph open mid-sentence?
    if starts_mid_sentence(&block.text_norm) {
        let is_first_prose = state
            .db
            .edge_paragraph(block.page_id, true)?
            .is_some_and(|b| b.id == block_id);

        if is_first_prose {
            if let Some(prev_page) = state.db.previous_page_id(block.page_id)? {
                if let Some(tail) = state.db.edge_paragraph(prev_page, false)? {
                    if ends_mid_sentence(&tail.text_norm) {
                        text = stitch_across_pages(&tail.text_norm, &text, oracle);
                        continued_from_page = Some(prev_page);
                    }
                }
            }
        }
    }

    // Forwards: does it break off mid-sentence?
    if ends_mid_sentence(&block.text_norm) {
        let is_last_prose = state
            .db
            .edge_paragraph(block.page_id, false)?
            .is_some_and(|b| b.id == block_id);

        if is_last_prose {
            match state.db.next_page_id(block.page_id)? {
                Some(next_page) => match state.db.edge_paragraph(next_page, true)? {
                    Some(head) if starts_mid_sentence(&head.text_norm) => {
                        text = stitch_across_pages(&text, &head.text_norm, oracle);
                        continues_on_page = Some(next_page);
                    }
                    // The next page exists but does not continue this
                    // paragraph, so nothing is missing.
                    _ => {}
                },
                // The page that finishes this thought has not been photographed.
                None => incomplete = true,
            }
        }
    }

    Ok(ParagraphContext {
        spans_pages: continued_from_page.is_some() || continues_on_page.is_some(),
        text,
        continued_from_page,
        continues_on_page,
        incomplete,
    })
}

/// Save the reader's note on one sentence.
///
/// No coaching here on purpose. The sentence tier is for the reader's own
/// working-out; the model is brought in once at the paragraph level, where the
/// judgement actually matters.
#[tauri::command]
pub fn save_sentence_note(
    state: State<'_, AppState>,
    block_id: i64,
    sentence_ordinal: i64,
    text: String,
) -> Result<i64> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::Invalid("write something first".into()));
    }
    state
        .db
        .save_summary(block_id, Some(sentence_ordinal), text, false)
}

fn source_for(page_no: Option<i64>) -> &'static str {
    if page_no.is_some() {
        "detected"
    } else {
        "unknown"
    }
}

fn era_for_block(state: &State<'_, AppState>, block: &Block) -> Result<Era> {
    let book_id = state.db.book_id_for_page(block.page_id)?;
    let book = state.db.get_book(book_id)?;
    Ok(Era::from_str_lossy(&book.era))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A person typing a server address should not have to know the scheme,
    /// and should not be punished for a trailing slash.
    #[test]
    fn an_address_typed_by_a_person_is_accepted() {
        let http = "http://192.168.1.50:11434";
        assert_eq!(Settings::normalise_host("192.168.1.50:11434"), http);
        assert_eq!(Settings::normalise_host("  192.168.1.50:11434/  "), http);
        assert_eq!(Settings::normalise_host(http), http);
        assert_eq!(Settings::normalise_host("http://192.168.1.50:11434/"), http);
    }

    #[test]
    fn https_is_left_alone() {
        // A tunnelled or hosted endpoint must keep its scheme.
        let url = "https://ollama.example.com";
        assert_eq!(Settings::normalise_host(url), url);
    }

    #[test]
    fn an_empty_address_falls_back_to_this_machine() {
        assert_eq!(
            Settings::normalise_host("   "),
            crate::ollama::DEFAULT_HOST
        );
    }

    #[test]
    fn a_blank_model_name_falls_back_rather_than_breaking_inference() {
        assert_eq!(non_empty("  ", DEFAULT_TEXT_MODEL), DEFAULT_TEXT_MODEL);
        assert_eq!(non_empty(" llama3 ", DEFAULT_TEXT_MODEL), "llama3");
    }

    #[test]
    fn settings_survive_a_round_trip_through_the_database() {
        let db = Db::open_in_memory().unwrap();
        let settings = Settings {
            ollama_host: "http://desktop.local:11434".into(),
            ollama_api_key: "sk-abc123".into(),
            ocr_model: "glm-ocr".into(),
            text_model: "qwen3:30b".into(),
            vision_model: "qwen3-vl:8b".into(),
            keep_alive: "-1".into(),
        };
        settings.save(&db).unwrap();

        let loaded = Settings::load(&db);
        assert_eq!(loaded.ollama_host, "http://desktop.local:11434");
        assert_eq!(loaded.text_model, "qwen3:30b");
        assert_eq!(loaded.ollama_api_key, "sk-abc123");
        assert_eq!(loaded.keep_alive, "-1");
    }

    /// Blank must not reach Ollama: it reads an empty keep_alive as zero and
    /// unloads the model the instant the request finishes, which would turn
    /// every page into a cold load.
    #[test]
    fn a_blank_keep_alive_falls_back_to_the_default() {
        let client = crate::ollama::OllamaClient::new("http://127.0.0.1:11434", None)
            .with_keep_alive("   ");
        assert_eq!(client.keep_alive(), crate::ollama::DEFAULT_KEEP_ALIVE);

        let pinned =
            crate::ollama::OllamaClient::new("http://127.0.0.1:11434", None).with_keep_alive("-1");
        assert_eq!(pinned.keep_alive(), "-1");
    }

    /// A local install has no key, and must not end up sending an empty
    /// bearer token — some proxies reject that outright.
    #[test]
    fn a_blank_api_key_is_no_key_at_all() {
        assert!(Settings::default().ollama_api_key.is_empty());
        // Construction must not panic on any of these.
        crate::ollama::OllamaClient::new("http://127.0.0.1:11434", None);
        crate::ollama::OllamaClient::new("http://127.0.0.1:11434", Some(""));
        crate::ollama::OllamaClient::new("http://127.0.0.1:11434", Some("   "));
        crate::ollama::OllamaClient::new("http://127.0.0.1:11434", Some("sk-abc123"));
        // A key that cannot be a header value is dropped, not fatal.
        crate::ollama::OllamaClient::new("http://127.0.0.1:11434", Some("bad\nkey"));
    }

    #[test]
    fn an_unset_database_yields_the_defaults() {
        let db = Db::open_in_memory().unwrap();
        let loaded = Settings::load(&db);
        assert_eq!(loaded.ollama_host, crate::ollama::DEFAULT_HOST);
        assert_eq!(loaded.ocr_model, DEFAULT_OCR_MODEL);
    }
}

/// Build the application state, creating the library directory on first run.
pub fn init_state(app: &AppHandle) -> Result<AppState> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Invalid(format!("no app data directory: {e}")))?;
    std::fs::create_dir_all(&data_dir)?;

    let library_dir = data_dir.join("library");
    std::fs::create_dir_all(&library_dir)?;

    let db = Db::open(data_dir.join("library.sqlite"))?;
    let settings = Settings::load(&db);

    // Bundled as a Tauri resource. Missing it degrades the app rather than
    // stopping it, so a checkout that has not run the build script still runs.
    let dict = app
        .path()
        .resolve("resources/dict.sqlite", tauri::path::BaseDirectory::Resource)
        .ok()
        .and_then(|p| match Dictionary::open(&p) {
            Ok(d) => Some(Arc::new(d)),
            Err(e) => {
                eprintln!("dictionary unavailable ({e}); lookups and dictionary-checked hyphenation are disabled");
                None
            }
        });

    Ok(AppState {
        db: Arc::new(db),
        ollama: std::sync::RwLock::new(
            OllamaClient::new(settings.ollama_host.clone(), Some(&settings.ollama_api_key))
                .with_keep_alive(&settings.keep_alive),
        ),
        settings: std::sync::Mutex::new(settings),
        library_dir,
        dict,
    })
}
