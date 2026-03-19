use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Invalid MD5 hash: {0}")]
    InvalidMd5(String),

    #[error("Missing document in change result for id: {0}")]
    MissingDocument(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Revision mismatch: expected {expected}, got {actual}")]
    RevisionMismatch { expected: String, actual: String },
}

pub type Result<T> = std::result::Result<T, Error>;
