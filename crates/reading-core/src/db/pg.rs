//! The library database, on Postgres.
//!
//! Replaces the SQLite layer in `super`, which stays for now because the
//! one-time importer has to read the old file.
//!
//! # Ownership
//!
//! Every row in this database belongs to exactly one person, and the whole
//! design of this module is about making that impossible to forget.
//!
//! `books.user_id` is the only place ownership is recorded. Pages, blocks,
//! summaries, critiques and vocabulary carry no user of their own: they are
//! reached by joining up to `books`. So there is one column to get right
//! rather than seven to keep in step.
//!
//! Accessors take a [`UserId`] — not an `Option<UserId>`, and not a separate
//! "check this first" function that a new endpoint could forget to call. The
//! filter is inside the query. A row belonging to someone else does not come
//! back, and the caller cannot ask it not to.
//!
//! A request for someone else's row reports [`AppError::NotFound`], never a
//! "forbidden". Distinguishing the two would confirm that a given id exists,
//! which is exactly what an attacker enumerating ids wants to learn.

use crate::sqlx::{PgPool, PgPoolOptions, PgRow, Row};

use crate::error::{AppError, Result};
use crate::models::{
    Block, Book, BookStats, DraftBlock, Page, SentenceSummary, SpineEntry, Summary, VocabEntry,
};

/// An authenticated user.
///
/// A newtype rather than a bare `i64` so it cannot be transposed with a book
/// id or a page id at a call site — the mistake that would silently hand one
/// reader another's library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub i64);

impl UserId {
    pub fn get(self) -> i64 {
        self.0
    }
}

/// A user of the server.
#[derive(Debug, Clone)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    /// Only an administrator may change the Ollama address, because that is a
    /// URL the *server* then makes requests to.
    pub is_admin: bool,
}

/// Schema history, embedded so a deployed server needs no repository beside
/// it. Append only: an applied migration is never edited, because the
/// databases that already ran it will not run it again.
const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_initial",
    include_str!("../../migrations/0001_initial.sql"),
)];

#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect and bring the schema up to date.
    ///
    /// The pool is small on purpose. Postgres handles a connection per backend
    /// process, and this workload is a handful of readers plus the occasional
    /// import — a large pool would cost the server memory to no end.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(url)
            .await
            .map_err(to_app_error)?;

        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// Wrap an existing pool, for tests that manage their own.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply anything in [`MIGRATIONS`] that has not run yet.
    ///
    /// Hand-rolled rather than `sqlx::migrate!`, which lives behind the
    /// `macros` feature — and that feature drags in the SQLite driver, which
    /// cannot coexist with the `rusqlite` the dictionary uses. Thirty lines is
    /// a better trade than losing the period dictionary.
    ///
    /// Each file runs inside a transaction and is recorded in the same
    /// transaction, so a migration that fails half way leaves nothing behind
    /// and can simply be run again.
    pub async fn migrate(&self) -> Result<()> {
        crate::sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 name       TEXT PRIMARY KEY,
                 applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
             )",
        )
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        self.adopt_existing_schema().await?;

        for (name, sql) in MIGRATIONS {
            let already = crate::sqlx::query("SELECT 1 FROM schema_migrations WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(to_app_error)?
                .is_some();

            if already {
                continue;
            }

            let mut tx = self.pool.begin().await.map_err(to_app_error)?;

            // raw_sql rather than query: a migration is many statements, and
            // the prepared-statement path takes exactly one.
            crate::sqlx::raw_sql(sql)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::Other(anyhow::anyhow!("migration {name} failed: {e}")))?;

            crate::sqlx::query("INSERT INTO schema_migrations (name) VALUES ($1)")
                .bind(name)
                .execute(&mut *tx)
                .await
                .map_err(to_app_error)?;

            tx.commit().await.map_err(to_app_error)?;
        }

        Ok(())
    }

    /// Recognise a database whose schema was applied by something other than
    /// this code, and record the first migration as done rather than trying
    /// to build tables that are already there.
    ///
    /// Early versions of `setup-postgres.ps1` ran the .sql file with psql,
    /// before migrations were given a single owner. A database created that
    /// way has every table and no bookkeeping, so the server would try the
    /// first migration on every start and fail with `relation "users" already
    /// exists` — a message that says nothing about what to do.
    ///
    /// Deliberately narrow. It fires only when the bookkeeping table is
    /// completely empty *and* the schema is plainly present, and it adopts
    /// only the first migration; anything added later still runs normally.
    async fn adopt_existing_schema(&self) -> Result<()> {
        let Some((first, _)) = MIGRATIONS.first() else {
            return Ok(());
        };

        let row = crate::sqlx::query(
            "SELECT
               (SELECT count(*) FROM schema_migrations) AS recorded,
               (to_regclass('public.users') IS NOT NULL) AS has_schema",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(to_app_error)?;

        let recorded: i64 = row.get("recorded");
        let has_schema: bool = row.get("has_schema");

        if recorded == 0 && has_schema {
            crate::sqlx::query("INSERT INTO schema_migrations (name) VALUES ($1)")
                .bind(first)
                .execute(&self.pool)
                .await
                .map_err(to_app_error)?;
        }

        Ok(())
    }

    // --- Users --------------------------------------------------------------

    /// Create a user. `password_hash` is already an argon2id PHC string; this
    /// layer never sees a plaintext password.
    pub async fn create_user(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
        is_admin: bool,
    ) -> Result<UserId> {
        // Lowercased on the way in to match the unique index on lower(email),
        // so "Me@example.com" and "me@example.com" are one account.
        let row = crate::sqlx::query(
            "INSERT INTO users (email, display_name, password_hash, is_admin)
             VALUES (lower($1), $2, $3, $4)
             RETURNING id",
        )
        .bind(email.trim())
        .bind(display_name.trim())
        .bind(password_hash)
        .bind(is_admin)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match &e {
            crate::sqlx::Error::Database(db) if db.is_unique_violation() => {
                AppError::Invalid(format!("{} is already registered", email.trim()))
            }
            _ => to_app_error(e),
        })?;

        Ok(UserId(row.get::<i64, _>("id")))
    }

    /// The stored hash for an address, for the sign-in path to verify against.
    ///
    /// Returns the user alongside it so a successful verification needs no
    /// second query.
    pub async fn user_for_sign_in(&self, email: &str) -> Result<Option<(User, String)>> {
        let row = crate::sqlx::query(
            "SELECT id, email, display_name, is_admin, password_hash
             FROM users WHERE lower(email) = lower($1)",
        )
        .bind(email.trim())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.map(|r| {
            (
                User {
                    id: UserId(r.get("id")),
                    email: r.get("email"),
                    display_name: r.get("display_name"),
                    is_admin: r.get("is_admin"),
                },
                r.get("password_hash"),
            )
        }))
    }

    pub async fn get_user(&self, id: UserId) -> Result<User> {
        let row = crate::sqlx::query("SELECT id, email, display_name, is_admin FROM users WHERE id = $1")
            .bind(id.get())
            .fetch_optional(&self.pool)
            .await
            .map_err(to_app_error)?
            .ok_or_else(|| AppError::NotFound("user".into()))?;

        Ok(User {
            id: UserId(row.get("id")),
            email: row.get("email"),
            display_name: row.get("display_name"),
            is_admin: row.get("is_admin"),
        })
    }

    /// Is this the first account? The first person to register becomes the
    /// administrator, because someone has to be able to set the Ollama address
    /// and a server with no administrator is unusable.
    pub async fn has_any_user(&self) -> Result<bool> {
        let row = crate::sqlx::query("SELECT EXISTS (SELECT 1 FROM users) AS present")
            .fetch_one(&self.pool)
            .await
            .map_err(to_app_error)?;
        Ok(row.get::<bool, _>("present"))
    }

    // --- Sessions -----------------------------------------------------------

    /// Store a session. `token_digest` is SHA-256 of the token, never the
    /// token — a read of this table must not yield working credentials.
    pub async fn create_session(
        &self,
        token_digest: &[u8],
        user: UserId,
        label: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        crate::sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, label, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(token_digest)
        .bind(user.get())
        .bind(label)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;
        Ok(())
    }

    /// Resolve a session to its user, and push the expiry forward.
    ///
    /// One statement: the lookup, the expiry test, and the renewal together.
    /// Splitting them would leave a window where a session expiring this
    /// instant could be renewed by the very request that should have failed.
    pub async fn user_for_session(&self, token_digest: &[u8]) -> Result<Option<User>> {
        let row = crate::sqlx::query(
            "UPDATE sessions s
                SET last_used = now(),
                    expires_at = now() + (s.expires_at - s.created_at)
              FROM users u
             WHERE s.token_hash = $1 AND u.id = s.user_id AND s.expires_at > now()
             RETURNING u.id, u.email, u.display_name, u.is_admin",
        )
        .bind(token_digest)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.map(|r| User {
            id: UserId(r.get("id")),
            email: r.get("email"),
            display_name: r.get("display_name"),
            is_admin: r.get("is_admin"),
        }))
    }

    /// Sign out. Absent is success: the caller wanted the session gone.
    pub async fn delete_session(&self, token_digest: &[u8]) -> Result<()> {
        crate::sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_digest)
            .execute(&self.pool)
            .await
            .map_err(to_app_error)?;
        Ok(())
    }

    /// Drop expired sessions. Nothing depends on this — they are already
    /// refused — but a table that only grows is a table that eventually
    /// matters.
    pub async fn purge_expired_sessions(&self) -> Result<u64> {
        let done = crate::sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(to_app_error)?;
        Ok(done.rows_affected())
    }

    // --- Settings -----------------------------------------------------------
    //
    // Server-wide, not per user. These describe the machine doing the
    // inference rather than anyone's preferences, and letting an arbitrary
    // account point the server at a URL of their choosing is an SSRF hole.
    // Enforcement of "admin only" belongs at the HTTP layer, which knows who
    // is asking; this layer just stores.

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = crate::sqlx::query("SELECT value FROM settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(to_app_error)?;
        Ok(row.map(|r| r.get("value")))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        crate::sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;
        Ok(())
    }

    // --- Books --------------------------------------------------------------

    pub async fn create_book(
        &self,
        user: UserId,
        title: &str,
        author: Option<&str>,
        era: &str,
    ) -> Result<i64> {
        let row = crate::sqlx::query(
            "INSERT INTO books (user_id, title, author, era)
             VALUES ($1, $2, $3, $4)
             RETURNING id",
        )
        .bind(user.get())
        .bind(title.trim())
        .bind(author.map(str::trim))
        .bind(era)
        .fetch_one(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.get("id"))
    }

    pub async fn list_books(&self, user: UserId) -> Result<Vec<Book>> {
        let rows = crate::sqlx::query(
            "SELECT id, title, author, era, dict_corpus, last_page_id,
                    created_at, updated_at
             FROM books
             WHERE user_id = $1
             ORDER BY updated_at DESC",
        )
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows.iter().map(book_from_row).collect())
    }

    /// A book, if it is this user's.
    ///
    /// Someone else's book reports NotFound rather than a permission error:
    /// telling the difference would confirm the id exists.
    pub async fn get_book(&self, user: UserId, id: i64) -> Result<Book> {
        let row = crate::sqlx::query(
            "SELECT id, title, author, era, dict_corpus, last_page_id,
                    created_at, updated_at
             FROM books
             WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("book {id}")))?;

        Ok(book_from_row(&row))
    }

    pub async fn delete_book(&self, user: UserId, id: i64) -> Result<()> {
        // Everything below cascades from here.
        let done = crate::sqlx::query("DELETE FROM books WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user.get())
            .execute(&self.pool)
            .await
            .map_err(to_app_error)?;

        if done.rows_affected() == 0 {
            return Err(AppError::NotFound(format!("book {id}")));
        }
        Ok(())
    }

    /// Remember where the reader stopped.
    ///
    /// The page is confirmed to belong to the same book in the statement
    /// itself, so a page id from another book cannot be stored even by a
    /// caller that has muddled its arguments.
    pub async fn set_last_page(
        &self,
        user: UserId,
        book_id: i64,
        page_id: Option<i64>,
    ) -> Result<()> {
        let done = crate::sqlx::query(
            "UPDATE books SET last_page_id = $1, updated_at = now()
             WHERE id = $2 AND user_id = $3
               AND ($1 IS NULL OR EXISTS (
                     SELECT 1 FROM pages WHERE pages.id = $1 AND pages.book_id = $2))",
        )
        .bind(page_id)
        .bind(book_id)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        if done.rows_affected() == 0 {
            // Two causes, told apart in one message rather than two queries.
            // Naming both is safe: it says nothing about whether the book
            // exists that the reader could not already work out from asking.
            return Err(AppError::NotFound(format!(
                "book {book_id}, or page {} is not in it",
                page_id.map(|p| p.to_string()).unwrap_or_else(|| "none".into())
            )));
        }
        Ok(())
    }

    /// Which page to open a book at: where the reader stopped, or its first.
    pub async fn resume_page(&self, user: UserId, book_id: i64) -> Result<Option<i64>> {
        let row = crate::sqlx::query(
            "SELECT COALESCE(
                        b.last_page_id,
                        (SELECT p.id FROM pages p
                          WHERE p.book_id = b.id
                          ORDER BY p.page_no NULLS LAST, p.id
                          LIMIT 1)
                    ) AS page_id
             FROM books b
             WHERE b.id = $1 AND b.user_id = $2",
        )
        .bind(book_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.and_then(|r| r.get::<Option<i64>, _>("page_id")))
    }
}

// --- Pages ------------------------------------------------------------------
//
// Every one of these reaches ownership through `pages -> books.user_id`.
// Where a statement writes, the ownership test is part of the statement
// rather than a separate read, so there is no window between checking and
// acting and no way to run the write without the check.

impl Db {
    /// What a book contains, for a deletion prompt that says what it costs.
    pub async fn book_stats(&self, user: UserId, book_id: i64) -> Result<BookStats> {
        // One statement rather than four round trips. The book itself is in
        // the FROM clause, so a book that is not this user's yields no row and
        // reports NotFound instead of four zeroes, which would look like an
        // empty book rather than someone else's.
        let row = crate::sqlx::query(
            "SELECT
               (SELECT count(*) FROM pages WHERE book_id = b.id) AS pages,
               (SELECT count(*) FROM blocks bl
                  JOIN pages p ON p.id = bl.page_id
                 WHERE p.book_id = b.id AND bl.kind = 'paragraph') AS paragraphs,
               -- Distinct blocks summarised, not revisions: the reader thinks
               -- in paragraphs they have done, not in drafts they wrote.
               (SELECT count(DISTINCT s.block_id) FROM summaries s
                  JOIN blocks bl ON bl.id = s.block_id
                  JOIN pages p ON p.id = bl.page_id
                 WHERE p.book_id = b.id AND s.sentence_ordinal IS NULL) AS summaries,
               (SELECT count(*) FROM vocab WHERE book_id = b.id) AS words_looked_up
             FROM books b
             WHERE b.id = $1 AND b.user_id = $2",
        )
        .bind(book_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("book {book_id}")))?;

        Ok(BookStats {
            pages: row.get("pages"),
            paragraphs: row.get("paragraphs"),
            summaries: row.get("summaries"),
            words_looked_up: row.get("words_looked_up"),
        })
    }

    /// Add a page. The page number is left unknown until the model reads it.
    ///
    /// Reports [`AppError::Invalid`] if this exact photograph is already in
    /// the book, naming the page it duplicates.
    pub async fn add_page(
        &self,
        user: UserId,
        book_id: i64,
        image_hash: &str,
        image_orig: &str,
        image_proc: &str,
    ) -> Result<i64> {
        if let Some((existing_id, page_no)) = self.page_with_hash(user, book_id, image_hash).await? {
            return Err(AppError::Invalid(match page_no {
                Some(n) => format!("this photograph is already in the book as page {n}"),
                None => format!("this photograph is already in the book (page id {existing_id})"),
            }));
        }

        // INSERT ... SELECT so the ownership test and the write are one
        // statement. A book that is not this user's produces no row to insert.
        let row = crate::sqlx::query(
            "INSERT INTO pages (book_id, image_hash, image_orig, image_proc, ocr_status)
             SELECT b.id, $2, $3, $4, 'pending' FROM books b
             WHERE b.id = $1 AND b.user_id = $5
             RETURNING id",
        )
        .bind(book_id)
        .bind(image_hash)
        .bind(image_orig)
        .bind(image_proc)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("book {book_id}")))?;

        Ok(row.get("id"))
    }

    /// Remove a page and everything derived from it.
    ///
    /// Returns the image paths so the caller can delete the files too; leaving
    /// multi-megabyte photographs behind for a page the reader deleted is how
    /// a library quietly fills a disk.
    pub async fn delete_page(&self, user: UserId, page_id: i64) -> Result<Vec<String>> {
        // DELETE ... RETURNING gets the paths and does the deletion at once,
        // so there is no chance of reporting paths for a row that then failed
        // to delete. Blocks, summaries and critiques cascade.
        let row = crate::sqlx::query(
            "DELETE FROM pages p
             USING books b
             WHERE p.id = $1 AND b.id = p.book_id AND b.user_id = $2
             RETURNING p.image_orig, p.image_proc",
        )
        .bind(page_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("page {page_id}")))?;

        let mut out = vec![row.get::<String, _>("image_orig")];
        out.extend(row.get::<Option<String>, _>("image_proc"));
        Ok(out)
    }

    /// An existing page in this book with the same image, as (id, page_no).
    pub async fn page_with_hash(
        &self,
        user: UserId,
        book_id: i64,
        hash: &str,
    ) -> Result<Option<(i64, Option<i64>)>> {
        let row = crate::sqlx::query(
            "SELECT p.id, p.page_no FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND p.image_hash = $2 AND b.user_id = $3",
        )
        .bind(book_id)
        .bind(hash)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.map(|r| (r.get("id"), r.get("page_no"))))
    }

    /// Add a page whose text is already known — from an EPUB, a PDF, or a web
    /// page — together with its blocks, in one transaction.
    ///
    /// No OCR, no image, and no `image_hash`: deduplication for these sources
    /// is by document, handled before this is called.
    pub async fn add_text_page(
        &self,
        user: UserId,
        book_id: i64,
        source_kind: &str,
        source_ref: Option<&str>,
        origin: &str,
        page_no: Option<i64>,
        blocks: &[DraftBlock],
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await.map_err(to_app_error)?;

        let row = crate::sqlx::query(
            "INSERT INTO pages
               (book_id, page_no, page_no_source, source_kind, source_ref,
                image_orig, ocr_status)
             SELECT b.id, $2, $3, $4, $5, $6, 'done' FROM books b
             WHERE b.id = $1 AND b.user_id = $7
             RETURNING id",
        )
        .bind(book_id)
        .bind(page_no)
        .bind(if page_no.is_some() { "detected" } else { "unknown" })
        .bind(source_kind)
        .bind(source_ref)
        .bind(origin)
        .bind(user.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("book {book_id}")))?;

        let page_id: i64 = row.get("id");
        insert_blocks(&mut tx, page_id, blocks).await?;

        tx.commit().await.map_err(to_app_error)?;
        Ok(page_id)
    }

    /// Has this document already been imported into this book?
    pub async fn has_source(&self, user: UserId, book_id: i64, origin: &str) -> Result<bool> {
        let row = crate::sqlx::query(
            "SELECT 1 FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND p.image_orig = $2 AND b.user_id = $3
             LIMIT 1",
        )
        .bind(book_id)
        .bind(origin)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.is_some())
    }

    pub async fn set_page_source_ref(
        &self,
        user: UserId,
        page_id: i64,
        source_ref: &str,
    ) -> Result<()> {
        let done = crate::sqlx::query(
            "UPDATE pages p SET source_ref = $2
             FROM books b
             WHERE p.id = $1 AND b.id = p.book_id AND b.user_id = $3",
        )
        .bind(page_id)
        .bind(source_ref)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "page", page_id)
    }

    /// The other half of the photograph this page came from.
    ///
    /// Returns the sibling's id, its page number, and which side *this* page
    /// is — enough to reconcile two facing pages, which are always
    /// consecutive.
    pub async fn spread_sibling(
        &self,
        user: UserId,
        page_id: i64,
    ) -> Result<Option<(i64, Option<i64>, String)>> {
        let Some(row) = crate::sqlx::query(
            "SELECT p.book_id, p.source_ref FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.id = $1 AND b.user_id = $2",
        )
        .bind(page_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        else {
            return Ok(None);
        };

        let book_id: i64 = row.get("book_id");
        let Some(source_ref) = row.get::<Option<String>, _>("source_ref") else {
            return Ok(None);
        };
        let Some((stamp, side)) = source_ref.split_once(':') else {
            return Ok(None);
        };
        let other = if side == "left" { "right" } else { "left" };

        let sibling = crate::sqlx::query(
            "SELECT id, page_no FROM pages
             WHERE book_id = $1 AND source_ref = $2 AND id <> $3",
        )
        .bind(book_id)
        .bind(format!("{stamp}:{other}"))
        .bind(page_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(sibling.map(|r| (r.get("id"), r.get("page_no"), side.to_string())))
    }

    /// Page numbers already in use by other pages of this book.
    pub async fn page_numbers_excluding(
        &self,
        user: UserId,
        book_id: i64,
        exclude: &[i64],
    ) -> Result<Vec<i64>> {
        // Filtered in SQL with = ANY rather than pulled back and filtered in
        // Rust, as the SQLite version had to.
        let rows = crate::sqlx::query(
            "SELECT p.page_no FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND b.user_id = $2
               AND p.page_no IS NOT NULL
               AND NOT (p.id = ANY($3))",
        )
        .bind(book_id)
        .bind(user.get())
        .bind(exclude)
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows.iter().map(|r| r.get("page_no")).collect())
    }

    /// Record the page number the model read off the page.
    pub async fn set_page_number(
        &self,
        user: UserId,
        page_id: i64,
        page_no: Option<i64>,
        source: &str,
    ) -> Result<()> {
        let done = crate::sqlx::query(
            "UPDATE pages p SET page_no = $2, page_no_source = $3
             FROM books b
             WHERE p.id = $1 AND b.id = p.book_id AND b.user_id = $4",
        )
        .bind(page_id)
        .bind(page_no)
        .bind(source)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "page", page_id)
    }

    /// Another page in the book already claiming this number, if any.
    ///
    /// Not an error — the reader may be re-photographing a page they were
    /// unhappy with — but worth telling them about.
    pub async fn page_with_number(
        &self,
        user: UserId,
        book_id: i64,
        page_no: i64,
        excluding: i64,
    ) -> Result<Option<i64>> {
        let row = crate::sqlx::query(
            "SELECT p.id FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND p.page_no = $2 AND p.id <> $3 AND b.user_id = $4",
        )
        .bind(book_id)
        .bind(page_no)
        .bind(excluding)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.map(|r| r.get("id")))
    }

    pub async fn set_page_status(
        &self,
        user: UserId,
        page_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let done = crate::sqlx::query(
            "UPDATE pages p SET ocr_status = $2, ocr_error = $3
             FROM books b
             WHERE p.id = $1 AND b.id = p.book_id AND b.user_id = $4",
        )
        .bind(page_id)
        .bind(status)
        .bind(error)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "page", page_id)
    }

    /// Pages in reading order: by the number printed on the page where we know
    /// it, and by import time for pages not yet transcribed.
    pub async fn list_pages(&self, user: UserId, book_id: i64) -> Result<Vec<Page>> {
        let rows = crate::sqlx::query(
            "SELECT p.id, p.book_id, p.page_no, p.page_no_source, p.source_kind,
                    p.source_ref, p.image_orig, p.image_proc, p.ocr_status,
                    p.ocr_model, p.ocr_error
             FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND b.user_id = $2
             ORDER BY p.page_no NULLS LAST, p.created_at, p.id",
        )
        .bind(book_id)
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows.iter().map(page_from_row).collect())
    }

    /// Store the OCR result and the blocks derived from it.
    ///
    /// The raw output is kept alongside the blocks so segmentation can be
    /// re-run and improved later without going back to the model.
    pub async fn save_ocr_result(
        &self,
        user: UserId,
        page_id: i64,
        model: &str,
        raw: &str,
        blocks: &[DraftBlock],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(to_app_error)?;

        let done = crate::sqlx::query(
            "UPDATE pages p
                SET ocr_raw = $2, ocr_model = $3, ocr_status = 'done', ocr_error = NULL
             FROM books b
             WHERE p.id = $1 AND b.id = p.book_id AND b.user_id = $4",
        )
        .bind(page_id)
        .bind(raw)
        .bind(model)
        .bind(user.get())
        .execute(&mut *tx)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "page", page_id)?;

        // Re-segmentation replaces blocks wholesale; anything the reader
        // edited by hand is protected by the caller, not here.
        crate::sqlx::query("DELETE FROM blocks WHERE page_id = $1")
            .bind(page_id)
            .execute(&mut *tx)
            .await
            .map_err(to_app_error)?;

        insert_blocks(&mut tx, page_id, blocks).await?;

        tx.commit().await.map_err(to_app_error)?;
        Ok(())
    }

}

// --- Blocks -----------------------------------------------------------------

impl Db {
    pub async fn list_blocks(&self, user: UserId, page_id: i64) -> Result<Vec<Block>> {
        let rows = crate::sqlx::query(
            "SELECT bl.id, bl.page_id, bl.ordinal, bl.kind, bl.text_raw,
                    bl.text_norm, bl.user_edited
             FROM blocks bl
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE bl.page_id = $1 AND b.user_id = $2
             ORDER BY bl.ordinal",
        )
        .bind(page_id)
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows.iter().map(block_from_row).collect())
    }

    /// Which book a page belongs to. Needed to resolve a block's era without
    /// walking the whole library.
    pub async fn book_id_for_page(&self, user: UserId, page_id: i64) -> Result<i64> {
        let row = crate::sqlx::query(
            "SELECT p.book_id FROM pages p
             JOIN books b ON b.id = p.book_id
             WHERE p.id = $1 AND b.user_id = $2",
        )
        .bind(page_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("page {page_id}")))?;

        Ok(row.get("book_id"))
    }

    pub async fn get_block(&self, user: UserId, id: i64) -> Result<Block> {
        let row = crate::sqlx::query(
            "SELECT bl.id, bl.page_id, bl.ordinal, bl.kind, bl.text_raw,
                    bl.text_norm, bl.user_edited
             FROM blocks bl
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE bl.id = $1 AND b.user_id = $2",
        )
        .bind(id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("block {id}")))?;

        Ok(block_from_row(&row))
    }

    /// The page immediately before or after this one in reading order.
    ///
    /// Adjacency follows the number printed on the page, so a book
    /// photographed out of sequence still knows which leaf precedes which.
    /// Unnumbered pages sort after numbered ones and among themselves by
    /// import time, so they stay in the sequence rather than dropping out.
    async fn adjacent_page(&self, user: UserId, page_id: i64, next: bool) -> Result<Option<i64>> {
        // Postgres compares row values directly, so the three-way ordering the
        // SQLite version spelled out as a disjunction is one comparison here.
        // COALESCE puts unnumbered pages at the end rather than nowhere.
        let sql = if next {
            "SELECT p.id FROM pages p, pages cur, books b
             WHERE cur.id = $1 AND p.book_id = cur.book_id
               AND b.id = cur.book_id AND b.user_id = $2
               AND (COALESCE(p.page_no, 9223372036854775807), p.created_at, p.id)
                 > (COALESCE(cur.page_no, 9223372036854775807), cur.created_at, cur.id)
             ORDER BY COALESCE(p.page_no, 9223372036854775807), p.created_at, p.id
             LIMIT 1"
        } else {
            "SELECT p.id FROM pages p, pages cur, books b
             WHERE cur.id = $1 AND p.book_id = cur.book_id
               AND b.id = cur.book_id AND b.user_id = $2
               AND (COALESCE(p.page_no, 9223372036854775807), p.created_at, p.id)
                 < (COALESCE(cur.page_no, 9223372036854775807), cur.created_at, cur.id)
             ORDER BY COALESCE(p.page_no, 9223372036854775807) DESC, p.created_at DESC, p.id DESC
             LIMIT 1"
        };

        let row = crate::sqlx::query(sql)
            .bind(page_id)
            .bind(user.get())
            .fetch_optional(&self.pool)
            .await
            .map_err(to_app_error)?;

        Ok(row.map(|r| r.get("id")))
    }

    pub async fn next_page_id(&self, user: UserId, page_id: i64) -> Result<Option<i64>> {
        self.adjacent_page(user, page_id, true).await
    }

    pub async fn previous_page_id(&self, user: UserId, page_id: i64) -> Result<Option<i64>> {
        self.adjacent_page(user, page_id, false).await
    }

    /// The first or last prose block of a page, skipping headings and other
    /// furniture — a paragraph continues into prose, not into a running head.
    pub async fn edge_paragraph(
        &self,
        user: UserId,
        page_id: i64,
        first: bool,
    ) -> Result<Option<Block>> {
        let blocks = self.list_blocks(user, page_id).await?;
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
    pub async fn delete_block(&self, user: UserId, block_id: i64) -> Result<()> {
        let done = crate::sqlx::query(
            "DELETE FROM blocks bl
             USING pages p, books b
             WHERE bl.id = $1 AND p.id = bl.page_id AND b.id = p.book_id
               AND b.user_id = $2",
        )
        .bind(block_id)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "block", block_id)
    }

    /// Change what a block is: prose, a heading, a quotation.
    ///
    /// Heading detection works from shape alone and cannot always be right —
    /// an italic section title reads as an ordinary sentence. Being able to
    /// say so keeps such a line out of the summarising queue and puts it into
    /// the book's outline where it belongs.
    pub async fn set_block_kind(&self, user: UserId, block_id: i64, kind: &str) -> Result<()> {
        const KINDS: [&str; 5] = ["paragraph", "heading", "quote", "footnote", "caption"];
        if !KINDS.contains(&kind) {
            return Err(AppError::Invalid(format!("{kind} is not a kind of block")));
        }

        let done = crate::sqlx::query(
            "UPDATE blocks bl SET kind = $2, user_edited = TRUE
             FROM pages p, books b
             WHERE bl.id = $1 AND p.id = bl.page_id AND b.id = p.book_id
               AND b.user_id = $3",
        )
        .bind(block_id)
        .bind(kind)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "block", block_id)
    }

    /// Correct a block by hand. OCR is never perfect, and a reader with no way
    /// to fix a mangled paragraph cannot use the application at all.
    pub async fn edit_block(&self, user: UserId, id: i64, text: &str) -> Result<()> {
        let done = crate::sqlx::query(
            "UPDATE blocks bl SET text_norm = $2, user_edited = TRUE
             FROM pages p, books b
             WHERE bl.id = $1 AND p.id = bl.page_id AND b.id = p.book_id
               AND b.user_id = $3",
        )
        .bind(id)
        .bind(text)
        .bind(user.get())
        .execute(&self.pool)
        .await
        .map_err(to_app_error)?;

        missing_unless_touched(done.rows_affected(), "block", id)
    }
}

// --- Summaries, critiques, and the spine ------------------------------------

impl Db {
    /// Save a summary as a new revision, preserving earlier attempts so the
    /// reader can see their own thinking improve.
    ///
    /// `sentence_ordinal` is `None` for a summary of the whole paragraph and
    /// `Some(i)` for the i-th sentence within it. Revisions are counted per
    /// target, so revising one sentence does not bump the paragraph's.
    pub async fn save_summary(
        &self,
        user: UserId,
        block_id: i64,
        sentence_ordinal: Option<i64>,
        sentence: &str,
        self_checked: bool,
    ) -> Result<i64> {
        // One statement: the ownership test, the revision number, and the
        // insert. Computing the revision separately would leave a window in
        // which two saves could pick the same number.
        //
        // `IS NOT DISTINCT FROM` rather than `=` because sentence_ordinal is
        // nullable, and NULL = NULL is unknown — which would restart the
        // paragraph-level revision count at 1 every single time.
        let row = crate::sqlx::query(
            "INSERT INTO summaries (block_id, sentence_ordinal, sentence, revision, self_checked)
             SELECT bl.id, $2, $3,
                    COALESCE((SELECT max(s.revision) FROM summaries s
                               WHERE s.block_id = bl.id
                                 AND s.sentence_ordinal IS NOT DISTINCT FROM $2), 0) + 1,
                    $4
             FROM blocks bl
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE bl.id = $1 AND b.user_id = $5
             RETURNING id",
        )
        .bind(block_id)
        .bind(sentence_ordinal)
        .bind(sentence)
        .bind(self_checked)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("block {block_id}")))?;

        Ok(row.get("id"))
    }

    /// The reader's latest note on each sentence of a paragraph.
    pub async fn sentence_summaries(
        &self,
        user: UserId,
        block_id: i64,
    ) -> Result<Vec<SentenceSummary>> {
        // DISTINCT ON is the Postgres way to say "the newest row per group",
        // and replaces the correlated MAX(revision) subquery the SQLite
        // version needed.
        let rows = crate::sqlx::query(
            "SELECT DISTINCT ON (s.sentence_ordinal) s.sentence_ordinal, s.sentence
             FROM summaries s
             JOIN blocks bl ON bl.id = s.block_id
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE s.block_id = $1 AND s.sentence_ordinal IS NOT NULL
               AND b.user_id = $2
             ORDER BY s.sentence_ordinal, s.revision DESC",
        )
        .bind(block_id)
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows
            .iter()
            .map(|r| SentenceSummary {
                ordinal: r.get("sentence_ordinal"),
                text: r.get("sentence"),
            })
            .collect())
    }

    /// The latest paragraph-level summary for a block.
    pub async fn latest_summary(&self, user: UserId, block_id: i64) -> Result<Option<Summary>> {
        let row = crate::sqlx::query(
            "SELECT s.id, s.block_id, s.sentence, s.revision, s.self_checked, s.created_at
             FROM summaries s
             JOIN blocks bl ON bl.id = s.block_id
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE s.block_id = $1 AND s.sentence_ordinal IS NULL AND b.user_id = $2
             ORDER BY s.revision DESC
             LIMIT 1",
        )
        .bind(block_id)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(row.map(|r| Summary {
            id: r.get("id"),
            block_id: r.get("block_id"),
            sentence: r.get("sentence"),
            revision: r.get("revision"),
            self_checked: r.get("self_checked"),
            created_at: r
                .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .to_rfc3339(),
        }))
    }

    pub async fn save_critique(
        &self,
        user: UserId,
        summary_id: i64,
        verdict: &str,
        covers: (bool, bool, bool),
        problem: &str,
        steering_question: &str,
        model: &str,
    ) -> Result<i64> {
        let row = crate::sqlx::query(
            "INSERT INTO critiques
               (summary_id, verdict, covers_subject, covers_action, covers_reason,
                problem, steering_question, model)
             SELECT s.id, $2, $3, $4, $5, $6, $7, $8
             FROM summaries s
             JOIN blocks bl ON bl.id = s.block_id
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE s.id = $1 AND b.user_id = $9
             RETURNING id",
        )
        .bind(summary_id)
        .bind(verdict)
        .bind(covers.0)
        .bind(covers.1)
        .bind(covers.2)
        .bind(problem)
        .bind(steering_question)
        .bind(model)
        .bind(user.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("summary {summary_id}")))?;

        Ok(row.get("id"))
    }

    /// The reader's accumulated one-sentence summaries for a book, in reading
    /// order — the "spine" that the build-up step assembles.
    pub async fn summary_spine(&self, user: UserId, book_id: i64) -> Result<Vec<SpineEntry>> {
        let rows = crate::sqlx::query(
            "SELECT p.page_no, bl.ordinal, bl.id AS block_id, s.sentence
             FROM (
               SELECT DISTINCT ON (block_id) block_id, sentence
               FROM summaries
               WHERE sentence_ordinal IS NULL
               ORDER BY block_id, revision DESC
             ) s
             JOIN blocks bl ON bl.id = s.block_id
             JOIN pages p ON p.id = bl.page_id
             JOIN books b ON b.id = p.book_id
             WHERE p.book_id = $1 AND b.user_id = $2
             ORDER BY p.page_no NULLS LAST, p.created_at, bl.ordinal",
        )
        .bind(book_id)
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows
            .iter()
            .map(|r| SpineEntry {
                page_no: r.get("page_no"),
                ordinal: r.get("ordinal"),
                block_id: r.get("block_id"),
                sentence: r.get("sentence"),
            })
            .collect())
    }
}

// --- Lookups and vocabulary -------------------------------------------------

impl Db {
    /// Record a word the reader looked up, with the sentence they met it in.
    ///
    /// The sentence is the point: a vocabulary list of bare words is nearly
    /// useless, while one that shows where you met each word is reviewable.
    pub async fn record_lookup(
        &self,
        user: UserId,
        book_id: i64,
        block_id: i64,
        word: &str,
        lemma: &str,
        sentence: &str,
        gloss: &str,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await.map_err(to_app_error)?;

        let row = crate::sqlx::query(
            "INSERT INTO lookups (book_id, block_id, word, lemma, sentence, llm_gloss)
             SELECT b.id, $2, $3, $4, $5, $6 FROM books b
             WHERE b.id = $1 AND b.user_id = $7
             RETURNING id",
        )
        .bind(book_id)
        .bind(block_id)
        .bind(word)
        .bind(lemma)
        .bind(sentence)
        .bind(gloss)
        .bind(user.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(to_app_error)?
        .ok_or_else(|| AppError::NotFound(format!("book {book_id}")))?;

        let lookup_id: i64 = row.get("id");

        // Second and later encounters bump the count rather than duplicating.
        // `vocab.count` is qualified because EXCLUDED has one too.
        crate::sqlx::query(
            "INSERT INTO vocab (book_id, word, lemma, count, first_lookup_id)
             VALUES ($1, $2, $3, 1, $4)
             ON CONFLICT (book_id, word)
             DO UPDATE SET count = vocab.count + 1",
        )
        .bind(book_id)
        .bind(word.to_lowercase())
        .bind(lemma)
        .bind(lookup_id)
        .execute(&mut *tx)
        .await
        .map_err(to_app_error)?;

        tx.commit().await.map_err(to_app_error)?;
        Ok(lookup_id)
    }

    /// Words looked up in this book, most frequent first.
    pub async fn vocabulary(&self, user: UserId, book_id: i64) -> Result<Vec<VocabEntry>> {
        let rows = crate::sqlx::query(
            "SELECT v.word, v.lemma, v.count, l.sentence, l.llm_gloss
             FROM vocab v
             JOIN books b ON b.id = v.book_id
             LEFT JOIN lookups l ON l.id = v.first_lookup_id
             WHERE v.book_id = $1 AND b.user_id = $2
             ORDER BY v.count DESC, v.word",
        )
        .bind(book_id)
        .bind(user.get())
        .fetch_all(&self.pool)
        .await
        .map_err(to_app_error)?;

        Ok(rows
            .iter()
            .map(|r| VocabEntry {
                word: r.get("word"),
                lemma: r.get("lemma"),
                count: r.get("count"),
                sentence: r.get("sentence"),
                gloss: r.get("llm_gloss"),
            })
            .collect())
    }
}

/// Insert a page's blocks. Shared by the OCR path and the text-import path,
/// which differ in how the text was obtained and in nothing after that.
async fn insert_blocks(
    tx: &mut sqlx_core::transaction::Transaction<'_, crate::sqlx::Postgres>,
    page_id: i64,
    blocks: &[DraftBlock],
) -> Result<()> {
    for (i, b) in blocks.iter().enumerate() {
        crate::sqlx::query(
            "INSERT INTO blocks (page_id, ordinal, kind, text_raw, text_norm)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(page_id)
        .bind(i as i64)
        .bind(b.kind.as_str())
        .bind(&b.text_raw)
        .bind(&b.text_norm)
        .execute(&mut **tx)
        .await
        .map_err(to_app_error)?;
    }
    Ok(())
}

/// Nothing updated means the row is absent *or* belongs to someone else, and
/// those are reported the same way on purpose.
fn missing_unless_touched(rows: u64, what: &str, id: i64) -> Result<()> {
    if rows == 0 {
        return Err(AppError::NotFound(format!("{what} {id}")));
    }
    Ok(())
}

fn page_from_row(row: &PgRow) -> Page {
    Page {
        id: row.get("id"),
        book_id: row.get("book_id"),
        page_no: row.get("page_no"),
        page_no_source: row.get("page_no_source"),
        source_kind: row.get("source_kind"),
        source_ref: row.get("source_ref"),
        image_orig: row.get("image_orig"),
        image_proc: row.get("image_proc"),
        ocr_status: row.get("ocr_status"),
        ocr_model: row.get("ocr_model"),
        ocr_error: row.get("ocr_error"),
    }
}

fn block_from_row(row: &PgRow) -> Block {
    Block {
        id: row.get("id"),
        page_id: row.get("page_id"),
        ordinal: row.get("ordinal"),
        kind: row.get("kind"),
        text_raw: row.get("text_raw"),
        text_norm: row.get("text_norm"),
        user_edited: row.get("user_edited"),
    }
}

fn book_from_row(row: &PgRow) -> Book {
    Book {
        id: row.get("id"),
        title: row.get("title"),
        author: row.get("author"),
        era: row.get("era"),
        dict_corpus: row.get("dict_corpus"),
        last_page_id: row.get("last_page_id"),
        created_at: row
            .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
            .to_rfc3339(),
        updated_at: row
            .get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
            .to_rfc3339(),
    }
}

/// `AppError` predates Postgres, so `crate::sqlx::Error` has nowhere of its own to
/// go. Kept as one function rather than a `From` impl to leave room for the
/// call sites that want to recognise a specific database error first — a
/// unique violation on an email is a message about that address, not a
/// database failure.
fn to_app_error(e: crate::sqlx::Error) -> AppError {
    match e {
        crate::sqlx::Error::RowNotFound => AppError::NotFound("row".into()),
        other => AppError::Other(anyhow::anyhow!("database error: {other}")),
    }
}
