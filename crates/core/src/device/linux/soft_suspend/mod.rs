//! Soft suspend via Linux kernel autosleep and a single [`WAKE_LOCK_NAME`] wake lock.
//!
//! Arms `/sys/power/autosleep` and blocks sleep with `/sys/power/wake_lock`
//! while named leases are held. Missing sysfs nodes are a no-op.
//!
//! Portable [`crate::device::soft_suspend::SoftSuspend`] construction
//! (`from_system`, `from_paths`, `with_paths`) lives on that enum.
//!
//! See the [sysfs-power ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
//! for `autosleep`, `wake_lock`, and `wake_unlock`.

pub(crate) mod paths;
pub mod session;

/// Wake-lock name written to `/sys/power/wake_lock` while Cadmus holds soft suspend.
pub(crate) const WAKE_LOCK_NAME: &str = "cadmus";
