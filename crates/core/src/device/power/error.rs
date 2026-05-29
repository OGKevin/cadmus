//! Power management error types.

use thiserror::Error;

/// Errors that can occur during power management operations.
#[derive(Error, Debug)]
pub enum PowerError {
    /// Standard I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A mutex was poisoned by a panicking thread.
    #[error("mutex lock poisoned")]
    LockPoisoned,
}
