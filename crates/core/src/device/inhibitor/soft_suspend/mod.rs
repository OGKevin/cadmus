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
use crate::lease::{Lease, LeaseName};

/// SoftSuspend-kind backend: settings plus named wake-lock leases.
///
/// Object-safe so [`Inhibitor`](super::Inhibitor) can hold `Arc<dyn SoftSuspendKind>`.
pub trait SoftSuspendKind: SoftSuspendBackend {
    /// Acquires a named SoftSuspend wake-lock lease.
    ///
    /// On Linux this collapses onto the shared `cadmus` wake lock; on NoOp it
    /// only updates the in-process holder tracker.
    fn acquire_lease(&self, name: LeaseName) -> Lease;
}
