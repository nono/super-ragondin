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
}

pub type Result<T> = std::result::Result<T, Error>;
