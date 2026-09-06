//! Errors returned when acquiring an inhibitor guard.
//!
//! [`Inhibitor::acquire`](super::Inhibitor::acquire) returns
//! [`InhibitorError::BatteryTooLow`] for [`super::Kind::Full`] when capacity is below
//! [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::FULL_INHIBIT_MIN_CAPACITY_PERCENT)
//! or cannot be read. SoftSuspend acquires do not produce this error.

use thiserror::Error;

/// Errors returned when acquiring an inhibitor guard.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InhibitorError {
    /// Battery capacity is below [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::FULL_INHIBIT_MIN_CAPACITY_PERCENT).
    ///
    /// Returned only for [`Kind::Full`](crate::device::inhibitor::Kind::Full).
    /// Callers such as OTA must abort critical work and surface a user-visible
    /// failure rather than retrying without charging.
    #[error("battery too low to acquire Full inhibitor")]
    BatteryTooLow,
}
