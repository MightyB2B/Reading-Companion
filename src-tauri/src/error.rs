//! What can go wrong between the window and the server.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not reach the library server at {url}. Is it running? ({source})")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// The server answered, and said why. Its message is already written for
    /// a reader, so it is passed through rather than wrapped.
    #[error("{message}")]
    Server { status: u16, message: String },

    #[error("the server sent something unexpected: {0}")]
    BadResponse(String),

    #[error("{0}")]
    Invalid(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl ClientError {
    /// Is this the frontend needing to show a sign-in screen?
    pub fn is_unauthenticated(&self) -> bool {
        matches!(self, ClientError::Server { status: 401, .. })
    }
}

/// Serialise to a plain string, so the frontend receives a readable message
/// rather than an enum shape it would have to destructure.
impl Serialize for ClientError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;
