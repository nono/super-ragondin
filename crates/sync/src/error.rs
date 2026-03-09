use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Store error: {0}")]
    Store(#[from] fjall::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Invalid MD5 hash: {0}")]
    InvalidMd5(String),

    #[error("Missing document in change result for id: {0}")]
    MissingDocument(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Revision mismatch: expected {expected}, got {actual}")]
    RevisionMismatch { expected: String, actual: String },

    #[error("Retryable error: {0}")]
    Retryable(String),

    #[error("Permanent error: {0}")]
    Permanent(String),
}

impl Error {
    /// Returns true if the error is retryable (network issues, rate limits, server errors)
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_) | Self::Http(_))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
