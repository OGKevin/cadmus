//! Inhibitor lease kinds.
//!
//! Cadmus exposes exactly two kinds, modelled after systemd inhibitors but scoped
//! to e-reader constraints. [`Kind::SoftSuspend`] keeps the kernel awake during
//! background work; [`Kind::Full`] additionally blocks Cadmus suspend and
//! user-initiated exits until the last holder releases.
//!
//! Acquire both through [`Inhibitor::acquire`](super::Inhibitor::acquire). Full
//! holder tracking and the nested `"full-inhibit"` wake lock live in the `full`
//! implementation module.

/// Kind of inhibitor lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Holds the kernel wake lock on Linux; does not block Cadmus suspend or exits.
    ///
    /// WiFi, main-loop events, library import, and similar background work use
    /// this kind so opportunistic autosleep does not sleep the device mid-task.
    /// Explicit Auto Suspend / power-button sleep can still run.
    SoftSuspend,
    /// Blocks Cadmus suspend and all user-initiated exits until released.
    ///
    /// Implies a nested [`Kind::SoftSuspend`] wake lock named `"full-inhibit"`
    /// and a status-LED pulse ([`LedPriority::FullInhibit`](crate::device::leds::LedPriority::FullInhibit)).
    /// OTA acquires this kind as `"ota"`.
    ///
    /// On Kobo, explicit `start_cycle` defers while any holder is active, and
    /// user exits / live AutoPowerOff are ignored. Battery-monitor safety
    /// power-off is not blocked.
    ///
    /// Acquire fails with [`InhibitorError::BatteryTooLow`](crate::device::inhibitor::InhibitorError::BatteryTooLow)
    /// when shared battery capacity is below
    /// [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::FULL_INHIBIT_MIN_CAPACITY_PERCENT).
    Full,
}
