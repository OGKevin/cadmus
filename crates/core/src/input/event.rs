use serde::Serialize;

use crate::geom::Point;

use super::codes::{VAL_PRESS, VAL_RELEASE, VAL_REPEAT};
use super::keyboard::KeyEvent;
use super::profile::DeviceProfile;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub time: libc::timeval,
    /// Event type, one of the `EV_*` constants (e.g. `EV_KEY`, `EV_ABS`).
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}
/// Identifies one opened evdev device, assigned when its reader is spawned.
///
/// Distinct from [`EventSource::Synthetic`] so state keyed by device (touch
/// pipeline, modifier tracking) cannot be driven by injected events.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(super) struct SourceId(pub(super) u32);

impl SourceId {
    /// First id handed out for a fresh device set.
    pub(super) const START: Self = Self(0);

    /// Returns the next id. Wraparound after four billion devices is
    /// irrelevant in practice and only risks aliasing with long-gone devices.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(super) fn next(&mut self) -> Self {
        let id = *self;
        self.0 = self.0.wrapping_add(1);
        id
    }
}
/// The origin of a [`TaggedInputEvent`].
///
/// Real devices are identified by the [`SourceId`] of the reader that read the
/// event; in-process injections (`display_rotate_event`,
/// `button_scheme_event`) carry [`EventSource::Synthetic`] instead of a magic
/// id so they can never be confused with a device.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(super) enum EventSource {
    Device(SourceId),
    Synthetic,
}
/// An [`InputEvent`] tagged with the [`DeviceProfile`] of the device that
/// produced it.
///
/// Persistent devices (touch digitizer, gpio-keys, power key) are discovered by
/// a feature scan at startup and kept open for the process lifetime. Hotplugged
/// devices (keyboards, remotes) are discovered by periodic rescans and may be
/// removed when they disappear. Both classes are classified into a
/// [`DeviceProfile`] by their kernel features so the device-event pipeline
/// routes their `EV_KEY` events through a uniform scancode table.
///
/// `source` additionally identifies *which* device produced the event, because
/// several devices share the pipeline and some state (touch reports, held
/// modifiers) must not leak between them.
#[derive(Debug, Clone)]
pub struct TaggedInputEvent {
    pub event: InputEvent,
    pub(super) profile: DeviceProfile,
    pub(super) source: EventSource,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FingerStatus {
    Down,
    Motion,
    Up,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ButtonStatus {
    Pressed,
    Released,
    Repeated,
}

impl ButtonStatus {
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub fn try_from_raw(value: i32) -> Option<ButtonStatus> {
        match value {
            VAL_RELEASE => Some(ButtonStatus::Released),
            VAL_PRESS => Some(ButtonStatus::Pressed),
            VAL_REPEAT => Some(ButtonStatus::Repeated),
            _ => None,
        }
    }
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ButtonCode {
    Power,
    Home,
    Light,
    Backward,
    Forward,
    Erase,
    Highlight,
    Raw(u16),
    /// Keyboard key event from a hotplugged or remote keyboard.
    ///
    /// Carries a Wayland-oriented [`KeyEvent`] so the same payload can be
    /// forwarded to a future Wayland server's `wl_keyboard` interface. The
    /// press/release/repeat state is carried by the enclosing
    /// [`DeviceEvent::Button`] `status` field.
    Keyboard(KeyEvent),
}
#[derive(Debug, Copy, Clone)]
pub enum DeviceEvent {
    Finger {
        id: i32,
        time: f64,
        status: FingerStatus,
        position: Point,
    },
    Button {
        time: f64,
        code: ButtonCode,
        status: ButtonStatus,
    },
    Plug(PowerSource),
    Unplug(PowerSource),
    /// Screen rotation request (`0`, `90`, `180`, or `270` degrees).
    ///
    /// On Kobo, handled by the lifecycle path to rotate the framebuffer. In the
    /// emulator lifecycle, the event is swallowed because rotation is
    /// unsupported, avoiding partial state.
    RotateScreen(i8),
    CoverOn,
    CoverOff,
    /// Network interface is up (DHCP complete).
    ///
    /// Emitted when WiFi monitoring detects a completed interface binding.
    ///
    /// Dispatch is two-phase:
    ///
    /// 1. **Lifecycle** — On Kobo, the lifecycle `handle_net_up` handler sets
    ///    `context.online`, shows a notification, and returns
    ///    [`EventOutcome::Continue`](crate::device::EventOutcome) when the
    ///    network was previously offline.
    /// 2. **Main loop** — The application main loop forwards the event to the
    ///    background [`Home`](crate::view::home::Home) view (history slot 0) when
    ///    another screen is active, so library fetchers can resume.
    NetUp,
    UserActivity,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PowerSource {
    Host,
    Wall,
}
#[cfg_attr(feature = "tracing", tracing::instrument(skip(time), level = tracing::Level::TRACE))]
pub(super) fn seconds(time: libc::timeval) -> f64 {
    time.tv_sec as f64 + time.tv_usec as f64 / 1e6
}
