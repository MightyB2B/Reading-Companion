//! The library database.
//!
//! One SQLite file holding books, pages, paragraph blocks, and everything the
//! reader writes. Guarded by a mutex rather than a pool: this is a
//! single-user desktop application, contention is nil, and a `Connection`
//! behind a `Mutex` is far simpler to reason about than a pool.

pub mod reading;
pub mod search;
pub mod scripture;
pub mod study;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, Result};
use crate::models::{Block, Book, DraftBlock, Page};

/// Bumped whenever the schema changes shape. Stored in SQLite's own
/// `user_version` so no bookkeeping table is needed.
///
/// v2: page numbers come from what the model reads on the page rather than
/// from import order, so `page_no` became nullable and lost its uniqueness
/// constraint, and `image_hash` was added to catch re-imports of the same
/// photograph.
/// v3: the reader works through a paragraph a sentence at a time before
/// compressing the whole paragraph, so a summary can now belong to one
/// sentence rather than to the paragraph.
/// v4: pages can come from an EPUB, a PDF, or a web page as well as a
/// photograph, so a page records where it came from.
/// v5: a book remembers the page you were last on, so closing the app does
/// not lose your place.
/// v6: settings are stored rather than held in memory, so the Ollama address
/// and model choices survive a restart.
/// v7: the study layer — notes and marks anchored to spans of text, the
/// author's own terms, standard-form argument reconstructions, verse
/// addressing, scripture citations found in any book, and workflows as data
/// rather than as hardcoded React.
/// v8: the library is searchable — an FTS5 index over every block — and a
/// note can be anchored in several places at once, which is what a selection
/// dragged across a page boundary has always meant.
const SCHEMA_VERSION: i64 = 8;

/// The v7 tables, shared verbatim between a fresh database and a migrated
/// one. Every statement is `IF NOT EXISTS`, so applying it twice is harmless
/// — which matters, because a database created by a build between these two
/// versions could have some of them already.
const MIGRATE_6_TO_7: &str = include_str!("v7.sql");

/// Same contract as v7: every statement idempotent, so it can be applied to a
/// fresh database and to a migrating one without branching.
const MIGRATE_7_TO_8: &str = include_str!("v8.sql");

const MIGRATE_5_TO_6: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// The default must be NULL — SQLite requires it of any column added with a
/// REFERENCES clause. `ON DELETE SET NULL` rather than CASCADE: deleting the
/// page you happened to stop on should not delete the book.
const MIGRATE_4_TO_5: &str = "
ALTER TABLE books ADD COLUMN last_page_id INTEGER
    REFERENCES pages(id) ON DELETE SET NULL;
";

/// `image_orig` keeps its NOT NULL: for a text source it holds the file path
/// or URL the page came from, which is the same thing it always meant — the
/// artefact this page was taken out of. Rebuilding the table to relax a
/// constraint that is still doing useful work would be risk for nothing.
const MIGRATE_3_TO_4: &str = "
ALTER TABLE pages ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'photo';
ALTER TABLE pages ADD COLUMN source_ref TEXT;
";

/// Null `sentence_ordinal` means the summary is of the whole paragraph, which
/// is what every existing row is.
const MIGRATE_2_TO_3: &str = "
ALTER TABLE summaries ADD COLUMN sentence_ordinal INTEGER;
";

const SCHEMA: &str = include_str!("schema.sql");

/// Rebuild `pages` to drop `UNIQUE (book_id, page_no)` and add the new
/// columns.
///
/// SQLite cannot drop a constraint in place, so the table is rebuilt. Foreign
/// keys are suspended for the duration: `blocks` references `pages(id)` with
/// ON DELETE CASCADE, and dropping the old table with them enforced would take
/// every block on every page with it.
const MIGRATE_1_TO_2: &str = "
PRAGMA foreign_keys = OFF;
BEGIN;

CREATE TABLE pages_new (
    id             INTEGER PRIMARY KEY,
    book_id        INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    -- Null until the model has read the page and told us its number.
    page_no        INTEGER,
    page_no_source TEXT    NOT NULL DEFAULT 'unknown',  -- detected|manual|unknown
    -- SHA-256 of the archival image, so the same photograph is not imported twice.
    image_hash     TEXT,
    image_orig     TEXT    NOT NULL,
    image_proc     TEXT,
    ocr_status     TEXT    NOT NULL DEFAULT 'pending',
    ocr_model      TEXT,
    ocr_raw        TEXT,
    ocr_error      TEXT,
    created_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO pages_new
    (id, book_id, page_no, page_no_source, image_hash, image_orig, image_proc,
     ocr_status, ocr_model, ocr_raw, ocr_error, created_at)
SELECT
    id, book_id, NULL, 'unknown', NULL, image_orig, image_proc,
    ocr_status, ocr_model, ocr_raw, ocr_error, created_at
FROM pages;

DROP TABLE pages;
ALTER TABLE pages_new RENAME TO pages;

CREATE INDEX idx_pages_book ON pages(book_id, page_no);
CREATE UNIQUE INDEX idx_pages_hash ON pages(book_id, image_hash);

COMMIT;
PRAGMA foreign_keys = ON;
";

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // WAL keeps a slow OCR write from blocking the reader's next page.
        // `journal_mode` returns a row, so it needs query_row not execute.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;

        let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            // The study layer is applied on top rather than folded into
            // schema.sql, so there is exactly one definition of these tables
            // and a fresh database is byte-for-byte what a migrated one
            // becomes.
            conn.execute_batch(MIGRATE_6_TO_7)?;
            conn.execute_batch(MIGRATE_7_TO_8)?;
            version = SCHEMA_VERSION;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 1 {
            // Migrate rather than refuse: by this point a reader may already
            // have pages and summaries they care about.
            conn.execute_batch(MIGRATE_1_TO_2)?;
            version = 2;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 2 {
            conn.execute_batch(MIGRATE_2_TO_3)?;
            version = 3;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 3 {
            conn.execute_batch(MIGRATE_3_TO_4)?;
            version = 4;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 4 {
            conn.execute_batch(MIGRATE_4_TO_5)?;
            version = 5;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 5 {
            conn.execute_batch(MIGRATE_5_TO_6)?;
            version = 6;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 6 {
            conn.execute_batch(MIGRATE_6_TO_7)?;
            version = 7;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version == 7 {
            conn.execute_batch(MIGRATE_7_TO_8)?;
            version = 8;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version != SCHEMA_VERSION {
            return Err(AppError::Invalid(format!(
                "library database is version {version}, this build expects {SCHEMA_VERSION}"
            )));
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned lock means another thread panicked mid-write. Recovering
        // the guard is better than cascading the panic through the UI.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- books ----------------------------------------------------------

    pub fn create_book(&self, title: &str, author: Option<&str>, era: &str) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO books (title, author, era) VALUES (?1, ?2, ?3)",
            params![title, author, era],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_books(&self) -> Result<Vec<Book>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, author, era, dict_corpus, last_page_id, created_at, updated_at
             FROM books ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Book {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    author: r.get(2)?,
                    era: r.get(3)?,
                    dict_corpus: r.get(4)?,
                    last_page_id: r.get(5)?,
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn get_book(&self, id: i64) -> Result<Book> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, title, author, era, dict_corpus, last_page_id, created_at, updated_at
             FROM books WHERE id = ?1",
            params![id],
            |r| {
                Ok(Book {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    author: r.get(2)?,
                    era: r.get(3)?,
                    dict_corpus: r.get(4)?,
                    last_page_id: r.get(5)?,
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("book {id}")))
    }

    /// Remember where the reader stopped.
    ///
    /// Written on every page turn, which is cheap: one indexed update against
    /// a WAL database, far below the cost of the turn itself.
    pub fn set_last_page(&self, book_id: i64, page_id: Option<i64>) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE books SET last_page_id = ?2 WHERE id = ?1",
            params![book_id, page_id],
        )?;
        Ok(())
    }

    /// Where to open a book: where the reader stopped, or its first page.
    ///
    /// Checked against the pages that actually exist, so a remembered page
    /// that has since been deleted or re-imported falls back rather than
    /// leaving the reader looking at nothing.
    pub fn resume_page(&self, book_id: i64) -> Result<Option<i64>> {
        let pages = self.list_pages(book_id)?;
        let remembered = self.get_book(book_id)?.last_page_id;

        Ok(remembered
            .filter(|id| pages.iter().any(|p| p.id == *id))
            .or_else(|| pages.first().map(|p| p.id)))
    }

    /// What a book contains, for a deletion prompt that says what it costs.
    pub fn book_stats(&self, book_id: i64) -> Result<BookStats> {
        let conn = self.lock();
        let count = |sql: &str| -> rusqlite::Result<i64> {
            conn.query_row(sql, params![book_id], |r| r.get(0))
        };

        Ok(BookStats {
            pages: count("SELECT COUNT(*) FROM pages WHERE book_id = ?1")?,
            paragraphs: count(
                "SELECT COUNT(*) FROM blocks b JOIN pages p ON p.id = b.page_id
                 WHERE p.book_id = ?1 AND b.kind = 'paragraph'",
            )?,
            // Distinct blocks summarised, not revisions: the reader thinks in
            // paragraphs they have done, not in drafts they wrote.
            summaries: count(
                "SELECT COUNT(DISTINCT s.block_id) FROM summaries s
                 JOIN blocks b ON b.id = s.block_id
                 JOIN pages p ON p.id = b.page_id
                 WHERE p.book_id = ?1 AND s.sentence_ordinal IS NULL",
            )?,
            words_looked_up: count("SELECT COUNT(*) FROM vocab WHERE book_id = ?1")?,
            // Everything the study layer added. These cascade-delete with the
            // book exactly as summaries do, and counting only the pre-v7
            // tables meant the delete prompt named a fraction of what it was
            // about to destroy — a reader could lose forty glosses and a dozen
            // reconstructions while being told about pages and summaries.
            notes: count("SELECT COUNT(*) FROM notes WHERE book_id = ?1")?,
            terms: count("SELECT COUNT(*) FROM terms WHERE book_id = ?1")?,
            arguments: count("SELECT COUNT(*) FROM arguments WHERE book_id = ?1")?,
        })
    }

    /// Delete a book and everything belonging to it.
    ///
    /// Rows go by cascade from `books`. The caller removes the images, which
    /// live on disk rather than in here.
    pub fn delete_book(&self, book_id: i64) -> Result<()> {
        let conn = self.lock();
        let n = conn.execute("DELETE FROM books WHERE id = ?1", params![book_id])?;
        if n == 0 {
            return Err(AppError::NotFound(format!("book {book_id}")));
        }
        Ok(())
    }

    // ---- pages ----------------------------------------------------------

    /// Add a page. The page number is left unknown until the model reads it.
    ///
    /// Returns [`AppError::Invalid`] if this exact photograph is already in the
    /// book, naming the page it duplicates.
    pub fn add_page(
        &self,
        book_id: i64,
        image_hash: &str,
        image_orig: &str,
        image_proc: &str,
    ) -> Result<i64> {
        let conn = self.lock();

        if let Some((existing_id, page_no)) = conn
            .query_row(
                "SELECT id, page_no FROM pages WHERE book_id = ?1 AND image_hash = ?2",
                params![book_id, image_hash],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?
        {
            return Err(AppError::Invalid(match page_no {
                Some(n) => format!("this photograph is already in the book as page {n}"),
                None => format!("this photograph is already in the book (page id {existing_id})"),
            }));
        }

        conn.execute(
            "INSERT INTO pages (book_id, image_hash, image_orig, image_proc, ocr_status)
             VALUES (?1, ?2, ?3, ?4, 'pending')",
            params![book_id, image_hash, image_orig, image_proc],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Remove a page and everything derived from it.
    ///
    /// Returns the image paths so the caller can delete the files too; leaving
    /// multi-megabyte photographs behind for a page the reader deleted is how
    /// a library quietly fills a disk.
    pub fn delete_page(&self, page_id: i64) -> Result<Vec<String>> {
        let conn = self.lock();
        let paths: (String, Option<String>) = conn
            .query_row(
                "SELECT image_orig, image_proc FROM pages WHERE id = ?1",
                params![page_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(format!("page {page_id}")))?;

        // Blocks, and through them summaries and critiques, cascade.
        conn.execute("DELETE FROM pages WHERE id = ?1", params![page_id])?;

        let mut out = vec![paths.0];
        out.extend(paths.1);
        Ok(out)
    }

    /// An existing page in this book with the same image, as (id, page_no).
    pub fn page_with_hash(&self, book_id: i64, hash: &str) -> Result<Option<(i64, Option<i64>)>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id, page_no FROM pages WHERE book_id = ?1 AND image_hash = ?2",
                params![book_id, hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Add a page whose text is already known — from an EPUB, a PDF, or a web
    /// page — together with its blocks, in one transaction.
    ///
    /// No OCR, no image, and no `image_hash`: deduplication for these sources
    /// is by document, handled before this is called.
    pub fn add_text_page(
        &self,
        book_id: i64,
        source_kind: &str,
        source_ref: Option<&str>,
        origin: &str,
        page_no: Option<i64>,
        blocks: &[DraftBlock],
    ) -> Result<i64> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        tx.execute(
            "INSERT INTO pages
               (book_id, page_no, page_no_source, source_kind, source_ref,
                image_orig, ocr_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'done')",
            params![
                book_id,
                page_no,
                if page_no.is_some() { "detected" } else { "unknown" },
                source_kind,
                source_ref,
                origin
            ],
        )?;
        let page_id = tx.last_insert_rowid();

        {
            let mut stmt = tx.prepare(
                "INSERT INTO blocks (page_id, ordinal, kind, text_raw, text_norm)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, b) in blocks.iter().enumerate() {
                stmt.execute(params![
                    page_id,
                    i as i64,
                    b.kind.as_str(),
                    b.text_raw,
                    b.text_norm
                ])?;
            }
        }
        tx.commit()?;
        Ok(page_id)
    }

    /// Has this document already been imported into this book?
    pub fn has_source(&self, book_id: i64, origin: &str) -> Result<bool> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT 1 FROM pages WHERE book_id = ?1 AND image_orig = ?2 LIMIT 1",
                params![book_id, origin],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn set_page_source_ref(&self, page_id: i64, source_ref: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE pages SET source_ref = ?2 WHERE id = ?1",
            params![page_id, source_ref],
        )?;
        Ok(())
    }

    /// The other half of the photograph this page came from.
    ///
    /// Returns the sibling's id, its page number, and which side *this* page
    /// is — enough to reconcile two facing pages, which are always
    /// consecutive.
    pub fn spread_sibling(&self, page_id: i64) -> Result<Option<(i64, Option<i64>, String)>> {
        let conn = self.lock();

        let Some((book_id, source_ref)) = conn
            .query_row(
                "SELECT book_id, source_ref FROM pages WHERE id = ?1",
                params![page_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?
            .and_then(|(b, s)| s.map(|s| (b, s)))
        else {
            return Ok(None);
        };

        let Some((stamp, side)) = source_ref.split_once(':') else {
            return Ok(None);
        };
        let other = if side == "left" { "right" } else { "left" };

        Ok(conn
            .query_row(
                "SELECT id, page_no FROM pages
                 WHERE book_id = ?1 AND source_ref = ?2 AND id != ?3",
                params![book_id, format!("{stamp}:{other}"), page_id],
                |r| Ok((r.get(0)?, r.get(1)?, side.to_string())),
            )
            .optional()?)
    }

    /// Page numbers already in use by other pages of this book.
    pub fn page_numbers_excluding(&self, book_id: i64, exclude: &[i64]) -> Result<Vec<i64>> {
        let conn = self.lock();
        let all: Vec<(i64, i64)> = conn
            .prepare("SELECT id, page_no FROM pages WHERE book_id = ?1 AND page_no IS NOT NULL")?
            .query_map(params![book_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(all
            .into_iter()
            .filter(|(id, _)| !exclude.contains(id))
            .map(|(_, n)| n)
            .collect())
    }

    /// Record the page number the model read off the page.
    pub fn set_page_number(&self, page_id: i64, page_no: Option<i64>, source: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE pages SET page_no = ?2, page_no_source = ?3 WHERE id = ?1",
            params![page_id, page_no, source],
        )?;
        Ok(())
    }

    /// Another page in the book already claiming this number, if any.
    ///
    /// Not an error — the reader may be re-photographing a page they were
    /// unhappy with — but worth telling them about.
    pub fn page_with_number(&self, book_id: i64, page_no: i64, excluding: i64) -> Result<Option<i64>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id FROM pages WHERE book_id = ?1 AND page_no = ?2 AND id != ?3",
                params![book_id, page_no, excluding],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Point a page at new image files, after it has been re-cropped.
    ///
    /// The hash goes with them: it is what stops the same photograph being
    /// imported twice, and a cropped page is genuinely a different image.
    /// `ocr_status` returns to pending because the text on disk describes a
    /// picture that no longer exists.
    pub fn replace_page_images(
        &self,
        page_id: i64,
        hash: &str,
        image_orig: &str,
        image_proc: &str,
    ) -> Result<()> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE pages
                SET image_hash = ?2, image_orig = ?3, image_proc = ?4,
                    ocr_status = 'pending', ocr_error = NULL
              WHERE id = ?1",
            params![page_id, hash, image_orig, image_proc],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound(format!("page {page_id}")));
        }
        Ok(())
    }

    pub fn page_images(&self, page_id: i64) -> Result<(i64, String, Option<String>)> {
        Ok(self.lock().query_row(
            "SELECT p.book_id, p.image_orig, p.image_proc FROM pages p WHERE p.id = ?1",
            params![page_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    pub fn set_page_status(&self, page_id: i64, status: &str, error: Option<&str>) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE pages SET ocr_status = ?2, ocr_error = ?3 WHERE id = ?1",
            params![page_id, status, error],
        )?;
        Ok(())
    }

    /// Pages in reading order: by the number printed on the page where we know
    /// it, and by import time for pages not yet transcribed.
    pub fn list_pages(&self, book_id: i64) -> Result<Vec<Page>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, book_id, page_no, page_no_source, source_kind, source_ref,
                    image_orig, image_proc, ocr_status, ocr_model, ocr_error
             FROM pages WHERE book_id = ?1
             ORDER BY page_no IS NULL, page_no, created_at, id",
        )?;
        let rows = stmt
            .query_map(params![book_id], |r| {
                Ok(Page {
                    id: r.get(0)?,
                    book_id: r.get(1)?,
                    page_no: r.get(2)?,
                    page_no_source: r.get(3)?,
                    source_kind: r.get(4)?,
                    source_ref: r.get(5)?,
                    image_orig: r.get(6)?,
                    image_proc: r.get(7)?,
                    ocr_status: r.get(8)?,
                    ocr_model: r.get(9)?,
                    ocr_error: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Store the OCR result and the blocks derived from it.
    ///
    /// The raw output is kept alongside the blocks so segmentation can be
    /// re-run and improved later without going back to the model.
    pub fn save_ocr_result(
        &self,
        page_id: i64,
        model: &str,
        raw: &str,
        blocks: &[DraftBlock],
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        tx.execute(
            "UPDATE pages SET ocr_raw = ?2, ocr_model = ?3, ocr_status = 'done', ocr_error = NULL
             WHERE id = ?1",
            params![page_id, raw, model],
        )?;
        // Re-segmentation replaces blocks wholesale; anything the reader edited
        // by hand is protected by the caller, not here.
        tx.execute("DELETE FROM blocks WHERE page_id = ?1", params![page_id])?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO blocks (page_id, ordinal, kind, text_raw, text_norm)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, b) in blocks.iter().enumerate() {
                stmt.execute(params![
                    page_id,
                    i as i64,
                    b.kind.as_str(),
                    b.text_raw,
                    b.text_norm
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- blocks ---------------------------------------------------------

    pub fn list_blocks(&self, page_id: i64) -> Result<Vec<Block>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, page_id, ordinal, kind, text_raw, text_norm, user_edited
             FROM blocks WHERE page_id = ?1 ORDER BY ordinal",
        )?;
        let rows = stmt
            .query_map(params![page_id], |r| {
                Ok(Block {
                    id: r.get(0)?,
                    page_id: r.get(1)?,
                    ordinal: r.get(2)?,
                    kind: r.get(3)?,
                    text_raw: r.get(4)?,
                    text_norm: r.get(5)?,
                    user_edited: r.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Which book a page belongs to. Needed to resolve a block's era without
    /// walking the whole library.
    pub fn book_id_for_page(&self, page_id: i64) -> Result<i64> {
        let conn = self.lock();
        conn.query_row(
            "SELECT book_id FROM pages WHERE id = ?1",
            params![page_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("page {page_id}")))
    }

    pub fn get_block(&self, id: i64) -> Result<Block> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, page_id, ordinal, kind, text_raw, text_norm, user_edited
             FROM blocks WHERE id = ?1",
            params![id],
            |r| {
                Ok(Block {
                    id: r.get(0)?,
                    page_id: r.get(1)?,
                    ordinal: r.get(2)?,
                    kind: r.get(3)?,
                    text_raw: r.get(4)?,
                    text_norm: r.get(5)?,
                    user_edited: r.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("block {id}")))
    }

    /// The page immediately before or after this one in reading order.
    ///
    /// Adjacency follows the number printed on the page, so a book photographed
    /// out of sequence still knows which leaf precedes which.
    fn adjacent_page(&self, page_id: i64, next: bool) -> Result<Option<i64>> {
        let conn = self.lock();
        // Compare on (page_no, created_at, id) so unnumbered pages still have a
        // stable order rather than dropping out of the sequence entirely.
        let sql = if next {
            "SELECT p.id FROM pages p, pages cur
             WHERE cur.id = ?1 AND p.book_id = cur.book_id
               AND ( (p.page_no IS NOT NULL AND cur.page_no IS NOT NULL AND p.page_no > cur.page_no)
                  OR (p.page_no IS NULL AND cur.page_no IS NULL AND p.created_at > cur.created_at)
                  OR (p.page_no IS NULL AND cur.page_no IS NOT NULL) )
             ORDER BY p.page_no IS NULL, p.page_no, p.created_at, p.id
             LIMIT 1"
        } else {
            "SELECT p.id FROM pages p, pages cur
             WHERE cur.id = ?1 AND p.book_id = cur.book_id
               AND ( (p.page_no IS NOT NULL AND cur.page_no IS NOT NULL AND p.page_no < cur.page_no)
                  OR (p.page_no IS NULL AND cur.page_no IS NULL AND p.created_at < cur.created_at)
                  OR (p.page_no IS NOT NULL AND cur.page_no IS NULL) )
             ORDER BY p.page_no IS NULL DESC, p.page_no DESC, p.created_at DESC, p.id DESC
             LIMIT 1"
        };
        Ok(conn.query_row(sql, params![page_id], |r| r.get(0)).optional()?)
    }

    pub fn next_page_id(&self, page_id: i64) -> Result<Option<i64>> {
        self.adjacent_page(page_id, true)
    }

    pub fn previous_page_id(&self, page_id: i64) -> Result<Option<i64>> {
        self.adjacent_page(page_id, false)
    }

    /// The first or last prose block of a page, skipping headings and other
    /// furniture — a paragraph continues into prose, not into a running head.
    pub fn edge_paragraph(&self, page_id: i64, first: bool) -> Result<Option<Block>> {
        let blocks = self.list_blocks(page_id)?;
        let mut prose = blocks.into_iter().filter(|b| b.kind == "paragraph");
        Ok(if first { prose.next() } else { prose.last() })
    }

    /// Delete a block and everything written about it.
    ///
    /// Transcription leaves artefacts no rule catches: a printer's signature
    /// mark fused with a folio, a caption, a stray line of a facing page. The
    /// reader can see at a glance what these are, so they get a way to say so
    /// rather than being asked to summarise them.
    ///
    /// Ordinals are left with a gap. They exist to order blocks, not to count
    /// them, and renumbering would invalidate nothing usefully.
    pub fn delete_block(&self, block_id: i64) -> Result<()> {
        let conn = self.lock();
        let n = conn.execute("DELETE FROM blocks WHERE id = ?1", params![block_id])?;
        if n == 0 {
            return Err(AppError::NotFound(format!("block {block_id}")));
        }
        Ok(())
    }

    /// Change what a block is: prose, a heading, a quotation.
    ///
    /// Heading detection works from shape alone and cannot always be right —
    /// an italic section title reads as an ordinary sentence. Being able to
    /// say so keeps such a line out of the summarising queue and puts it into
    /// the book's outline where it belongs.
    pub fn set_block_kind(&self, block_id: i64, kind: &str) -> Result<()> {
        const KINDS: [&str; 5] = ["paragraph", "heading", "quote", "footnote", "caption"];
        if !KINDS.contains(&kind) {
            return Err(AppError::Invalid(format!("{kind} is not a kind of block")));
        }
        let conn = self.lock();
        let n = conn.execute(
            "UPDATE blocks SET kind = ?2, user_edited = 1 WHERE id = ?1",
            params![block_id, kind],
        )?;
        if n == 0 {
            return Err(AppError::NotFound(format!("block {block_id}")));
        }
        Ok(())
    }

    /// Correct a block by hand. OCR is never perfect, and a reader with no way
    /// to fix a mangled paragraph cannot use the application at all.
    pub fn edit_block(&self, id: i64, text: &str) -> Result<()> {
        let conn = self.lock();
        let n = conn.execute(
            "UPDATE blocks SET text_norm = ?2, user_edited = 1 WHERE id = ?1",
            params![id, text],
        )?;
        if n == 0 {
            return Err(AppError::NotFound(format!("block {id}")));
        }
        Ok(())
    }

    // ---- summaries and critiques ---------------------------------------

    /// Save a summary as a new revision, preserving earlier attempts so the
    /// reader can see their own thinking improve.
    ///
    /// `sentence_ordinal` is `None` for a summary of the whole paragraph and
    /// `Some(i)` for the i-th sentence within it. Revisions are counted per
    /// target, so revising one sentence does not bump the paragraph's.
    pub fn save_summary(
        &self,
        block_id: i64,
        sentence_ordinal: Option<i64>,
        sentence: &str,
        self_checked: bool,
    ) -> Result<i64> {
        let conn = self.lock();
        let revision: i64 = conn.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM summaries
             WHERE block_id = ?1 AND sentence_ordinal IS ?2",
            params![block_id, sentence_ordinal],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT INTO summaries (block_id, sentence_ordinal, sentence, revision, self_checked)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                block_id,
                sentence_ordinal,
                sentence,
                revision,
                self_checked as i64
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// The reader's latest note on each sentence of a paragraph.
    pub fn sentence_summaries(&self, block_id: i64) -> Result<Vec<SentenceSummary>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT sentence_ordinal, sentence
             FROM summaries s
             WHERE block_id = ?1 AND sentence_ordinal IS NOT NULL
               AND revision = (SELECT MAX(revision) FROM summaries
                               WHERE block_id = s.block_id
                                 AND sentence_ordinal IS s.sentence_ordinal)
             ORDER BY sentence_ordinal",
        )?;
        let rows = stmt
            .query_map(params![block_id], |r| {
                Ok(SentenceSummary {
                    ordinal: r.get(0)?,
                    text: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The latest paragraph-level summary for a block.
    pub fn latest_summary(&self, block_id: i64) -> Result<Option<crate::models::Summary>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT id, block_id, sentence, revision, self_checked, created_at
                 FROM summaries WHERE block_id = ?1 AND sentence_ordinal IS NULL
                 ORDER BY revision DESC LIMIT 1",
                params![block_id],
                |r| {
                    Ok(crate::models::Summary {
                        id: r.get(0)?,
                        block_id: r.get(1)?,
                        sentence: r.get(2)?,
                        revision: r.get(3)?,
                        self_checked: r.get::<_, i64>(4)? != 0,
                        created_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn save_critique(
        &self,
        summary_id: i64,
        verdict: &str,
        covers: (bool, bool, bool),
        problem: &str,
        steering_question: &str,
        model: &str,
    ) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO critiques
               (summary_id, verdict, covers_subject, covers_action, covers_reason,
                problem, steering_question, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                summary_id,
                verdict,
                covers.0 as i64,
                covers.1 as i64,
                covers.2 as i64,
                problem,
                steering_question,
                model
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// The reader's accumulated one-sentence summaries for a book, in reading
    /// order — the "spine" that the build-up step assembles.
    pub fn summary_spine(&self, book_id: i64) -> Result<Vec<SpineEntry>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT p.page_no, b.ordinal, b.id, s.sentence
             FROM blocks b
             JOIN pages p ON p.id = b.page_id
             JOIN summaries s ON s.block_id = b.id
             WHERE p.book_id = ?1
               AND s.sentence_ordinal IS NULL
               AND s.revision = (SELECT MAX(revision) FROM summaries
                                 WHERE block_id = b.id AND sentence_ordinal IS NULL)
             ORDER BY p.page_no IS NULL, p.page_no, p.created_at, b.ordinal",
        )?;
        let rows = stmt
            .query_map(params![book_id], |r| {
                Ok(SpineEntry {
                    page_no: r.get(0)?,
                    ordinal: r.get(1)?,
                    block_id: r.get(2)?,
                    sentence: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

impl Db {
    /// Record a word the reader looked up, with the sentence they met it in.
    ///
    /// The sentence is the point: a vocabulary list of bare words is nearly
    /// useless, while one that shows where you met each word is reviewable.
    pub fn record_lookup(
        &self,
        book_id: i64,
        block_id: i64,
        word: &str,
        lemma: &str,
        sentence: &str,
        gloss: &str,
    ) -> Result<i64> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        tx.execute(
            "INSERT INTO lookups (book_id, block_id, word, lemma, sentence, llm_gloss)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![book_id, block_id, word, lemma, sentence, gloss],
        )?;
        let lookup_id = tx.last_insert_rowid();

        // Second and later encounters bump the count rather than duplicating.
        tx.execute(
            "INSERT INTO vocab (book_id, word, lemma, count, first_lookup_id)
             VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT(book_id, word)
             DO UPDATE SET count = count + 1",
            params![book_id, word.to_lowercase(), lemma, lookup_id],
        )?;

        tx.commit()?;
        Ok(lookup_id)
    }

    /// Words looked up in this book, most frequent first.
    pub fn vocabulary(&self, book_id: i64) -> Result<Vec<VocabEntry>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT v.word, v.lemma, v.count, l.sentence, l.llm_gloss
             FROM vocab v
             LEFT JOIN lookups l ON l.id = v.first_lookup_id
             WHERE v.book_id = ?1
             ORDER BY v.count DESC, v.word",
        )?;
        let rows = stmt
            .query_map(params![book_id], |r| {
                Ok(VocabEntry {
                    word: r.get(0)?,
                    lemma: r.get(1)?,
                    count: r.get(2)?,
                    sentence: r.get(3)?,
                    gloss: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BookStats {
    pub pages: i64,
    pub paragraphs: i64,
    /// Paragraphs the reader has summarised.
    pub summaries: i64,
    pub words_looked_up: i64,
    /// Notes and marks together — both are rows in `notes`.
    pub notes: i64,
    pub terms: i64,
    pub arguments: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SentenceSummary {
    pub ordinal: i64,
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VocabEntry {
    pub word: String,
    pub lemma: Option<String>,
    pub count: i64,
    pub sentence: Option<String>,
    pub gloss: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpineEntry {
    /// Null for a page whose number has not been read yet.
    pub page_no: Option<i64>,
    pub ordinal: i64,
    pub block_id: i64,
    pub sentence: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BlockKind;

    fn draft(text: &str) -> DraftBlock {
        DraftBlock {
            kind: BlockKind::Paragraph,
            text_raw: text.to_string(),
            text_norm: text.to_string(),
        }
    }

    fn seeded() -> (Db, i64, i64) {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("The Art of Reading", Some("A. Author"), "modern").unwrap();
        let page = db.add_page(book, "hash-a", "orig.jpg", "proc.jpg").unwrap();
        (db, book, page)
    }

    #[test]
    fn creates_and_lists_a_book() {
        let (db, book, _) = seeded();
        let books = db.list_books().unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].id, book);
        assert_eq!(books[0].era, "modern");
    }

    /// The v1 shape, as shipped before page numbers were read off the page.
    /// Only the parts the migration touches are reproduced.
    const V1_SCHEMA: &str = "
        CREATE TABLE books (
            id INTEGER PRIMARY KEY, title TEXT NOT NULL, author TEXT,
            era TEXT NOT NULL DEFAULT 'modern',
            dict_corpus TEXT NOT NULL DEFAULT 'auto',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE pages (
            id INTEGER PRIMARY KEY,
            book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            page_no INTEGER NOT NULL,
            image_orig TEXT NOT NULL, image_proc TEXT,
            ocr_status TEXT NOT NULL DEFAULT 'pending',
            ocr_model TEXT, ocr_raw TEXT, ocr_error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (book_id, page_no)
        );
        CREATE TABLE blocks (
            id INTEGER PRIMARY KEY,
            page_id INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'paragraph',
            text_raw TEXT NOT NULL, text_norm TEXT NOT NULL,
            user_edited INTEGER NOT NULL DEFAULT 0,
            UNIQUE (page_id, ordinal)
        );
        CREATE TABLE summaries (
            id INTEGER PRIMARY KEY,
            block_id INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
            sentence TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
            self_checked INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    ";

    /// The v1 -> v3 migration rebuilds `pages`, and `blocks` cascade-delete
    /// from it. Getting the foreign-key handling wrong would silently destroy
    /// every block on every page of a reader's library, so it is pinned here.
    #[test]
    fn migrating_a_v1_library_preserves_the_readers_work() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute_batch(
            "INSERT INTO books (id, title) VALUES (1, 'Systematic Theology');
             INSERT INTO pages (id, book_id, page_no, image_orig, ocr_status, ocr_raw)
                 VALUES (1, 1, 1, 'a.jpg', 'done', 'raw text');
             INSERT INTO blocks (id, page_id, ordinal, text_raw, text_norm)
                 VALUES (1, 1, 0, 'A paragraph.', 'A paragraph.'),
                        (2, 1, 1, 'Another one.', 'Another one.');
             INSERT INTO summaries (block_id, sentence) VALUES (1, 'My sentence.');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        let db = Db::init(conn).expect("migration should succeed");

        assert_eq!(db.list_books().unwrap().len(), 1);

        let pages = db.list_pages(1).unwrap();
        assert_eq!(pages.len(), 1);
        // The number is discarded: it came from import order, which is exactly
        // what this migration exists to stop trusting.
        assert_eq!(pages[0].page_no, None);
        assert_eq!(pages[0].page_no_source, "unknown");

        // The blocks must have survived the table rebuild.
        let blocks = db.list_blocks(1).unwrap();
        assert_eq!(blocks.len(), 2, "blocks were lost in the rebuild");
        assert_eq!(blocks[0].text_raw, "A paragraph.");

        // And the reader's own writing.
        let summary = db.latest_summary(1).unwrap().unwrap();
        assert_eq!(summary.sentence, "My sentence.");

        // The new behaviour is live.
        let err = db.add_page(1, "h", "b.jpg", "bp.jpg").unwrap();
        db.set_page_number(err, Some(12), "detected").unwrap();
        assert!(db.add_page(1, "h", "c.jpg", "cp.jpg").is_err());
    }

    /// Foreign keys are switched off during the rebuild; leaving them off
    /// would silently orphan rows on every later delete.
    #[test]
    fn foreign_keys_survive_the_migration() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.execute_batch(
            "INSERT INTO books (id, title) VALUES (1, 'B');
             INSERT INTO pages (id, book_id, page_no, image_orig) VALUES (1, 1, 1, 'a.jpg');
             INSERT INTO blocks (id, page_id, ordinal, text_raw, text_norm)
                 VALUES (1, 1, 0, 'x', 'x');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        let db = Db::init(conn).unwrap();
        let on: i64 = db
            .lock()
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on, 1, "foreign keys were left disabled after migrating");

        // Cascade still works after the rebuild.
        db.lock()
            .execute("DELETE FROM books WHERE id = 1", [])
            .unwrap();
        assert!(db.list_blocks(1).unwrap().is_empty());
    }

    #[test]
    fn a_new_page_has_no_number_until_it_is_read() {
        let (db, book, _) = seeded();
        let pages = db.list_pages(book).unwrap();
        assert_eq!(pages[0].page_no, None);
        assert_eq!(pages[0].page_no_source, "unknown");
    }

    /// The user-visible bug this replaced: the same photograph imported three
    /// times, becoming pages 1, 2 and 3 of the book.
    #[test]
    fn re_importing_the_same_photograph_is_rejected() {
        let (db, book, _) = seeded();
        let err = db
            .add_page(book, "hash-a", "copy.jpg", "copyp.jpg")
            .unwrap_err();
        assert!(
            err.to_string().contains("already in the book"),
            "got: {err}"
        );
        assert_eq!(db.list_pages(book).unwrap().len(), 1);
    }

    #[test]
    fn the_same_photograph_may_appear_in_two_different_books() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_book("A", None, "modern").unwrap();
        let b = db.create_book("B", None, "victorian").unwrap();
        db.add_page(a, "same", "1.jpg", "1p.jpg").unwrap();
        // Deduplication is per book, not global.
        assert!(db.add_page(b, "same", "1.jpg", "1p.jpg").is_ok());
    }

    #[test]
    fn pages_order_by_the_number_printed_on_them_not_import_order() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();

        // Photographed out of sequence, as happens in practice.
        let p_third = db.add_page(book, "h3", "3.jpg", "3p.jpg").unwrap();
        let p_first = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        let p_second = db.add_page(book, "h2", "2.jpg", "2p.jpg").unwrap();

        db.set_page_number(p_third, Some(42), "detected").unwrap();
        db.set_page_number(p_first, Some(40), "detected").unwrap();
        db.set_page_number(p_second, Some(41), "detected").unwrap();

        let order: Vec<Option<i64>> =
            db.list_pages(book).unwrap().iter().map(|p| p.page_no).collect();
        assert_eq!(order, vec![Some(40), Some(41), Some(42)]);
    }

    #[test]
    fn unnumbered_pages_sort_after_numbered_ones() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let numbered = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        db.add_page(book, "h2", "2.jpg", "2p.jpg").unwrap();
        db.set_page_number(numbered, Some(7), "detected").unwrap();

        let pages = db.list_pages(book).unwrap();
        assert_eq!(pages[0].page_no, Some(7));
        assert_eq!(pages[1].page_no, None);
    }

    /// Two photographs of the same numbered page is not an error — a reader
    /// may re-shoot a page that came out badly — but it is worth surfacing.
    #[test]
    fn a_clashing_page_number_can_be_found() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let a = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        let b = db.add_page(book, "h2", "2.jpg", "2p.jpg").unwrap();
        db.set_page_number(a, Some(12), "detected").unwrap();
        db.set_page_number(b, Some(12), "detected").unwrap();

        assert_eq!(db.page_with_number(book, 12, b).unwrap(), Some(a));
        assert_eq!(db.page_with_number(book, 99, b).unwrap(), None);
    }

    #[test]
    fn saving_ocr_stores_blocks_in_order() {
        let (db, _, page) = seeded();
        db.save_ocr_result(page, "glm-ocr", "raw output", &[draft("First."), draft("Second.")])
            .unwrap();

        let blocks = db.list_blocks(page).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text_raw, "First.");
        assert_eq!(blocks[1].ordinal, 1);
        assert_eq!(db.list_pages(blocks[0].page_id).unwrap()[0].ocr_status, "done");
    }

    #[test]
    fn re_running_ocr_replaces_previous_blocks() {
        let (db, _, page) = seeded();
        db.save_ocr_result(page, "glm-ocr", "v1", &[draft("Old.")]).unwrap();
        db.save_ocr_result(page, "glm-ocr", "v2", &[draft("New."), draft("Also new.")])
            .unwrap();

        let blocks = db.list_blocks(page).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text_raw, "New.");
    }

    #[test]
    fn editing_a_block_marks_it_user_edited() {
        let (db, _, page) = seeded();
        db.save_ocr_result(page, "glm-ocr", "raw", &[draft("Mangled OCR txet.")]).unwrap();
        let id = db.list_blocks(page).unwrap()[0].id;

        db.edit_block(id, "Corrected OCR text.").unwrap();

        let b = db.get_block(id).unwrap();
        assert_eq!(b.text_norm, "Corrected OCR text.");
        assert!(b.user_edited);
        // The diplomatic transcription is untouched by an edit.
        assert_eq!(b.text_raw, "Mangled OCR txet.");
    }

    #[test]
    fn summaries_are_revised_not_overwritten() {
        let (db, _, page) = seeded();
        db.save_ocr_result(page, "glm-ocr", "raw", &[draft("A paragraph.")]).unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;

        db.save_summary(block, None, "First attempt.", false).unwrap();
        db.save_summary(block, None, "Better attempt.", true).unwrap();

        let latest = db.latest_summary(block).unwrap().unwrap();
        assert_eq!(latest.sentence, "Better attempt.");
        assert_eq!(latest.revision, 2);
        assert!(latest.self_checked);
    }

    #[test]
    fn the_spine_follows_reading_order() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();

        let p1 = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        let p2 = db.add_page(book, "h2", "2.jpg", "2p.jpg").unwrap();
        db.set_page_number(p1, Some(1), "detected").unwrap();
        db.set_page_number(p2, Some(2), "detected").unwrap();
        db.save_ocr_result(p1, "m", "r", &[draft("P1 para1"), draft("P1 para2")]).unwrap();
        db.save_ocr_result(p2, "m", "r", &[draft("P2 para1")]).unwrap();

        // Summarise out of order to prove ordering comes from the text.
        let b2 = db.list_blocks(p2).unwrap()[0].id;
        let b1 = db.list_blocks(p1).unwrap();
        db.save_summary(b2, None, "Third sentence.", true).unwrap();
        db.save_summary(b1[1].id, None, "Second sentence.", true).unwrap();
        db.save_summary(b1[0].id, None, "First sentence.", true).unwrap();

        let spine = db.summary_spine(book).unwrap();
        let sentences: Vec<_> = spine.iter().map(|s| s.sentence.as_str()).collect();
        assert_eq!(sentences, vec!["First sentence.", "Second sentence.", "Third sentence."]);
    }

    #[test]
    fn the_spine_uses_only_the_latest_revision() {
        let (db, book, page) = seeded();
        db.save_ocr_result(page, "m", "r", &[draft("A paragraph.")]).unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;

        db.save_summary(block, None, "Early draft.", false).unwrap();
        db.save_summary(block, None, "Final version.", true).unwrap();

        let spine = db.summary_spine(book).unwrap();
        assert_eq!(spine.len(), 1);
        assert_eq!(spine[0].sentence, "Final version.");
    }

    #[test]
    fn deleting_a_book_cascades_to_its_pages_and_blocks() {
        let (db, book, page) = seeded();
        db.save_ocr_result(page, "m", "r", &[draft("Text.")]).unwrap();

        db.lock()
            .execute("DELETE FROM books WHERE id = ?1", params![book])
            .unwrap();

        assert!(db.list_pages(book).unwrap().is_empty());
        assert!(db.list_blocks(page).unwrap().is_empty());
    }

    /// Deleting a book must reach everything hanging off it. The cascade runs
    /// four tables deep — pages, blocks, summaries, critiques — and a missing
    /// `ON DELETE` anywhere in that chain would leave orphans behind while
    /// looking like it worked.
    #[test]
    fn deleting_a_book_removes_everything_belonging_to_it() {
        let (db, book, page) = seeded();
        db.save_ocr_result(page, "m", "r", &[draft("A paragraph."), draft("Another.")])
            .unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;
        let summary = db.save_summary(block, None, "My sentence.", true).unwrap();
        db.save_critique(summary, "partial", (true, true, false), "incomplete", "?", "m")
            .unwrap();
        db.record_lookup(book, block, "nice", "nice", "A sentence.", "gloss")
            .unwrap();

        db.delete_book(book).unwrap();

        assert!(db.list_books().unwrap().is_empty());
        assert!(db.list_pages(book).unwrap().is_empty());
        assert!(db.list_blocks(page).unwrap().is_empty());
        assert!(db.vocabulary(book).unwrap().is_empty());
        assert!(db.summary_spine(book).unwrap().is_empty());

        let conn = db.lock();
        for table in ["summaries", "critiques", "lookups"] {
            let left: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(left, 0, "{table} still has rows after deleting the book");
        }
    }

    #[test]
    fn deleting_one_book_leaves_the_others_alone() {
        let db = Db::open_in_memory().unwrap();
        let keep = db.create_book("Keep", None, "modern").unwrap();
        let drop = db.create_book("Drop", None, "modern").unwrap();
        let kept_page = db.add_page(keep, "h1", "1.jpg", "1p.jpg").unwrap();
        db.add_page(drop, "h2", "2.jpg", "2p.jpg").unwrap();
        db.save_ocr_result(kept_page, "m", "r", &[draft("Kept text.")])
            .unwrap();

        db.delete_book(drop).unwrap();

        assert_eq!(db.list_books().unwrap().len(), 1);
        assert_eq!(db.list_blocks(kept_page).unwrap().len(), 1);
    }

    #[test]
    fn a_book_reopens_where_the_reader_stopped() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let first = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        let third = db.add_page(book, "h3", "3.jpg", "3p.jpg").unwrap();
        db.set_page_number(first, Some(1), "detected").unwrap();
        db.set_page_number(third, Some(3), "detected").unwrap();

        // Nothing remembered yet: start at the beginning.
        assert_eq!(db.resume_page(book).unwrap(), Some(first));

        db.set_last_page(book, Some(third)).unwrap();
        assert_eq!(db.resume_page(book).unwrap(), Some(third));
        assert_eq!(db.get_book(book).unwrap().last_page_id, Some(third));
    }

    /// A remembered page can be deleted, or replaced by re-importing. Opening
    /// the book must then fall back rather than show nothing.
    #[test]
    fn a_remembered_page_that_no_longer_exists_falls_back() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("B", None, "modern").unwrap();
        let first = db.add_page(book, "h1", "1.jpg", "1p.jpg").unwrap();
        let second = db.add_page(book, "h2", "2.jpg", "2p.jpg").unwrap();

        db.set_last_page(book, Some(second)).unwrap();
        db.delete_page(second).unwrap();

        // The foreign key clears the reference rather than orphaning it.
        assert_eq!(db.get_book(book).unwrap().last_page_id, None);
        assert_eq!(db.resume_page(book).unwrap(), Some(first));
    }

    #[test]
    fn a_book_with_no_pages_has_nowhere_to_resume() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book("Empty", None, "modern").unwrap();
        assert_eq!(db.resume_page(book).unwrap(), None);
    }

    #[test]
    fn stats_describe_what_deleting_would_cost() {
        let (db, book, page) = seeded();
        db.save_ocr_result(
            page,
            "m",
            "r",
            &[draft("First para."), draft("Second para."), draft("Third.")],
        )
        .unwrap();
        let blocks = db.list_blocks(page).unwrap();
        db.save_summary(blocks[0].id, None, "One.", true).unwrap();
        // A revision of the same paragraph is not a second summary.
        db.save_summary(blocks[0].id, None, "One, better.", true).unwrap();
        db.save_summary(blocks[1].id, None, "Two.", true).unwrap();
        db.record_lookup(book, blocks[0].id, "nice", "nice", "s", "g")
            .unwrap();

        let stats = db.book_stats(book).unwrap();
        assert_eq!(stats.pages, 1);
        assert_eq!(stats.paragraphs, 3);
        assert_eq!(stats.summaries, 2, "revisions must not be counted twice");
        assert_eq!(stats.words_looked_up, 1);
    }

    #[test]
    fn critiques_record_the_rubric() {
        let (db, _, page) = seeded();
        db.save_ocr_result(page, "m", "r", &[draft("A paragraph.")]).unwrap();
        let block = db.list_blocks(page).unwrap()[0].id;
        let summary = db.save_summary(block, None, "My sentence.", true).unwrap();

        db.save_critique(summary, "partial", (true, true, false), "incomplete", "What else?", "qwen3:4b")
            .unwrap();

        let conn = db.lock();
        let (verdict, reason, problem): (String, i64, String) = conn
            .query_row(
                "SELECT verdict, covers_reason, problem FROM critiques WHERE summary_id = ?1",
                params![summary],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(verdict, "partial");
        assert_eq!(reason, 0);
        assert_eq!(problem, "incomplete");
    }
}
