//! RAII guards for inhibitor leases.
//!
//! [`InhibitorGuard`] is returned from [`Inhibitor::acquire`].
//! Drop the guard to release the lease. On Linux [`Kind::SoftSuspend`],
//! the last guard dropping may start the configured release-grace timer before
//! `wake_unlock` is written.

use super::Kind;
use crate::lease::Lease;

/// RAII guard holding an inhibitor lease.
///
/// Drop the guard to release the lease. On Linux [`Kind::SoftSuspend`], the last
/// guard dropping may start the release-grace timer before `wake_unlock` is
/// written.
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
