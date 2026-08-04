//! Soft suspend via Linux kernel autosleep and a single `cadmus` wake lock.
//!
//! Arms `/sys/power/autosleep` and blocks sleep with `/sys/power/wake_lock`
//! while named leases are held. Missing sysfs nodes are a no-op.
//!
//! See the [sysfs-power ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
//! for `autosleep`, `wake_lock`, and `wake_unlock`. Device research that led to
//! this design is in
//! [soft suspend via autosleep and wake_lock](../../../../../guide/investigations/kobo/issue-361-autosleep-wake-lock.html).

mod mode;
mod paths;
mod session;

pub use mode::AutosleepMode;
pub use paths::{SoftSuspendPaths, discover_available_modes};
pub use session::{SoftSuspendLease, SoftSuspendSession};

pub(crate) const WAKE_LOCK_NAME: &str = "cadmus";
