//! Errors returned when acquiring an inhibitor guard.
//!
//! [`Inhibitor::acquire`](super::Inhibitor::acquire) returns
//! [`InhibitorError::BatteryTooLow`] for [`super::Kind::Sleep`] and
//! [`super::Kind::Full`] when capacity is below
//! [`INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::INHIBIT_MIN_CAPACITY_PERCENT)
//! or cannot be read. SoftSuspend acquires do not produce this error.

use thiserror::Error;

/// Errors returned when acquiring an inhibitor guard.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InhibitorError {
    /// Battery capacity is below [`INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::INHIBIT_MIN_CAPACITY_PERCENT)
    /// or cannot be read.
    ///
    /// Returned for [`Kind::Sleep`](crate::device::inhibitor::Kind::Sleep) and
    /// [`Kind::Full`](crate::device::inhibitor::Kind::Full). Callers such as OTA
    /// must abort critical work and surface a user-visible failure rather than
    /// retrying without charging; other holders carry on without the lease.
    #[error("battery too low to acquire inhibitor")]
    BatteryTooLow,
}
