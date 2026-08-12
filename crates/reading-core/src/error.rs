use serde::{Serialize, Serializer};

/// Anything that can go wrong in a Tauri command.
///
/// Ollama being unreachable is by far the most common failure in normal use, so
/// it gets its own variant with an actionable message rather than surfacing as
/// an opaque reqwest error.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error(
        "could not reach Ollama at {url}. Is it running? Start it with `ollama serve`. ({source})"
    )]
    OllamaUnreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("Ollama returned {status}: {body}")]
    OllamaStatus { status: u16, body: String },

    /// The request reached Ollama but reading or decoding the response failed.
    #[error("failed reading Ollama response: {0}")]
    OllamaResponse(#[from] reqwest::Error),

    /// The model emitted something that did not match the requested JSON schema.
    #[error("model returned malformed output: {0}")]
    BadModelOutput(String),

    #[error("{0} not found")]
    NotFound(String),

    #[error("{0}")]
    Invalid(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::BadModelOutput(e.to_string())
    }
}

/// Serialise to a plain string so the frontend receives a readable message
/// instead of an enum shape it would have to destructure.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
