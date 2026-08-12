//! Reading Companion's engine.
//!
//! Everything the application does that is not drawing: the library database,
//! the OCR pipeline, paragraph segmentation, the period dictionary, and the
//! Socratic coach.
//!
//! Deliberately free of any UI or transport dependency. The desktop app and
//! the HTTP server are both callers of this crate rather than owners of it,
//! which is what lets the same library run in-process on a laptop and on a
//! server with a real GPU without the logic existing twice.

/// The parts of sqlx this crate uses, gathered under the name they would have
/// had anyway.
///
/// We depend on `sqlx-core` and `sqlx-postgres` directly rather than on the
/// `sqlx` facade, because the facade declares an optional SQLite driver whose
/// `libsqlite3-sys` version collides with the one `rusqlite` needs — and
/// cargo settles versions before it prunes unused features, so the feature
/// being off does not help. See the workspace Cargo.toml.
///
/// This module exists so that cost is paid once, here, instead of by every
/// call site spelling out `sqlx_core::query::query`.
pub mod sqlx {
    pub use sqlx_core::error::Error;
    pub use sqlx_core::query::query;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

pub mod auth;
pub mod coach;
pub mod db;
pub mod dict;
pub mod error;
pub mod ingest;
pub mod models;
pub mod ocr;
pub mod ollama;

pub use error::{AppError, Result};
