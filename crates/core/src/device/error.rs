//! Device error types.

use thiserror::Error;

/// Errors that can occur during device operations.
#[derive(Error, Debug)]
pub enum DeviceError {
    /// Failed to read device metadata.
    #[error("Failed to read device metadata: {0}")]
    Metadata(String),
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
