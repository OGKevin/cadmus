//! RAII guard for a named soft-suspend lease.

use crate::lease::Lease;

/// RAII guard holding a soft-suspend lease.
#[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
pub struct SoftSuspendLease {
    inner: Option<Lease>,
}

impl SoftSuspendLease {
    /// Empty lease used by the no-op backend.
    pub(crate) fn noop() -> Self {
        Self { inner: None }
    }

    /// Wraps an active [`Lease`] from a live Linux session.
    #[cfg(any(target_os = "linux", docsrs))]
    pub(crate) fn from_lease(lease: Lease) -> Self {
        Self { inner: Some(lease) }
    }

    /// Returns whether this guard still holds the lease.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
}

impl Drop for SoftSuspendLease {
    fn drop(&mut self) {
        self.inner.take();
    }
}
