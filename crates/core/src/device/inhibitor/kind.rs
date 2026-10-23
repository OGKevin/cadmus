//! Inhibitor lease kinds.
//!
//! Cadmus exposes three kinds, modelled after systemd inhibitors but scoped
//! to e-reader constraints. [`Kind::SoftSuspend`] keeps the kernel awake during
//! background work; [`Kind::Sleep`] also defers Auto Suspend; [`Kind::Full`]
//! additionally blocks all Cadmus suspend and user-initiated exits until the
//! last holder releases.
//!
//! Acquire all three through [`Inhibitor::acquire`](super::Inhibitor::acquire).
//! Full holder tracking and the nested `"full-inhibit"` wake lock live in the
//! `full` implementation module. Sleep tracking and `"sleep-inhibit"` live in
//! `sleep`.

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
    /// [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`](crate::device::inhibitor::FULL_INHIBIT_MIN_CAPACITY_PERCENT)
    /// or cannot be read.
    Full,
    /// Nested wake lock `"sleep-inhibit"` (paused during explicit suspend); defers Auto `start_cycle` only.
    ///
    /// Blocks opportunistic autosleep and Auto Suspend. Does not block user
    /// exits, explicit suspend (menu Suspend, power button, cover), or live
    /// AutoPowerOff. Explicit suspend pauses only the nested `"sleep-inhibit"`
    /// lease; other SoftSuspend holders stay held. No battery gate, no status
    /// LED. Last drop does not post
    /// [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared).
    Sleep,
}
