//! Errors returned when acquiring an inhibitor guard.
//!
//! Reserved for Part 2 Full inhibit (e.g. [`InhibitorError::BatteryTooLow`]).
//! [`Inhibitor::acquire`] is infallible for
//! [`Kind::SoftSuspend`]; unimplemented kinds panic.

use thiserror::Error;

/// Errors returned when acquiring an inhibitor guard.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InhibitorError {
    /// Battery capacity is below the configured power-off threshold.
    ///
    /// Returned only for [`Kind::Full`]. Callers such as OTA
    /// must abort critical work and surface a user-visible failure rather than
    /// retrying without charging.
    #[error("battery too low to acquire Full inhibitor")]
    BatteryTooLow,
    /// Full inhibitor is not available on this build.
    #[error("Full inhibitor is not available")]
    FullUnsupported,
}
