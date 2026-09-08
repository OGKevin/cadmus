//! Hub channel payloads that can carry an optional soft-suspend lease.
//!
//! Producers attach a [`HubLease`] so the wake lock stays held while the message
//! sits in the hub queue. The main loop drops that lease after acquiring its own
//! `main-loop` lease, keeping coverage continuous across the hand-off.
//!
//! This path is [`Kind::SoftSuspend`] only. Critical work that must also block
//! explicit suspend uses [`Kind::Full`] on the worker (for example OTA), not a
//! hub-message lease.

use crate::device::inhibitor::{Inhibitor, InhibitorGuard, Kind, SoftSuspendName};
use crate::view::Event;
#[cfg(any(feature = "kobo", feature = "emulator", docsrs))]
use std::sync::mpsc::Sender;

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

    /// Wraps `event` with a soft-suspend lease when acquire succeeds; otherwise logs and
    /// returns a bare message.
    pub fn try_with_soft_suspend(
        inhibitor: &Inhibitor,
        name: SoftSuspendName,
        event: Event,
    ) -> Self {
        match inhibitor.acquire(Kind::SoftSuspend, name) {
            Ok(lease) => Self::with_soft_suspend(event, lease),
            Err(error) => {
                tracing::error!(
                    error = %error,
                    soft_suspend_lease = %name,
                    "failed to acquire soft-suspend lease for hub message"
                );
                event.into()
            }
        }
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

/// Enqueues `event` with the input handoff lease pattern when possible.
///
/// A short-lived overlap lease keeps the wake lock held until the message lease
/// is attached; on acquire failure the event is sent without a lease.
#[cfg(any(feature = "kobo", feature = "emulator", docsrs))]
pub(crate) fn send_input_hub_message(tx: &Sender<HubMessage>, inhibitor: &Inhibitor, event: Event) {
    let overlap = match inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Input) {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(
                error = %error,
                soft_suspend_lease = %SoftSuspendName::Input,
                "failed to acquire soft-suspend lease for input event"
            );
            tx.send(event.into()).ok();
            return;
        }
    };
    let message = HubMessage::try_with_soft_suspend(inhibitor, SoftSuspendName::Input, event);
    tx.send(message).ok();
    drop(overlap);
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
        let inhibitor = Inhibitor::with_paths(
            paths,
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        (dir, inhibitor)
    }

    #[test]
    fn soft_suspend_message_holds_lease_until_dropped() {
        let (_dir, inhibitor) = fixture();

        let _short = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Input)
            .unwrap();
        let message = HubMessage::with_soft_suspend(
            Event::ClockTick,
            inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::Input)
                .unwrap(),
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
            inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::Rtc)
                .unwrap(),
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
