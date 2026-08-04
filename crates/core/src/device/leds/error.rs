//! LED control error types.

use thiserror::Error;

/// Errors that can occur while controlling device LEDs.
#[derive(Error, Debug)]
pub enum LedsError {
    /// Standard I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
