//! Error types for monolingual dictionary operations.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MonolingualError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] crate::http::HttpError),

    #[error("Failed to deserialize API response: {0}")]
    Deserialization(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(#[from] anyhow::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Dictionary not found for language: {0}")]
    NotFound(String),

    #[error("Request failed: {0}")]
    Request(String),

    #[error("Failed to extract dictionary archive: {0}")]
    Extraction(String),

    #[error("Installation already in progress for language: {0}")]
    InstallationInProgress(String),
}
