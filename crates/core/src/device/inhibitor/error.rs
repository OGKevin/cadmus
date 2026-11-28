//! Errors returned by inhibitor operations.
//!
//! [`Inhibitor::acquire`](super::Inhibitor::acquire) returns
//! [`InhibitorError::BatteryTooLow`] for [`super::Kind::Sleep`] and
//! [`super::Kind::Full`] when capacity is below
//! [`INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::INHIBIT_MIN_CAPACITY_PERCENT)
//! or cannot be read. SoftSuspend acquires do not produce this error.

use thiserror::Error;

/// Errors returned by inhibitor operations.
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

    /// The kernel wake lock could not be confirmed during acquisition or shutdown.
    ///
    /// Full acquisition fails rather than promising protection the kernel does
    /// not have. Backend retries may still converge the shared lock later.
    #[error("kernel wake-lock write failed")]
    WakeLockWriteFailed,

    /// The wake-lock control worker stopped or dropped its reply.
    #[error("wake-lock control worker failed")]
    WorkerFailed,
}
