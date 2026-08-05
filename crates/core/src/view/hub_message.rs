//! Hub channel payloads that can carry an optional soft-suspend lease.
//!
//! Producers attach a [`HubLease`] so the wake lock stays held while the message
//! sits in the hub queue. The main loop drops that lease after acquiring its own
//! `main-loop` lease, keeping coverage continuous across the hand-off.

use crate::device::soft_suspend::SoftSuspendLease;
use crate::view::Event;

/// RAII lease attached to a [`HubMessage`] while it is in flight on the hub.
///
/// Dropping the enclosing message (or the lease extracted via
/// [`HubMessage::into_parts`]) releases the underlying resource.
pub enum HubLease {
    /// Soft-suspend wake lock for work that must keep autosleep from entering
    /// `mem` until the main loop takes over.
    SoftSuspend(SoftSuspendLease),
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

    /// Wraps `event` with a soft-suspend lease (see [`HubLease::SoftSuspend`]).
    pub fn with_soft_suspend(event: Event, lease: SoftSuspendLease) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::soft_suspend::{SoftSuspendPaths, SoftSuspendSession};
    use std::fs;
    use std::sync::Arc;

    fn session() -> (tempfile::TempDir, Arc<SoftSuspendSession>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = SoftSuspendPaths {
            state: dir.path().join("state"),
            autosleep: dir.path().join("autosleep"),
            wake_lock: dir.path().join("wake_lock"),
            wake_unlock: dir.path().join("wake_unlock"),
        };
        fs::write(&paths.state, "freeze mem\n").expect("state");
        fs::write(&paths.autosleep, "off\n").expect("autosleep");
        fs::write(&paths.wake_lock, "").expect("wake_lock");
        fs::write(&paths.wake_unlock, "").expect("wake_unlock");
        let session = SoftSuspendSession::with_paths(paths, None);
        (dir, session)
    }

    #[test]
    fn soft_suspend_message_holds_lease_until_dropped() {
        let (_dir, session) = session();

        let _short = session.acquire("input");
        let message = HubMessage::with_soft_suspend(Event::ClockTick, session.acquire("input"));

        assert!(!session.is_empty());
        drop(_short);
        assert!(!session.is_empty());
        drop(message);
        assert!(session.is_empty());
    }

    #[test]
    fn rtc_alarm_message_holds_lease_until_dropped() {
        let (_dir, session) = session();

        let message = HubMessage::with_soft_suspend(
            Event::RtcAlarmFired(crate::AlarmType::AutoSuspend),
            session.acquire("rtc"),
        );

        assert!(!session.is_empty());
        drop(message);
        assert!(session.is_empty());
    }

    #[test]
    fn bare_message_does_not_acquire_lease() {
        let (_dir, session) = session();

        let message = HubMessage::from(Event::ClockTick);

        assert!(session.is_empty());
        drop(message);
        assert!(session.is_empty());
    }
}
