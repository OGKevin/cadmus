//! Soft suspend via Linux kernel autosleep and a single [`WAKE_LOCK_NAME`] wake lock.
//!
//! Arms `/sys/power/autosleep` and blocks sleep with `/sys/power/wake_lock`
//! while named leases are held. Missing sysfs nodes are a no-op.
//!
//! This module holds the Linux SoftSuspend-kind implementation (wake lock,
//! autosleep policy, soft-indicate) built by device probe and injected into
//! [`Inhibitor`](crate::device::inhibitor::Inhibitor). Settings configure that
//! inhibitor through [`SoftSuspendBackend`](crate::device::soft_suspend::SoftSuspendBackend).
//!
//! See the [sysfs-power ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
//! for `autosleep`, `wake_lock`, and `wake_unlock`.

pub(crate) mod autosleep;
pub(crate) mod kind;
pub(crate) mod paths;
#[cfg(all(test, target_os = "linux"))]
mod session_tests;
pub(crate) mod wake;

/// Wake-lock name written to `/sys/power/wake_lock` while Cadmus holds soft suspend.
pub(crate) const WAKE_LOCK_NAME: &str = "cadmus";
