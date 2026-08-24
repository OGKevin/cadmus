//! Hub channel payloads that can carry an optional soft-suspend lease.
//!
//! Producers attach a [`HubLease`] so the wake lock stays held while the message
//! sits in the hub queue. The main loop drops that lease after acquiring its own
//! `main-loop` lease, keeping coverage continuous across the hand-off.

use crate::device::inhibitor::InhibitorGuard;
use crate::view::Event;

/// RAII lease attached to a [`HubMessage`] while it is in flight on the hub.
///
/// Dropping the enclosing message (or the lease extracted via
/// [`HubMessage::into_parts`]) releases the underlying resource.
pub enum HubLease {
    /// Soft-suspend wake lock for work that must keep autosleep from entering
    /// `mem` until the main loop takes over.
    SoftSuspend(InhibitorGuard),
}

/// Event delivered on the hub, optionally pinning a [`HubLease`] until handled.
///
/// Bare messages (`From<Event>`) carry no lease. Input and other producers that
/// must stay awake use [`Self::with_soft_suspend`] so the lease spans enqueue
/// until the receiver overlaps with the main-loop lease.
pub struct HubMessage {
    /// View / device event to dispatch.
    pub event: Event,
    /// Held until this message is dropped or dismantled with [`Self::into_parts`].
    _lease: Option<HubLease>,
}

impl HubMessage {
    /// Wraps `event` and keeps `lease` alive for as long as this message exists.
    pub fn with_lease(event: Event, lease: HubLease) -> Self {
        Self {
            event,
            _lease: Some(lease),
        }
    }

    /// Wraps `event` with a soft-suspend inhibitor guard (see [`HubLease::SoftSuspend`]).
    pub fn with_soft_suspend(event: Event, lease: InhibitorGuard) -> Self {
        Self::with_lease(event, HubLease::SoftSuspend(lease))
    }

    /// Splits into the event and any remaining lease without dropping the lease.
    pub fn into_parts(self) -> (Event, Option<HubLease>) {
        (self.event, self._lease)
    }
}

impl From<Event> for HubMessage {
    /// Builds a message with no attached lease.
    fn from(event: Event) -> Self {
        Self {
            event,
            _lease: None,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
    use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, Arc<Inhibitor>) {
        let (dir, paths) = SoftSuspendPaths::test_fixture();
        let inhibitor = Inhibitor::with_paths(paths, None);
        (dir, inhibitor)
    }

    #[test]
    fn soft_suspend_message_holds_lease_until_dropped() {
        let (_dir, inhibitor) = fixture();

        let _short = inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Input);
        let message = HubMessage::with_soft_suspend(
            Event::ClockTick,
            inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Input),
        );

        assert!(!inhibitor.is_empty());
        drop(_short);
        assert!(!inhibitor.is_empty());
        drop(message);
        assert!(inhibitor.is_empty());
    }

    #[test]
    fn rtc_alarm_message_holds_lease_until_dropped() {
        let (_dir, inhibitor) = fixture();

        let message = HubMessage::with_soft_suspend(
            Event::RtcAlarmFired(crate::AlarmType::AutoSuspend),
            inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Rtc),
        );

        assert!(!inhibitor.is_empty());
        drop(message);
        assert!(inhibitor.is_empty());
    }

    #[test]
    fn bare_message_does_not_acquire_lease() {
        let (_dir, inhibitor) = fixture();

        let message = HubMessage::from(Event::ClockTick);

        assert!(inhibitor.is_empty());
        drop(message);
        assert!(inhibitor.is_empty());
    }
}
