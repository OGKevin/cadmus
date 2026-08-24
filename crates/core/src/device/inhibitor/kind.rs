//! Inhibitor lease kinds.
//!
//! Cadmus exposes exactly two kinds, modelled after systemd inhibitors but scoped
//! to e-reader constraints. [`Kind::SoftSuspend`] keeps the kernel awake during
//! background work; [`Kind::Full`] additionally blocks Cadmus suspend and
//! user-initiated exits until the last holder releases.

/// Kind of inhibitor lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Holds the kernel wake lock on Linux; does not block Cadmus suspend or exits.
    ///
    /// WiFi, main-loop events, library import, and similar background work use
    /// this kind so opportunistic autosleep does not sleep the device mid-task.
    SoftSuspend,
    /// Blocks Cadmus suspend and all user-initiated exits until released.
    ///
    /// Implies a nested [`Kind::SoftSuspend`] wake lock. OTA and other critical
    /// sections acquire this kind while work must not be interrupted.
    ///
    /// [`Inhibitor::acquire`](crate::device::inhibitor::Inhibitor::acquire) panics
    /// for this kind until Full inhibit is implemented.
    Full,
}
