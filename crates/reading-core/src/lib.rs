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

pub mod coach;
pub mod db;
pub mod dict;
pub mod error;
pub mod ingest;
pub mod models;
pub mod ocr;
pub mod ollama;

pub use error::{AppError, Result};
