//! SoftSuspend-kind backend contract and NoOp implementation.
//!
//! [`SoftSuspendKind`] is injected into [`Inhibitor`](super::Inhibitor). Linux
//! implementations live under [`crate::device::linux::soft_suspend`]; this
//! module holds the portable trait and the inert NoOp used on emulator and
//! failed probes.

mod name;
mod noop;

pub use name::SoftSuspendName;
pub(crate) use noop::NoOpSoftSuspendKind;

use crate::device::soft_suspend::SoftSuspendBackend;
use crate::lease::{Lease, LeaseName, WeakLeaseTracker};

/// Result of trying to suppress the shared kernel wake lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeLockPauseOutcome {
    /// The kernel wake lock is suppressed for the sleep window.
    Paused,
    /// A Full inhibit holder is active; the lock remains in normal operation.
    BlockedByFull,
}

/// SoftSuspend-kind backend: settings plus named wake-lock leases.
///
/// Object-safe so [`Inhibitor`](super::Inhibitor) can hold `Arc<dyn SoftSuspendKind>`.
pub trait SoftSuspendKind: SoftSuspendBackend {
    /// Acquires a named SoftSuspend wake-lock lease.
    ///
    /// On Linux this collapses onto the shared `cadmus` wake lock; on NoOp it
    /// only updates the in-process holder tracker.
    fn acquire_lease(&self, name: LeaseName) -> Lease;

    /// Wires the Full-holder tracker used to gate suppression.
    fn set_full_tracker(&self, tracker: WeakLeaseTracker);

    /// Releases the kernel wake lock for a Cadmus suspend sleep window.
    ///
    /// Holder accounting stays intact: leases acquired or dropped while paused
    /// still update the tracker, but do not write the kernel wake-lock nodes.
    /// Returns [`WakeLockPauseOutcome::BlockedByFull`] when a Full holder is
    /// active. NoOp backends have no kernel lock but report the same gate.
    fn pause_wake_lock(&self) -> WakeLockPauseOutcome;

    /// Ends a Cadmus suspend sleep window.
    ///
    /// Re-acquires the kernel wake lock when any SoftSuspend holder is still
    /// alive. NoOp backends have no kernel lock and do nothing.
    fn resume_wake_lock(&self);
}
