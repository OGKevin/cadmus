//! RAII guards for inhibitor leases.
//!
//! [`InhibitorGuard`] is returned from [`Inhibitor::acquire`](super::Inhibitor::acquire).
//! Drop the guard to release the lease. On Linux [`Kind::SoftSuspend`],
//! the last guard dropping may start the configured release-grace timer before
//! `wake_unlock` is written. The last [`Kind::Full`] drop also releases the
//! nested `"full-inhibit"` wake lock / LED and may fire the Full last-release
//! notifier.

use super::Kind;
use crate::lease::Lease;

/// RAII guard holding an inhibitor lease.
///
/// Drop the guard to release the lease. On Linux [`Kind::SoftSuspend`], the last
/// guard dropping may start the release-grace timer before `wake_unlock` is
/// written. On [`Kind::Full`], dropping the last named holder (for example
/// `"ota"`) runs the Full 1→0 transition (nested wake lock, LED, notifier).
///
/// Holders that reboot must drop Full **first**, then send the reboot event,
/// so the notifier does not start a suspend cycle across reboot. OTA sends
/// [`Event::ClearDeferredSuspend`](crate::view::Event::ClearDeferredSuspend)
/// before this guard drops.
#[derive(Debug)]
#[must_use = "inhibitor guard is released immediately if unused"]
pub struct InhibitorGuard {
    kind: Kind,
    lease: Option<Lease>,
}

impl InhibitorGuard {
    pub(crate) fn soft_suspend(lease: Lease) -> Self {
        Self {
            kind: Kind::SoftSuspend,
            lease: Some(lease),
        }
    }

    pub(crate) fn full(lease: Lease) -> Self {
        Self {
            kind: Kind::Full,
            lease: Some(lease),
        }
    }

    /// Returns the inhibitor kind for this guard.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Returns whether this guard still holds an active lease.
    pub fn is_active(&self) -> bool {
        self.lease.is_some()
    }
}

impl Drop for InhibitorGuard {
    fn drop(&mut self) {
        self.lease.take();
    }
}
