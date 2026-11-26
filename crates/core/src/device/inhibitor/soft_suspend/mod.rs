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

use crate::device::inhibitor::InhibitorError;
use crate::device::soft_suspend::SoftSuspendBackend;
use crate::lease::{Lease, LeaseName, WeakLeaseTracker};

/// Monotonic token identifying one wake-lock suppression window.
///
/// Full interruption or resume advances the epoch, so an older pause can never
/// be mistaken for the current suppression even after the holder disappeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuppressionEpoch(pub(crate) u64);

/// Result of trying to suppress the shared kernel wake lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeLockPauseOutcome {
    /// The kernel wake lock is suppressed for this sleep window.
    Paused(SuppressionEpoch),
    /// A Full inhibit holder is active; the lock remains in normal operation.
    BlockedByFull,
    /// Suppression could not be confirmed because the unlock worker failed.
    /// Callers must not enter a sleep leg assuming the lock was released.
    UnlockFailed,
}

/// Result of restoring the shared kernel wake lock after a sleep window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeLockResumeOutcome {
    /// SoftSuspend holders remain and the kernel lock was re-acquired.
    Restored,
    /// No SoftSuspend holders remain, so the lock stays released.
    Unheld,
    /// Restoration could not be confirmed because a sysfs write failed.
    RestoreFailed,
    /// The control worker stopped or closed its request channel.
    WorkerFailed,
}

/// SoftSuspend-kind backend: settings plus named wake-lock leases.
///
/// Object-safe so [`Inhibitor`](super::Inhibitor) can hold `Arc<dyn SoftSuspendKind>`.
#[async_trait::async_trait]
pub trait SoftSuspendKind: SoftSuspendBackend {
    /// Registers lease ownership without starting hardware I/O.
    fn register_lease(&self, name: LeaseName) -> Lease;

    /// Confirms Linux wake-lock demand after registration.
    ///
    /// Suppression remains respected. Full uses [`Self::resume_wake_lock`]
    /// instead to invalidate the previous suspension epoch deliberately.
    ///
    /// # Errors
    ///
    /// Returns [`InhibitorError::WorkerFailed`] if the control worker stopped.
    async fn confirm_leases(&self) -> Result<(), InhibitorError>;

    /// Registers a lease and waits for the worker's reconciliation attempt.
    ///
    /// Linux preserves the logical lease when a write fails and a bounded
    /// background retry is scheduled, matching the backend's eventual lease
    /// behavior. NoOp only updates its in-process tracker.
    async fn acquire_lease(&self, name: LeaseName) -> Result<Lease, InhibitorError> {
        let lease = self.register_lease(name);
        self.confirm_leases().await?;
        Ok(lease)
    }

    /// Wires the Full-holder tracker used to gate suppression.
    fn set_full_tracker(&self, tracker: WeakLeaseTracker);

    /// Releases the kernel wake lock for a Cadmus suspend sleep window.
    ///
    /// Holder accounting stays intact: leases acquired or dropped while paused
    /// still update the tracker, but do not write the kernel wake-lock nodes.
    /// Returns [`WakeLockPauseOutcome::BlockedByFull`] when a Full holder is
    /// active. NoOp backends have no kernel lock but report the same gate.
    async fn pause_wake_lock(&self) -> WakeLockPauseOutcome;

    /// Ends a Cadmus suspend sleep window.
    ///
    /// Re-acquires the kernel wake lock when any SoftSuspend holder is still
    /// alive and reports the confirmed outcome. NoOp backends have no kernel
    /// lock but report the same holder-derived state.
    async fn resume_wake_lock(&self) -> WakeLockResumeOutcome;

    /// Stops hardware transitions and releases the lock through the backend worker.
    async fn shutdown(&self) -> Result<(), InhibitorError> {
        Ok(())
    }

    /// Whether `epoch` still identifies the live suppression window.
    ///
    /// This reads only in-memory state, never sysfs, so polling does not need
    /// to offload a check onto the blocking pool.
    fn wake_lock_epoch_is_current(&self, epoch: SuppressionEpoch) -> bool;
}
