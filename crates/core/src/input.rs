use crate::framebuffer::Display;
use crate::geom::{LinearDir, Point};
use crate::settings::ButtonScheme;
use nix::{ioctl_read, ioctl_read_buf};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use std::fs::File;
use std::mem::{self, MaybeUninit};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::ptr;
use std::time::{Duration, Instant};
use tokio::io::Interest;
use tokio::io::unix::{AsyncFd, AsyncFdReadyGuard};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

// Event types
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;

// Event codes
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_PRESSURE: u16 = 0x3a;
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const MSC_RAW: u16 = 0x03;
pub const SYN_REPORT: u16 = 0x00;
pub const SYN_DROPPED: u16 = 0x03;

// Event values
pub const MSC_RAW_GSENSOR_PORTRAIT_DOWN: i32 = 0x17;
pub const MSC_RAW_GSENSOR_PORTRAIT_UP: i32 = 0x18;
pub const MSC_RAW_GSENSOR_LANDSCAPE_RIGHT: i32 = 0x19;
pub const MSC_RAW_GSENSOR_LANDSCAPE_LEFT: i32 = 0x1a;
// pub const MSC_RAW_GSENSOR_BACK: i32 = 0x1b;
// pub const MSC_RAW_GSENSOR_FRONT: i32 = 0x1c;

// The indices of this clockwise ordering of the sensor values match the Forma's rotation values.
pub const GYROSCOPE_ROTATIONS: [i32; 4] = [
    MSC_RAW_GSENSOR_LANDSCAPE_LEFT,
    MSC_RAW_GSENSOR_PORTRAIT_UP,
    MSC_RAW_GSENSOR_LANDSCAPE_RIGHT,
    MSC_RAW_GSENSOR_PORTRAIT_DOWN,
];

pub const VAL_RELEASE: i32 = 0;
pub const VAL_PRESS: i32 = 1;
pub const VAL_REPEAT: i32 = 2;

// Key codes
pub const KEY_POWER: u16 = 116;
pub const KEY_HOME: u16 = 102;
pub const KEY_LIGHT: u16 = 90;
pub const KEY_BACKWARD: u16 = 193;
pub const KEY_FORWARD: u16 = 194;
pub const KEY_SPACE: u16 = 57;
pub const KEY_UP: u16 = 103;
pub const KEY_DOWN: u16 = 108;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_NUMLOCK: u16 = 69;
pub const KEY_SCROLLLOCK: u16 = 70;
pub const PEN_ERASE: u16 = 331;
pub const PEN_HIGHLIGHT: u16 = 332;
pub const SLEEP_COVER: [u16; 2] = [59, 35];
// Synthetic touch button
pub const BTN_TOUCH: u16 = 330;
// The following key codes are fake, and are used to support
// software toggles within this design
pub const KEY_ROTATE_DISPLAY: u16 = 0xffff;
pub const KEY_BUTTON_SCHEME: u16 = 0xfffe;

pub const SINGLE_TOUCH_CODES: TouchCodes = TouchCodes {
    pressure: ABS_PRESSURE,
    x: ABS_X,
    y: ABS_Y,
};

pub const MULTI_TOUCH_CODES_A: TouchCodes = TouchCodes {
    pressure: ABS_MT_TOUCH_MAJOR,
    x: ABS_MT_POSITION_X,
    y: ABS_MT_POSITION_Y,
};

pub const MULTI_TOUCH_CODES_B: TouchCodes = TouchCodes {
    pressure: ABS_MT_PRESSURE,
    ..MULTI_TOUCH_CODES_A
};

#[repr(C)]
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub time: libc::timeval,
    pub kind: u16, // type
    pub code: u16,
    pub value: i32,
}

/// How a device's `EV_KEY` events map to semantic actions.
///
/// Each opened evdev device is classified into a profile by probing its kernel
/// features (name, bustype, event-type bits) — see [`classify_device`]. The
/// profile is a small scancode table ([`key_map`]); scancodes not in the table
/// are forwarded as [`ButtonCode::Keyboard`] when [`tracks_modifiers`] is
/// `true` (keyboard-like devices) or [`ButtonCode::Raw`] otherwise.
///
/// This replaces the older `DeviceOrigin` enum: instead of hard-coding special
/// functions to specific device names, each device's function map is data
/// selected by detected features. The Kobo Remote's arrow-key page-turn is
/// thus handled congruently with the built-in page-turn buttons — both are
/// entries in their respective profile's `key_map`, dispatched through the
/// same lookup in [`parse_device_events`].
///
/// [`key_map`]: DeviceProfile::key_map
/// [`tracks_modifiers`]: DeviceProfile::tracks_modifiers
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct DeviceProfile {
    key_map: &'static [(u16, KeyAction)],
    tracks_modifiers: bool,
}

impl DeviceProfile {
    /// Profile for built-in device hardware (touch digitizer, gpio-keys,
    /// power key): maps the standard built-in scancodes to their functions,
    /// including the synthetic control codes injected via
    /// [`display_rotate_event`] / [`button_scheme_event`]. Unknown scancodes
    /// become [`ButtonCode::Raw`].
    const fn built_in() -> Self {
        Self {
            key_map: BUILT_IN_KEY_MAP,
            tracks_modifiers: false,
        }
    }

    /// Profile for the Kobo Remote Bluetooth keyboard: up/down arrows map to
    /// page-turn buttons (bypassing the button-scheme, since the remote is
    /// orientation-agnostic); all other keys are forwarded as
    /// [`ButtonCode::Keyboard`] with modifier tracking.
    const fn remote() -> Self {
        Self {
            key_map: REMOTE_KEY_MAP,
            tracks_modifiers: true,
        }
    }

    /// Profile for a generic hotplugged keyboard: no special scancodes; all
    /// keys are forwarded as [`ButtonCode::Keyboard`] with modifier tracking.
    const fn keyboard() -> Self {
        Self {
            key_map: &[],
            tracks_modifiers: true,
        }
    }

    /// Returns the [`KeyAction`] mapped to `code`, if any.
    fn lookup(self, code: u16) -> Option<KeyAction> {
        self.key_map
            .iter()
            .find_map(|(c, action)| (*c == code).then_some(*action))
    }
}

/// A semantic action mapped to a specific evdev scancode within a
/// [`DeviceProfile`].
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum KeyAction {
    Power,
    Home,
    Light,
    /// Page-turn that respects the button-scheme (rotation-based inversion),
    /// used by built-in page-turn buttons.
    PageTurnScheme(LinearDir),
    /// Page-turn that bypasses the button-scheme, used by orientation-agnostic
    /// remotes.
    PageTurnRaw(LinearDir),
    Erase,
    Highlight,
    /// Magnetic sleep-cover open/close signal.
    SleepCover,
    /// Synthetic control: rotate the touch coordinate mapping.
    RotateDisplay,
    /// Synthetic control: toggle the button scheme.
    ButtonScheme,
}

impl KeyAction {
    /// Resolves a button-dispatch action to a [`ButtonCode`].
    ///
    /// [`PageTurnScheme`] respects the button-scheme (rotation-based
    /// inversion); [`PageTurnRaw`] bypasses it. Control actions
    /// ([`SleepCover`], [`ButtonScheme`], [`RotateDisplay`]) are handled by
    /// their own match arms in [`parse_device_events`] before this is called,
    /// so reaching them here is a logic error.
    ///
    /// [`PageTurnScheme`]: KeyAction::PageTurnScheme
    /// [`PageTurnRaw`]: KeyAction::PageTurnRaw
    /// [`SleepCover`]: KeyAction::SleepCover
    /// [`ButtonScheme`]: KeyAction::ButtonScheme
    /// [`RotateDisplay`]: KeyAction::RotateDisplay
    fn to_button_code(
        self,
        rotation: i8,
        button_scheme: ButtonScheme,
        startup_rotation: i8,
        dir: i8,
    ) -> ButtonCode {
        match self {
            KeyAction::Power => ButtonCode::Power,
            KeyAction::Home => ButtonCode::Home,
            KeyAction::Light => ButtonCode::Light,
            KeyAction::Erase => ButtonCode::Erase,
            KeyAction::Highlight => ButtonCode::Highlight,
            KeyAction::PageTurnScheme(direction) => {
                resolve_button_direction(direction, rotation, button_scheme, startup_rotation, dir)
            }
            KeyAction::PageTurnRaw(LinearDir::Forward) => ButtonCode::Forward,
            KeyAction::PageTurnRaw(LinearDir::Backward) => ButtonCode::Backward,
            KeyAction::SleepCover | KeyAction::ButtonScheme | KeyAction::RotateDisplay => {
                unreachable!("control actions are handled before to_button_code is called")
            }
        }
    }
}

/// Scancode table for [`DeviceProfile::built_in`].
const BUILT_IN_KEY_MAP: &[(u16, KeyAction)] = &[
    (KEY_POWER, KeyAction::Power),
    (KEY_HOME, KeyAction::Home),
    (KEY_LIGHT, KeyAction::Light),
    (KEY_BACKWARD, KeyAction::PageTurnScheme(LinearDir::Backward)),
    (KEY_FORWARD, KeyAction::PageTurnScheme(LinearDir::Forward)),
    (PEN_ERASE, KeyAction::Erase),
    (PEN_HIGHLIGHT, KeyAction::Highlight),
    (SLEEP_COVER[0], KeyAction::SleepCover),
    (SLEEP_COVER[1], KeyAction::SleepCover),
    (KEY_ROTATE_DISPLAY, KeyAction::RotateDisplay),
    (KEY_BUTTON_SCHEME, KeyAction::ButtonScheme),
];

/// Scancode table for [`DeviceProfile::remote`].
const REMOTE_KEY_MAP: &[(u16, KeyAction)] = &[
    (KEY_UP, KeyAction::PageTurnRaw(LinearDir::Backward)),
    (KEY_DOWN, KeyAction::PageTurnRaw(LinearDir::Forward)),
];

/// Identifies one opened evdev device, assigned when its reader is spawned.
///
/// Distinct from [`EventSource::Synthetic`] so state keyed by device (touch
/// pipeline, modifier tracking) cannot be driven by injected events.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct SourceId(u32);

impl SourceId {
    /// First id handed out for a fresh device set.
    const START: Self = Self(0);

    /// Returns the next id. Wraparound after four billion devices is
    /// irrelevant in practice and only risks aliasing with long-gone devices.
    fn next(&mut self) -> Self {
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
pub(crate) enum EventSource {
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
    pub(crate) profile: DeviceProfile,
    pub(crate) source: EventSource,
}

// Handle different touch protocols
#[derive(Debug)]
pub struct TouchCodes {
    pressure: u16,
    x: u16,
    y: u16,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TouchProto {
    Single,
    MultiA,
    MultiB, // Pressure won't indicate a finger release.
    MultiC,
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
    pub fn try_from_raw(value: i32) -> Option<ButtonStatus> {
        match value {
            VAL_RELEASE => Some(ButtonStatus::Released),
            VAL_PRESS => Some(ButtonStatus::Pressed),
            VAL_REPEAT => Some(ButtonStatus::Repeated),
            _ => None,
        }
    }
}

/// XKB real modifier indices, matching the bit positions used in Wayland's
/// `wl_keyboard.modifiers` event (`mods_depressed`, `mods_latched`,
/// `mods_locked`).
///
/// The discriminants correspond to the canonical XKB modifier order defined in
/// the XKB specification. Virtual modifiers (Alt, Super, NumLock, …) are
/// mapped to their conventional real-modifier bit by `from_evdev_code`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u32)]
pub enum Modifier {
    Shift = 0,
    Caps = 1,
    Ctrl = 2,
    Alt = 3,
    NumLock = 4,
    Mod3 = 5,
    Super = 6,
    AltGr = 7,
}

impl Modifier {
    /// Returns the bitmask for this modifier, suitable for the `mods_*`
    /// fields of a Wayland `wl_keyboard.modifiers` event.
    pub const fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// Maps an evdev keycode to the modifier it controls, if any.
    ///
    /// `KEY_LEFTALT` / `KEY_RIGHTALT` map to [`Modifier::Alt`]; on layouts
    /// where the right Alt acts as AltGr, a future keymap-aware path can
    /// remap it to [`Modifier::AltGr`].
    pub const fn from_evdev_code(code: u16) -> Option<Modifier> {
        match code {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => Some(Modifier::Shift),
            KEY_CAPSLOCK => Some(Modifier::Caps),
            KEY_LEFTCTRL | KEY_RIGHTCTRL => Some(Modifier::Ctrl),
            KEY_LEFTALT | KEY_RIGHTALT => Some(Modifier::Alt),
            KEY_NUMLOCK => Some(Modifier::NumLock),
            KEY_LEFTMETA | KEY_RIGHTMETA => Some(Modifier::Super),
            _ => None,
        }
    }
}

/// Wayland-compatible modifier state.
///
/// Mirrors the four `u32` fields of the `wl_keyboard.modifiers` event so the
/// same value can be forwarded directly to a future Wayland server:
///
/// - `mods_depressed` — physically held modifiers.
/// - `mods_latched` — sticky modifiers released after the next non-modifier key.
/// - `mods_locked` — toggled modifiers (Caps Lock, Num Lock).
/// - `group` — active keyboard layout index.
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Modifiers {
    pub mods_depressed: u32,
    pub mods_latched: u32,
    pub mods_locked: u32,
    pub group: u32,
}

impl Modifiers {
    /// Effective modifier mask: `depressed | latched | locked`.
    ///
    /// Matches the `XKB_STATE_MODS_EFFECTIVE` semantics used by xkbcommon.
    pub const fn effective(self) -> u32 {
        self.mods_depressed | self.mods_latched | self.mods_locked
    }

    /// Returns `true` if `modifier` is active in the effective mask.
    pub const fn has(self, modifier: Modifier) -> bool {
        self.effective() & modifier.bit() != 0
    }

    /// Returns `true` if `modifier` is physically held (depressed).
    pub const fn is_depressed(self, modifier: Modifier) -> bool {
        self.mods_depressed & modifier.bit() != 0
    }

    /// Sets or clears the depressed state of `modifier`.
    pub fn set_depressed(&mut self, modifier: Modifier, active: bool) {
        if active {
            self.mods_depressed |= modifier.bit();
        } else {
            self.mods_depressed &= !modifier.bit();
        }
    }

    /// Returns `true` if Shift is active in the effective mask.
    pub const fn shift(self) -> bool {
        self.has(Modifier::Shift)
    }

    /// Returns `true` if Control is active in the effective mask.
    pub const fn ctrl(self) -> bool {
        self.has(Modifier::Ctrl)
    }

    /// Returns `true` if Alt is active in the effective mask.
    pub const fn alt(self) -> bool {
        self.has(Modifier::Alt)
    }

    /// Returns `true` if Super (Meta/Windows) is active in the effective mask.
    pub const fn super_(self) -> bool {
        self.has(Modifier::Super)
    }

    /// Returns `true` if Caps Lock is active in the effective mask.
    pub const fn caps(self) -> bool {
        self.has(Modifier::Caps)
    }

    /// Returns `true` if Num Lock is active in the effective mask.
    pub const fn num_lock(self) -> bool {
        self.has(Modifier::NumLock)
    }

    /// Returns `true` if AltGr (Level Three) is active in the effective mask.
    pub const fn alt_gr(self) -> bool {
        self.has(Modifier::AltGr)
    }
}

/// Wayland-compatible key state, matching the `wl_keyboard.key` event's
/// `state` argument.
///
/// Wayland protocol versions < 10 only define `released` and `pressed`;
/// version 10 added `repeated` for compositor-driven key repeat.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyState {
    Released,
    Pressed,
    Repeated,
}

impl KeyState {
    /// Maps an evdev `EV_KEY` value to a [`KeyState`].
    pub fn from_evdev_value(value: i32) -> Option<Self> {
        match value {
            VAL_RELEASE => Some(KeyState::Released),
            VAL_PRESS => Some(KeyState::Pressed),
            VAL_REPEAT => Some(KeyState::Repeated),
            _ => None,
        }
    }
}

/// Wayland-oriented key event payload.
///
/// Carries the evdev scancode and a snapshot of the modifier state at the
/// moment the key was pressed or released. The scancode is the raw evdev
/// keycode (as used by the Wayland protocol); XKB keycodes are `keycode + 8`.
///
/// The `ButtonStatus` carried alongside this in [`ButtonCode::Keyboard`]
/// provides the press/release/repeat state, and the `time` field of
/// [`DeviceEvent::Button`] provides the timestamp — matching the split
/// between `wl_keyboard.key` (key + state + time) and
/// `wl_keyboard.modifiers` (modifier mask) in the Wayland protocol.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEvent {
    /// Evdev scancode (e.g. `KEY_SPACE` = 57). Wayland uses evdev scancodes
    /// directly; XKB keycodes are `keycode + 8`.
    pub keycode: u32,
    /// Modifier state snapshot at the time of the event.
    pub modifiers: Modifiers,
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

fn resolve_button_direction(
    mut direction: LinearDir,
    rotation: i8,
    button_scheme: ButtonScheme,
    startup_rotation: i8,
    dir: i8,
) -> ButtonCode {
    let should_invert = rotation == (4 + startup_rotation - dir) % 4
        || rotation == (4 + startup_rotation - 2 * dir) % 4;
    if should_invert ^ (button_scheme == ButtonScheme::Inverted) {
        direction = direction.opposite();
    }

    if direction == LinearDir::Forward {
        return ButtonCode::Forward;
    }

    ButtonCode::Backward
}

pub fn display_rotate_event(n: i8) -> TaggedInputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    TaggedInputEvent {
        event: InputEvent {
            time: tp,
            kind: EV_KEY,
            code: KEY_ROTATE_DISPLAY,
            value: n as i32,
        },
        profile: DeviceProfile::built_in(),
        source: EventSource::Synthetic,
    }
}

pub fn button_scheme_event(v: i32) -> TaggedInputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    TaggedInputEvent {
        event: InputEvent {
            time: tp,
            kind: EV_KEY,
            code: KEY_BUTTON_SCHEME,
            value: v,
        },
        profile: DeviceProfile::built_in(),
        source: EventSource::Synthetic,
    }
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

pub fn seconds(time: libc::timeval) -> f64 {
    time.tv_sec as f64 + time.tv_usec as f64 / 1e6
}

pub fn raw_events(
    display: Display,
    proto: TouchProto,
    cancel: CancellationToken,
) -> (
    UnboundedSender<TaggedInputEvent>,
    UnboundedReceiver<TaggedInputEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_reader = tx.clone();
    crate::runtime::current_handle().spawn(async move {
        parse_raw_events(&cancel, display, proto, &tx_reader).await;
    });
    (tx, rx)
}

/// How long an opened device may stay silent before it is first reported.
const SILENCE_REPORT_AFTER: Duration = Duration::from_secs(60);

/// How often a still-silent device repeats its report after the first one.
const SILENCE_REPEAT_EVERY: Duration = Duration::from_secs(600);

/// Reports a device that has forwarded nothing since it was opened.
///
/// Only devices that have forwarded nothing at all are reported, so an
/// idle-but-working device cannot trip it. Reports are trace-level, so the
/// watch is only constructed when trace logging is enabled.
///
/// Timing comes from [`Instant`] (`CLOCK_MONOTONIC`), which does not advance
/// while the system is suspended, and `tokio::time::sleep` shares that clock,
/// so sleep and soft suspend cannot inflate the measured silence. The first
/// window is [`SILENCE_REPORT_AFTER`]; later ones repeat every
/// [`SILENCE_REPEAT_EVERY`].
struct SilenceWatch<'a> {
    path: &'a str,
    profile: DeviceProfile,
    opened_at: Instant,
    reported: bool,
}

impl<'a> SilenceWatch<'a> {
    fn new(path: &'a str, profile: DeviceProfile, opened_at: Instant) -> Self {
        Self {
            path,
            profile,
            opened_at,
            reported: false,
        }
    }

    /// Waits one silence window, then logs the absence of events.
    async fn next_report(&mut self, events_read: u64) {
        let window = if self.reported {
            SILENCE_REPEAT_EVERY
        } else {
            SILENCE_REPORT_AFTER
        };
        tokio::time::sleep(window).await;

        let open_ms = self.opened_at.elapsed().as_millis() as u64;
        tracing::trace!(
            device = self.path,
            profile = ?self.profile,
            events_read,
            open_ms,
            "input device produced no events since open"
        );
        self.reported = true;
    }
}

/// Waits for `fd` to become readable.
///
/// When `silence` holds a watch, the wait races that device's silence timer and
/// returns `None` whenever it elapses, so the caller loops and keeps waiting.
async fn wait_readable<'a>(
    fd: &'a AsyncFd<File>,
    silence: &mut Option<SilenceWatch<'_>>,
    events_read: u64,
) -> std::io::Result<Option<AsyncFdReadyGuard<'a, File>>> {
    match silence {
        None => fd.readable().await.map(Some),
        Some(watch) => tokio::select! {
            guard = fd.readable() => guard.map(Some),
            () = watch.next_report(events_read) => Ok(None),
        },
    }
}

/// Forwards evdev events from one nonblocking device until it closes, errors,
/// or its consumers vanish.
///
/// Each forwarded event is tagged with `profile`, the scancode classification of
/// the device it was read from, and `source`, the identity of this reader's
/// device among all open devices.
///
/// `AsyncFd` only says the fd is readable. A single `read` may still return
/// fewer bytes than `size_of::<InputEvent>()`. Treating
/// that short read as EOF would drop the device for the rest of the session.
/// `pending` / `filled` keep those bytes across `readable()` waits until one
/// full record is available. A zero-length read is the only EOF.
///
/// While the device has forwarded nothing yet, its readiness wait races a
/// [`SilenceWatch`] so "opened but never delivered a byte" is observable.
async fn read_input_events(
    fd: AsyncFd<File>,
    path: &str,
    profile: DeviceProfile,
    source: SourceId,
    tx: &UnboundedSender<TaggedInputEvent>,
) -> ReaderExit {
    let opened_at = Instant::now();
    let mut pending = [0_u8; mem::size_of::<InputEvent>()];
    let mut filled = 0usize;
    let mut events_read = 0u64;
    let mut silence = if tracing::enabled!(tracing::Level::TRACE) {
        Some(SilenceWatch::new(path, profile, opened_at))
    } else {
        None
    };

    loop {
        let mut guard = loop {
            match wait_readable(&fd, &mut silence, events_read).await {
                Ok(Some(guard)) => break guard,
                Ok(None) => continue,
                Err(error) => {
                    return reader_exit(ExitReason::Io(error), events_read, opened_at);
                }
            }
        };
        loop {
            match guard.try_io(|inner| read_input_event(inner.get_ref(), &mut pending, &mut filled))
            {
                Ok(Ok(event)) => {
                    if tx
                        .send(TaggedInputEvent {
                            event,
                            profile,
                            source: EventSource::Device(source),
                        })
                        .is_err()
                    {
                        return reader_exit(ExitReason::ChannelClosed, events_read, opened_at);
                    }
                    events_read += 1;
                    if events_read == 1 {
                        silence = None;
                        tracing::debug!(
                            device = path,
                            first_event_after_ms = opened_at.elapsed().as_millis() as u64,
                            "input device produced its first event"
                        );
                    }
                }
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return reader_exit(ExitReason::Eof, events_read, opened_at);
                }
                Ok(Err(err)) => return reader_exit(ExitReason::Io(err), events_read, opened_at),
                Err(_would_block) => break,
            }
        }
    }
}

/// Why a device reader task stopped forwarding events.
///
/// Logged with the reader's totals: before this existed, the channel-closed and
/// end-of-file paths returned silently, so a dead touch digitizer was
/// indistinguishable in a log from a device that never sent anything.
#[derive(Debug)]
enum ExitReason {
    /// The device node reported end-of-file.
    Eof,
    /// Every consumer of the tagged-event stream is gone.
    ChannelClosed,
    /// A read or readiness error other than end-of-file.
    Io(std::io::Error),
}

impl std::fmt::Display for ExitReason {
    /// Renders as a stable label, with the inner error text appended for the
    /// I/O variant so the log carries the kernel message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => f.write_str("eof"),
            Self::ChannelClosed => f.write_str("channel_closed"),
            Self::Io(error) => write!(f, "io_error: {error}"),
        }
    }
}

/// Totals a reader reports when it stops forwarding events.
#[derive(Debug)]
struct ReaderExit {
    reason: ExitReason,
    events_read: u64,
    open_ms: u64,
}

fn reader_exit(reason: ExitReason, events_read: u64, opened_at: Instant) -> ReaderExit {
    ReaderExit {
        reason,
        events_read,
        open_ms: opened_at.elapsed().as_millis() as u64,
    }
}

/// Reads one `InputEvent`, keeping a short read in `pending` until the next
/// readiness wake. A zero-length read is EOF. `WouldBlock` leaves `filled` as-is.
///
/// `InputEvent` is a C layout with no safe constructor from raw bytes, so this
/// builds one with `MaybeUninit`. The copy is sound because `pending` only
/// reaches `assume_init` once the read has filled all
/// `size_of::<InputEvent>()` bytes, so every byte is written first.
fn read_input_event(
    file: &File,
    pending: &mut [u8; mem::size_of::<InputEvent>()],
    filled: &mut usize,
) -> std::io::Result<InputEvent> {
    debug_assert!(*filled < pending.len(), "pending record is never left full");
    let n = read_fd(file.as_raw_fd(), &mut pending[*filled..])?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "evdev closed",
        ));
    }
    *filled += n;
    if *filled < pending.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "partial evdev event",
        ));
    }
    let bytes = *pending;
    *filled = 0;
    let mut input_event = MaybeUninit::<InputEvent>::uninit();
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            input_event.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Ok(input_event.assume_init())
    }
}

pub fn usb_events() -> UnboundedReceiver<DeviceEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::current_handle().spawn(async move {
        read_usb_events(&tx).await;
    });
    rx
}

/// Per-read size for `/tmp/nickel-hardware-status` (matches typical FIFO chunking).
const USB_STATUS_READ_BYTES: usize = 256;
/// Cap for an incomplete line held across reads: two read buffers, enough for any
/// real nickel status string split across wakes (messages are short ASCII tokens).
const USB_STATUS_PENDING_MAX: usize = USB_STATUS_READ_BYTES * 2;

async fn read_usb_events(tx: &UnboundedSender<DeviceEvent>) {
    // `OpenOptions` + `custom_flags` rather than a raw `libc::open` +
    // `OwnedFd::from_raw_fd`: the owned `File` closes the fd on drop, so there is
    // no unsafe here and no leak if the async setup below fails.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/tmp/nickel-hardware-status")
    {
        Ok(file) => file,
        Err(error) => {
            tracing::debug!(error = %error, "usb status fifo unavailable");
            return;
        }
    };
    let owned = OwnedFd::from(file);
    let async_fd = match AsyncFd::with_interest(owned, Interest::READABLE) {
        Ok(async_fd) => async_fd,
        Err(err) => {
            tracing::warn!(error = %err, "usb status reader failed");
            return;
        }
    };

    let mut pending = Vec::new();
    loop {
        let mut guard = match async_fd.readable().await {
            Ok(guard) => guard,
            Err(err) => {
                tracing::warn!(error = %err, "usb status wait failed");
                return;
            }
        };
        loop {
            match guard
                .try_io(|inner| read_usb_status(inner.get_ref().as_raw_fd(), &mut pending, tx))
            {
                Ok(Ok(0)) => return,
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "usb status read failed");
                    return;
                }
                Err(_would_block) => break,
            }
        }
    }
}

fn read_usb_status(
    fd: RawFd,
    pending: &mut Vec<u8>,
    tx: &UnboundedSender<DeviceEvent>,
) -> std::io::Result<usize> {
    let mut buf = [0u8; USB_STATUS_READ_BYTES];
    let n = read_fd(fd, &mut buf)?;
    if n == 0 {
        return Ok(0);
    }
    let end = buf[..n].iter().position(|byte| *byte == 0).unwrap_or(n);
    feed_usb_status_chunk(pending, &buf[..end], tx);
    Ok(n)
}

/// Appends `chunk` to `pending`, emits complete newline-delimited lines, and
/// retains any trailing fragment for the next read.
fn feed_usb_status_chunk(pending: &mut Vec<u8>, chunk: &[u8], tx: &UnboundedSender<DeviceEvent>) {
    pending.extend_from_slice(chunk);
    if pending.len() > USB_STATUS_PENDING_MAX {
        tracing::warn!(
            pending_len = pending.len(),
            max = USB_STATUS_PENDING_MAX,
            "usb status pending buffer overflow; discarding partial line"
        );
        pending.clear();
        return;
    }

    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let line = pending.drain(..=newline).collect::<Vec<u8>>();
        let line = &line[..line.len() - 1];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        match std::str::from_utf8(line) {
            Ok(message) => send_usb_hardware_message(tx, message),
            Err(_) => tracing::warn!(bytes = ?line, "usb status line is not valid utf-8"),
        }
    }
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        // SAFETY: `read` is called with a valid fd from `AsyncFd` and a buffer
        // slice that outlives this call; partial reads and `EINTR` are handled.
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len() as libc::size_t,
            )
        };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

fn send_usb_hardware_message(tx: &UnboundedSender<DeviceEvent>, message: &str) {
    let event = match message {
        "usb plug add" => DeviceEvent::Plug(PowerSource::Host),
        "usb plug remove" => DeviceEvent::Unplug(PowerSource::Host),
        "usb ac add" => DeviceEvent::Plug(PowerSource::Wall),
        "usb ac remove" => DeviceEvent::Unplug(PowerSource::Wall),
        _ => return,
    };
    tx.send(event).ok();
}

fn compute_mirror_axes(rotation: i8, mirroring_scheme: (i8, i8)) -> (bool, bool) {
    let (mxy, dir) = mirroring_scheme;
    let mx = (4 + (mxy + dir)) % 4;
    let my = (4 + (mxy - dir)) % 4;
    let mirror_x = mxy == rotation || mx == rotation;
    let mirror_y = mxy == rotation || my == rotation;
    (mirror_x, mirror_y)
}

/// Captures the subset of device properties needed for input event processing.
#[derive(Clone, Copy)]
pub struct DeviceInputInfo {
    pub proto: TouchProto,
    pub mark: u8,
    pub mirroring_scheme: (i8, i8),
    pub swapping_scheme: i8,
    pub startup_rotation: i8,
    pub gyro_rotation_transform: GyroRotationTransform,
}

#[derive(Clone, Copy)]
pub struct GyroRotationTransform(fn(i8) -> i8);

impl GyroRotationTransform {
    pub fn new(f: fn(i8) -> i8) -> Self {
        Self(f)
    }

    pub fn transform(&self, n: i8) -> i8 {
        (self.0)(n)
    }
}

impl Default for GyroRotationTransform {
    fn default() -> Self {
        Self(|n| n)
    }
}

pub fn device_events(
    mut rx: UnboundedReceiver<TaggedInputEvent>,
    display: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) -> UnboundedReceiver<DeviceEvent> {
    let Display { dims, rotation } = display;
    tracing::trace!(
        rotation,
        screen_dims = ?dims,
        proto = ?info.proto,
        mark = info.mark,
        mirroring_scheme = ?info.mirroring_scheme,
        swapping_scheme = info.swapping_scheme,
        startup_rotation = info.startup_rotation,
        "starting device event pipeline"
    );
    let (ty, ry) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::current_handle().spawn(async move {
        parse_device_events(
            &mut rx,
            &ty,
            Display { dims, rotation },
            button_scheme,
            info,
        )
        .await;
    });
    ry
}

#[repr(C)]
#[derive(Debug)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// If the ioctl fails, `None` is returned; `absinfo` must be a pointer to
/// an uninitialized [`InputAbsInfo`] that the kernel fills in on success.
async fn evdev_abs_info(path: &str, axis: u16) -> Option<InputAbsInfo> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let fd = file.as_raw_fd();
    let request = evdev_abs_ioctl(axis);
    // The ioctl writes into `absinfo` and returns 0 on success; on failure
    // (-1) the buffer is left untouched.
    let ret = tokio::task::spawn_blocking(move || unsafe {
        let mut absinfo = MaybeUninit::<InputAbsInfo>::uninit();
        let ret = libc::ioctl(fd, request as libc::c_ulong, absinfo.as_mut_ptr());
        (ret, absinfo)
    })
    .await
    .ok()?;
    let (ret, absinfo) = ret;
    if ret < 0 {
        return None;
    }
    Some(unsafe { absinfo.assume_init() })
}

const fn evdev_abs_ioctl(axis: u16) -> u32 {
    const IOC_READ: u32 = 2;
    const IOC_TYPE_E: u32 = b'E' as u32;
    const ABSINFO_SIZE: u32 = mem::size_of::<InputAbsInfo>() as u32;
    (IOC_READ << 30) | (ABSINFO_SIZE << 16) | (IOC_TYPE_E << 8) | (0x40 + axis as u32)
}

pub fn gsensor_rotation_from_raw(value: i32, transform: GyroRotationTransform) -> Option<i8> {
    if !(MSC_RAW_GSENSOR_PORTRAIT_DOWN..=MSC_RAW_GSENSOR_LANDSCAPE_LEFT).contains(&value) {
        return None;
    }
    GYROSCOPE_ROTATIONS
        .iter()
        .position(|&v| v == value)
        .map(|i| transform.transform(i as i8))
}

/// `min..max` reported by `EVIOCGABS` for one axis of one device.
type AbsRange = (i32, i32);

/// Per-axis `EVIOCGABS` ranges of one device: `x` then `y`.
type TouchAxes = (Option<AbsRange>, Option<AbsRange>);

pub async fn trace_touch_device_geometry(
    path: &str,
    proto: TouchProto,
    screen: Display,
) -> TouchAxes {
    let Display { dims, rotation } = screen;
    let (x_axis, y_axis) = match proto {
        TouchProto::Single => (ABS_X, ABS_Y),
        TouchProto::MultiA | TouchProto::MultiB | TouchProto::MultiC => {
            (ABS_MT_POSITION_X, ABS_MT_POSITION_Y)
        }
    };
    let abs_x = evdev_abs_info(path, x_axis).await;
    let abs_y = evdev_abs_info(path, y_axis).await;
    tracing::trace!(
        path,
        proto = ?proto,
        screen_rotation = rotation,
        screen_dims = ?dims,
        abs_x = ?abs_x,
        abs_y = ?abs_y,
        "evdev vs display geometry"
    );
    let range = |info: Option<InputAbsInfo>| info.map(|info| (info.minimum, info.maximum));
    (range(abs_x), range(abs_y))
}

#[derive(Debug)]
struct TouchState {
    position: Point,
    pressure: i32,
}

impl Default for TouchState {
    fn default() -> Self {
        TouchState {
            position: Point::default(),
            pressure: 0,
        }
    }
}

/// Touch report state for a single digitizer.
///
/// Kept per device: several evdev devices share the event pipeline and each
/// ends its reports with `EV_SYN` / `SYN_REPORT`, so a shared pipeline would let
/// an unrelated device's report lift in-contact fingers mid-gesture.
#[derive(Debug, Default)]
struct TouchPipeline {
    /// Tracking id of the slot currently being filled, or `0` for single-touch.
    id: i32,
    /// Partial reports keyed by tracking id, cleared per `SYN_REPORT` for
    /// multi-touch protocols.
    packets: FxHashMap<i32, TouchState>,
    /// Fingers currently in contact, keyed by tracking id.
    fingers: FxHashMap<i32, Point>,
}

/// Returns the touch pipeline for `source`, creating it on first sight.
///
/// Synthetic events never reach here, so an injected control event can neither
/// read nor reset touch state. Entries for devices whose readers exited are
/// left in place; their ids are never reused, so they are inert.
fn touch_pipeline(
    touch: &mut FxHashMap<SourceId, TouchPipeline>,
    source: SourceId,
    proto: TouchProto,
) -> &mut TouchPipeline {
    let pipeline = touch.entry(source).or_default();
    // Single-touch has no tracking ids, so one implicit slot is pre-created.
    if proto == TouchProto::Single && pipeline.packets.is_empty() {
        pipeline.packets.insert(pipeline.id, TouchState::default());
    }
    pipeline
}

/// How often the decoder heartbeat reports its event counters.
///
/// The heartbeat exists so a log can prove the decoder is alive and counting
/// even when no device event reaches it — the #1138 freeze showed the app loop
/// healthy while the digitizer stayed silent, and without this line that state
/// is indistinguishable from a dead decoder.
const DECODER_HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

/// How long a device may go without a report before [`DeviceEvent::UserActivity`]
/// is emitted again.
const ACTIVITY_REPORT_INTERVAL_SECS: libc::time_t = 60;

/// Returns the decoder heartbeat timer.
///
/// A persistent interval rather than a fresh `sleep` per event: the timer keeps
/// ticking while events stream, so the heartbeat still fires. `Interval::tick`
/// is cancel-safe, so polling it inside `select!` against `rx.recv()` neither
/// loses nor duplicates ticks. The first tick resolves immediately, which emits
/// a zero baseline line when the decoder starts.
fn heartbeat_interval() -> tokio::time::Interval {
    let mut heartbeat = tokio::time::interval(DECODER_HEARTBEAT_EVERY);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat
}

/// Emits [`DeviceEvent::UserActivity`] when the device clock passed the
/// activity interval since the last report, and records the new reference time.
///
/// The difference is taken as an absolute value because `tv_sec` wraps on
/// kernels whose `time_t` cannot grow forever, which would otherwise suppress
/// the heartbeat for the rest of the session.
fn report_user_activity(
    last_activity: &mut libc::time_t,
    time: libc::timeval,
    ty: &UnboundedSender<DeviceEvent>,
) {
    if (time.tv_sec - *last_activity).abs() < ACTIVITY_REPORT_INTERVAL_SECS {
        return;
    }
    *last_activity = time.tv_sec;
    ty.send(DeviceEvent::UserActivity).ok();
}

/// Event counters for [`parse_device_events`], emitted on the heartbeat tick.
///
/// Read them as a funnel: a device whose reader forwards bytes shows up in
/// `abs`/`key`, bytes the pipeline cannot place show up in `unmapped_abs`
/// (rotated `tc` table, unexpected pressure axis, report without tracking id),
/// and complete reports show up in `syn`/`touch_packets`. `dropped_syn` counts
/// kernel-side event loss.
#[derive(Debug, Default)]
struct DecoderCounters {
    abs: u64,
    key: u64,
    syn: u64,
    unmapped_abs: u64,
    touch_packets: u64,
    dropped_syn: u64,
}

/// Returns `true` when an `EV_ABS` code cannot be consumed by the touch
/// pipeline, i.e. it matches neither the tracking id nor any axis of the
/// active `tc` table. Such events indicate a profile/protocol mismatch — for
/// example a rotated or wrong-proto `tc` table — rather than lost data.
fn is_unmapped_abs_code(code: u16, tc: &TouchCodes) -> bool {
    code != ABS_MT_TRACKING_ID && code != tc.x && code != tc.y && code != tc.pressure
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(rx, ty, screen, button_scheme, info),
        level = tracing::Level::TRACE,
        fields(
            rotation = screen.rotation,
            dims = ?screen.dims,
            proto = ?info.proto,
            mark = info.mark,
            mirroring_scheme = ?info.mirroring_scheme,
            swapping_scheme = info.swapping_scheme,
            startup_rotation = info.startup_rotation,
        )
    )
)]
async fn parse_device_events(
    rx: &mut UnboundedReceiver<TaggedInputEvent>,
    ty: &UnboundedSender<DeviceEvent>,
    screen: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) {
    let DeviceInputInfo {
        proto,
        mark,
        mirroring_scheme,
        swapping_scheme,
        startup_rotation,
        gyro_rotation_transform,
    } = info;
    let (_, dir) = mirroring_scheme;
    let mut last_activity = -60;
    let Display {
        mut dims,
        mut rotation,
    } = screen;

    // Touch report state, scoped per digitizer: several devices share this
    // pipeline and every evdev device ends each report with EV_SYN /
    // SYN_REPORT, so shared state would let an unrelated button press lift
    // in-contact fingers mid-gesture. Scoping by device also makes a digitizer
    // replug (new source id, fresh state) work without special cases.
    let mut touch: FxHashMap<SourceId, TouchPipeline> = FxHashMap::default();

    let mut tc = match proto {
        TouchProto::Single => SINGLE_TOUCH_CODES,
        TouchProto::MultiA => MULTI_TOUCH_CODES_A,
        TouchProto::MultiB => MULTI_TOUCH_CODES_B,
        TouchProto::MultiC => MULTI_TOUCH_CODES_B,
    };

    let (mut mirror_x, mut mirror_y) = compute_mirror_axes(rotation, mirroring_scheme);
    if rotation % 2 == swapping_scheme {
        mem::swap(&mut tc.x, &mut tc.y);
    }

    let axes_swapped = rotation % 2 == swapping_scheme;
    tracing::trace!(
        rotation,
        dims = ?dims,
        mirror_x,
        mirror_y,
        mirroring_scheme = ?mirroring_scheme,
        swapping_scheme,
        startup_rotation,
        proto = ?proto,
        mark,
        tc_x = tc.x,
        tc_y = tc.y,
        axes_swapped,
        "parse_device_events started"
    );

    let mut button_scheme = button_scheme;

    // Modifier state for keyboards, tracked per source as a Wayland-compatible
    // `Modifiers` value so it can be forwarded directly to a `wl_keyboard`
    // interface in a future Wayland server. Per source because a keyboard that
    // disconnects with a modifier held never sends the release, and that stale
    // bit must not latch into later events from other keyboards.
    let mut modifiers: FxHashMap<SourceId, Modifiers> = FxHashMap::default();

    let mut counters = DecoderCounters::default();
    let mut heartbeat = heartbeat_interval();

    loop {
        tokio::select! {
            tagged = rx.recv() => {
                let Some(tagged) = tagged else { break };
                let evt = tagged.event;
                #[cfg(feature = "tracing")]
                let _span = tracing::trace_span!("processing input event", event = ?evt).entered();

        // `None` for synthetic events: they are EV_KEY controls and must not
        // touch per-device state.
        let device = match tagged.source {
            EventSource::Device(source) => Some(source),
            EventSource::Synthetic => None,
        };

        if evt.kind == EV_ABS {
            counters.abs += 1;
            if is_unmapped_abs_code(evt.code, &tc) {
                counters.unmapped_abs += 1;
            }
            let Some(source) = device else {
                continue;
            };
            let pipeline = touch_pipeline(&mut touch, source, proto);
            if evt.code == ABS_MT_TRACKING_ID {
                if evt.value >= 0 {
                    pipeline.id = evt.value;
                    pipeline.packets.insert(pipeline.id, TouchState::default());
                }
            } else if evt.code == tc.x {
                if let Some(state) = pipeline.packets.get_mut(&pipeline.id) {
                    state.position.x = if mirror_x {
                        dims.0 as i32 - 1 - evt.value
                    } else {
                        evt.value
                    };
                    tracing::trace!(
                        raw = evt.value,
                        evt_code = evt.code,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axis = "screen_x",
                        mirrored = mirror_x,
                        dim_used = dims.0,
                        result = state.position.x,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        "touch axis raw"
                    );
                }
            } else if evt.code == tc.y {
                if let Some(state) = pipeline.packets.get_mut(&pipeline.id) {
                    state.position.y = if mirror_y {
                        dims.1 as i32 - 1 - evt.value
                    } else {
                        evt.value
                    };
                    tracing::trace!(
                        raw = evt.value,
                        evt_code = evt.code,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axis = "screen_y",
                        mirrored = mirror_y,
                        dim_used = dims.1,
                        result = state.position.y,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        "touch axis raw"
                    );
                }
            } else if evt.code == tc.pressure
                && let Some(state) = pipeline.packets.get_mut(&pipeline.id)
            {
                state.pressure = evt.value;
                if proto == TouchProto::Single && mark == 3 && state.pressure == 0 {
                    state.position.x = dims.0 as i32 - 1 - state.position.x;
                    mem::swap(&mut state.position.x, &mut state.position.y);
                }
            }
        } else if evt.kind == EV_SYN {
            if evt.code == SYN_REPORT {
                counters.syn += 1;
                report_user_activity(&mut last_activity, evt.time, ty);

                let Some(source) = device else {
                    continue;
                };
                counters.touch_packets += 1;
                let pipeline = touch_pipeline(&mut touch, source, proto);
            if proto == TouchProto::MultiB {
                pipeline.fingers.retain(|other_id, other_position| {
                    pipeline.packets.contains_key(other_id)
                        || ty
                            .send(DeviceEvent::Finger {
                                id: *other_id,
                                time: seconds(evt.time),
                                status: FingerStatus::Up,
                                position: *other_position,
                            })
                            .is_err()
                });
            }

            for (&id, state) in &pipeline.packets {
                if state.pressure > 0 {
                    tracing::debug!(
                        id,
                        raw_packet = ?state,
                        final_position = ?state.position,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        rotation,
                        mirror_x,
                        mirror_y,
                        dims = ?dims,
                        "touch packet"
                    );
                }
                if let Some(&pos) = pipeline.fingers.get(&id) {
                    if state.pressure > 0 {
                        if state.position != pos {
                            ty.send(DeviceEvent::Finger {
                                id,
                                time: seconds(evt.time),
                                status: FingerStatus::Motion,
                                position: state.position,
                            })
                            .unwrap();
                            pipeline.fingers.insert(id, state.position);
                        }
                    } else {
                        ty.send(DeviceEvent::Finger {
                            id,
                            time: seconds(evt.time),
                            status: FingerStatus::Up,
                            position: state.position,
                        })
                        .unwrap();
                        pipeline.fingers.remove(&id);
                    }
                } else if state.pressure > 0 {
                    tracing::debug!(
                        id,
                        position = ?state.position,
                        pressure = state.pressure,
                        rotation,
                        mirror_x,
                        mirror_y,
                        dims = ?dims,
                        "finger down"
                    );
                    ty.send(DeviceEvent::Finger {
                        id,
                        time: seconds(evt.time),
                        status: FingerStatus::Down,
                        position: state.position,
                    })
                    .unwrap();
                    pipeline.fingers.insert(id, state.position);
                }
            }

            if proto != TouchProto::Single {
                pipeline.packets.clear();
            }
            } else if evt.code == SYN_DROPPED {
                counters.dropped_syn += 1;
                tracing::warn!(
                    source = ?device,
                    dropped = counters.dropped_syn,
                    "kernel dropped input events"
                );
                if let Some(source) = device
                    && let Some(pipeline) = touch.remove(&source)
                {
                    for (id, position) in pipeline.fingers {
                        ty.send(DeviceEvent::Finger {
                            id,
                            time: seconds(evt.time),
                            status: FingerStatus::Up,
                            position,
                        })
                        .ok();
                    }
                }
            }
        } else if evt.kind == EV_KEY {
            counters.key += 1;
            if evt.code == BTN_TOUCH {
                // Touch-contact signal from a touch/pen device; not a button.
            } else {
                match tagged.profile.lookup(evt.code) {
                    Some(KeyAction::SleepCover) => {
                        if evt.value == VAL_PRESS || evt.value == VAL_REPEAT {
                            ty.send(DeviceEvent::CoverOn).ok();
                        } else if evt.value == VAL_RELEASE {
                            ty.send(DeviceEvent::CoverOff).ok();
                        }
                    }
                    Some(KeyAction::ButtonScheme) => {
                        button_scheme = if evt.value == VAL_PRESS {
                            ButtonScheme::Inverted
                        } else {
                            ButtonScheme::Natural
                        };
                    }
                    Some(KeyAction::RotateDisplay) => {
                        let next_rotation = evt.value as i8;
                        tracing::debug!(
                            from_rotation = rotation,
                            to_rotation = next_rotation,
                            dims_before = ?dims,
                            mirror_x_before = mirror_x,
                            mirror_y_before = mirror_y,
                            tc_x = tc.x,
                            tc_y = tc.y,
                            "KEY_ROTATE_DISPLAY received"
                        );
                        if next_rotation != rotation {
                            let delta = (rotation - next_rotation).abs();
                            let will_swap_axes = delta % 2 == 1;
                            if will_swap_axes {
                                mem::swap(&mut tc.x, &mut tc.y);
                                mem::swap(&mut dims.0, &mut dims.1);
                            }
                            rotation = next_rotation;
                            let should_mirror = compute_mirror_axes(rotation, mirroring_scheme);
                            mirror_x = should_mirror.0;
                            mirror_y = should_mirror.1;
                            tracing::debug!(
                                delta,
                                will_swap_axes,
                                rotation,
                                mirror_x,
                                mirror_y,
                                dims_after = ?dims,
                                tc_x = tc.x,
                                tc_y = tc.y,
                                axes_swapped = rotation % 2 == swapping_scheme,
                                "rotation applied to input mapping"
                            );
                        } else {
                            tracing::debug!(
                                rotation,
                                "KEY_ROTATE_DISPLAY matches current input rotation; no remap"
                            );
                        }
                    }
                    Some(action) => {
                        if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                            let time = seconds(evt.time);
                            let code = action.to_button_code(
                                rotation,
                                button_scheme,
                                startup_rotation,
                                dir,
                            );
                            tracing::debug!(
                                code = ?code,
                                status = ?button_status,
                                time,
                                raw_code = evt.code,
                                raw_value = evt.value,
                                "decoded button event"
                            );
                            ty.send(DeviceEvent::Button {
                                time,
                                code,
                                status: button_status,
                            })
                            .ok();
                        }
                    }
                    None => {
                        if tagged.profile.tracks_modifiers
                            && let Some(source) = device
                        {
                            // Per-source state: a keyboard that disconnects
                            // with a modifier held never sends the release, so
                            // its bit must not poison other keyboards. Unseen
                            // sources start from a default `Modifiers`.
                            let modifiers = modifiers.entry(source).or_default();
                            if let Some(modifier) = Modifier::from_evdev_code(evt.code) {
                                modifiers.set_depressed(modifier, evt.value != 0);
                                tracing::trace!(
                                    modifier = ?modifier,
                                    mods_depressed = modifiers.mods_depressed,
                                    keycode = evt.code,
                                    value = evt.value,
                                    "keyboard modifier state change"
                                );
                            } else if let Some(button_status) =
                                ButtonStatus::try_from_raw(evt.value)
                            {
                                ty.send(DeviceEvent::Button {
                                    time: seconds(evt.time),
                                    code: ButtonCode::Keyboard(KeyEvent {
                                        keycode: evt.code as u32,
                                        modifiers: *modifiers,
                                    }),
                                    status: button_status,
                                })
                                .ok();
                            }
                        } else if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                            let time = seconds(evt.time);
                            let code = ButtonCode::Raw(evt.code);
                            tracing::debug!(
                                code = ?code,
                                status = ?button_status,
                                time,
                                raw_code = evt.code,
                                raw_value = evt.value,
                                "decoded button event"
                            );
                            ty.send(DeviceEvent::Button {
                                time,
                                code,
                                status: button_status,
                            })
                            .ok();
                        }
                    }
                }
            }
        } else if evt.kind == EV_MSC && evt.code == MSC_RAW {
            if let Some(next_rotation) =
                gsensor_rotation_from_raw(evt.value, gyro_rotation_transform)
            {
                tracing::trace!(
                    raw_value = evt.value,
                    next_rotation,
                    current_rotation = rotation,
                    "gyroscope rotation event"
                );
                ty.send(DeviceEvent::RotateScreen(next_rotation)).ok();
            }
        }
            }
            _ = heartbeat.tick() => {
                tracing::trace!(
                    abs = counters.abs,
                    key = counters.key,
                    syn = counters.syn,
                    unmapped_abs = counters.unmapped_abs,
                    touch_packets = counters.touch_packets,
                    dropped_syn = counters.dropped_syn,
                    sources = touch.len(),
                    tc_x = tc.x,
                    tc_y = tc.y,
                    "input decoder heartbeat"
                );
            }
        }
    }
}

// Ioctl wrappers for evdev device introspection.
//
// `EVIOCGID` returns the bustype/vendor/product/version tuple used to
// deduplicate devices that the kernel exposes under multiple event nodes.
// `EVIOCGEV` returns the bitmap of event types supported by a device, used
// to filter for input devices (those that emit `EV_KEY` or `EV_ABS`).
ioctl_read!(eviocgid, b'E', 0x02, InputId);

#[repr(C)]
#[derive(Debug, Clone)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

ioctl_read_buf!(eviocgbit_ev, b'E', 0x20, u8);
// `EVIOCGBIT(EV_ABS, ...)` — the bitmap of ABS axes a device supports, used to
// distinguish touch digitizers (position axes) from keyboards whose HID
// descriptor happens to declare a stray axis (e.g. `ABS_MISC`).
ioctl_read_buf!(eviocgbit_abs, b'E', 0x20 + EV_ABS, u8);

// `EVIOCGNAME` — returns the kernel device name string, used to identify
// specific hardware such as the Kobo Remote Bluetooth keyboard by name.
ioctl_read_buf!(eviocgname, b'E', 0x06, u8);

// Linux input bus types (`BUS_*` from `<linux/input.h>`), used by the
// fallback heuristics in [`classify_device`]. Built-in hardware typically
// reports `BUS_HOST` or `BUS_I2C`; the Kobo Remote is identified by name, not
// bustype. USB and unidentified bustypes fall through to the generic
// keyboard profile.
const BUS_I2C: u16 = 0x18;
const BUS_HOST: u16 = 0x19;

/// The kernel name prefix reported by the Kobo Remote Bluetooth keyboard via
/// `EVIOCGNAME`. The full name varies by host Bluetooth stack — `"Kobo Remote"`
/// on Kobo devices, `"Kobo Remote Keyboard"` on BlueZ/uhid — so a prefix match
/// is used. Arrow keys on this device map to page-turn buttons via the remote
/// [`DeviceProfile`].
const KOBO_REMOTE_NAME_PREFIX: &str = "Kobo Remote";

/// Returns `true` if `name` identifies a Kobo Remote Bluetooth keyboard. The
/// full name varies by host Bluetooth stack, so a prefix match is used.
fn is_kobo_remote_name(name: &str) -> bool {
    name.starts_with(KOBO_REMOTE_NAME_PREFIX)
}

/// Hard-coded kernel names (`EVIOCGNAME`) of built-in Kobo input devices.
///
/// Bus-type and `EVIOCGID` probing alone cannot distinguish some built-in
/// devices: on current Kobo hardware, `gpio-keys` (the page-turn / home /
/// light buttons) and `bd71828-pwrkey` (the PMIC power key) report the
/// *same* `EVIOCGID` tuple (`bus 0x19, vendor 0x1, product 0x1, version
/// 0x100`). The feature scan classifies each device by name so each one is
/// unambiguously identified and the deduplicator does not collapse two
/// distinct devices into one.
///
/// Unknown devices still fall through to the bus-type heuristics in
/// [`classify_device`], so other Kobo models with different touch digitizer
/// names (`ft5x06_ts`, `Wacom Digitizer`, …) continue to work.
const BUILT_IN_DEVICE_NAMES: &[&str] = &["Elan Touchscreen", "bd71828-pwrkey", "gpio-keys"];

/// Returns `true` if `name` is one of the hard-coded built-in device names.
fn is_built_in_name(name: &str) -> bool {
    BUILT_IN_DEVICE_NAMES.contains(&name)
}

/// Kernel-reported features of an evdev device, probed via `EVIOCGID`,
/// `EVIOCGNAME`, and `EVIOCGEV`. Used to classify each device into a
/// [`DeviceProfile`] and to deduplicate devices exposed under multiple event
/// nodes.
struct DeviceFeatures {
    name: Option<String>,
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    emits_key: bool,
    emits_abs: bool,
    /// The device emits `EV_ABS` events with position axes (`ABS_X`/`ABS_Y` or
    /// `ABS_MT_POSITION_X`/`ABS_MT_POSITION_Y`), i.e. it is a touch or pen
    /// digitizer. Keyboards whose HID descriptor declares a stray axis (e.g.
    /// `ABS_MISC`) do not qualify.
    has_position_axes: bool,
}

impl DeviceFeatures {
    /// Returns a stable identity string for deduplication, based on the
    /// device name and the bustype/vendor/product/version tuple. Returns
    /// `None` when the device reports no identity (all zeros and no name),
    /// so callers fall back to path-based deduplication.
    ///
    /// The device name is included because distinct built-in devices can
    /// share an `EVIOCGID` tuple: `gpio-keys` and `bd71828-pwrkey` both
    /// report `bus 0x19, vendor 0x1, product 0x1, version 0x100` on current
    /// Kobo hardware. Without the name, the deduplicator would treat the
    /// power key as a duplicate of the page-turn buttons and drop one of
    /// them.
    fn identity(&self) -> Option<String> {
        let name = self.name.as_deref().unwrap_or("");
        if name.is_empty() && self.bustype == 0 && self.vendor == 0 && self.product == 0 {
            return None;
        }
        Some(format!(
            "id:{}:{:04x}:{:04x}:{:04x}:{:04x}",
            name, self.bustype, self.vendor, self.product, self.version
        ))
    }
}

/// Returns `true` if the ABS-axes bitmap (`EVIOCGBIT(EV_ABS, ...)`) declares
/// support for `axis`.
fn has_abs_axis(absbits: &[u8; 8], axis: u16) -> bool {
    (absbits[axis as usize / 8] & (1 << (axis % 8))) != 0
}

/// Probes the device at `path` and returns its [`DeviceFeatures`].
///
/// Opens the device once and issues all introspection ioctls on the resulting
/// file descriptor. Returns `None` only if the device cannot be opened.
/// # Safety of ioctls
///
/// The ioctls write into pre-zeroed buffers and their results are checked, so
/// partial failures leave defaults in place.
///
/// The ioctls are issued inside `spawn_blocking`: they have no async
/// equivalent and must not block the Tokio worker pool.
async fn probe_device(path: &str) -> Option<DeviceFeatures> {
    let file = tokio::fs::File::open(path).await.ok()?;
    tokio::task::spawn_blocking(move || {
        let fd = file.as_raw_fd();
        let mut id = InputId {
            bustype: 0,
            vendor: 0,
            product: 0,
            version: 0,
        };
        let _ = unsafe { eviocgid(fd, &mut id) };

        let mut name_buf = [0u8; 256];
        let name = unsafe { eviocgname(fd, &mut name_buf) }.ok().and_then(|n| {
            std::str::from_utf8(&name_buf[..n as usize])
                .ok()
                .map(|s| s.trim_end_matches('\0').to_string())
        });

        let (emits_key, emits_abs) = {
            let mut evbits = [0u8; 4];
            if unsafe { eviocgbit_ev(fd, &mut evbits) }.is_err() {
                (false, false)
            } else {
                // EV_KEY is event type 1; EV_ABS is event type 3.
                ((evbits[0] & (1 << 1)) != 0, (evbits[0] & (1 << 3)) != 0)
            }
        };

        // ABS axes bitmap: only digitizer position axes mark a device as a
        // touch screen for classification. Keyboards created through uhid can
        // declare a stray vendor axis (e.g. ABS_MISC), which must not turn
        // them into built-in hardware.
        let has_position_axes = emits_abs && {
            let mut absbits = [0u8; 8];
            unsafe { eviocgbit_abs(fd, &mut absbits) }.is_ok()
                && (has_abs_axis(&absbits, ABS_X) || has_abs_axis(&absbits, ABS_MT_POSITION_X))
        };

        DeviceFeatures {
            name,
            bustype: id.bustype,
            vendor: id.vendor,
            product: id.product,
            version: id.version,
            emits_key,
            emits_abs,
            has_position_axes,
        }
    })
    .await
    .ok()
}

/// Classifies a device into a [`DeviceProfile`] from its probed features.
///
/// Dispatch is by hard-coded device name first, with bus-type / event-bit
/// heuristics as a fallback for built-in hardware not listed in
/// [`BUILT_IN_DEVICE_NAMES`] (e.g. other Kobo touch digitizers). Rules, in
/// priority order:
///
/// - Kobo Remote Bluetooth keyboard (name prefix `"Kobo Remote"`) → remote
///   profile: up/down arrows map to raw page-turn.
/// - Hard-coded built-in device name (see [`BUILT_IN_DEVICE_NAMES`]) →
///   built-in profile: power, home, light, scheme-aware page-turn, sleep
///   cover, and pen buttons.
/// - A digitizer (position axes), or a device reporting `BUS_HOST` /
///   `BUS_I2C` → built-in profile. Catches touch digitizers and other
///   built-in buttons on Kobo models whose names are not listed above.
///   Position axes are required for the `EV_ABS` signal because keyboards
///   created through uhid can declare stray vendor axes (e.g. `ABS_MISC`)
///   without being built-in hardware.
/// - Anything else → generic keyboard profile: forward as
///   [`ButtonCode::Keyboard`].
fn classify_device(features: &DeviceFeatures) -> DeviceProfile {
    let name = features.name.as_deref().unwrap_or("");
    if is_kobo_remote_name(name) {
        return DeviceProfile::remote();
    }
    if is_built_in_name(name) {
        return DeviceProfile::built_in();
    }
    if features.has_position_axes || features.bustype == BUS_HOST || features.bustype == BUS_I2C {
        return DeviceProfile::built_in();
    }
    DeviceProfile::keyboard()
}

/// Scans `/dev/input/event*` for input devices not already tracked.
///
/// Returns each new device's path alongside its probed [`DeviceFeatures`] so the
/// caller can classify it without re-probing. Excludes paths in `tracked_paths`
/// (already opened persistent + removable devices) and `blacklisted_paths`
/// (known duplicates). Deduplicates by device identity so a device exposed
/// under multiple event nodes is only opened once; duplicates are added to
/// `blacklisted_paths` to avoid re-querying them on every scan.
///
/// Blacklisted paths whose node no longer exists are dropped during the scan:
/// the kernel reuses `eventN` numbers, so a stale entry would otherwise cause
/// a different device later assigned the same node to be skipped forever. The
/// current path set comes from the same directory listing the scan walks, so
/// the cleanup adds no extra filesystem access.
///
/// Unlike the previous keyboard-only scan, this opens any device that emits
/// `EV_KEY` or `EV_ABS`, so the touch digitizer and built-in buttons are
/// discovered at startup alongside hotplugged keyboards.
async fn scan_input_devices(
    tracked_paths: &[String],
    known_identities: &mut FxHashSet<String>,
    blacklisted_paths: &mut FxHashSet<String>,
) -> Vec<(String, DeviceFeatures)> {
    let mut found = Vec::new();
    let mut entries = match tokio::fs::read_dir("/dev/input").await {
        Ok(e) => e,
        Err(_) => return found,
    };

    let mut current_paths = FxHashSet::default();
    while let Some(entry) = entries.next_entry().await.ok().flatten() {
        let path = entry.path();
        let Some(path_str) = path.to_str() else {
            continue;
        };
        current_paths.insert(path_str.to_string());
        if !path_str.starts_with("/dev/input/event") {
            continue;
        }
        if tracked_paths.iter().any(|p| p == path_str) || blacklisted_paths.contains(path_str) {
            continue;
        }
        let Some(features) = probe_device(path_str).await else {
            continue;
        };
        if !features.emits_key && !features.emits_abs {
            continue;
        }
        match features.identity() {
            Some(identity) => {
                if known_identities.insert(identity.clone()) {
                    tracing::debug!(device = path_str, identity, "scan: new device");
                    found.push((path_str.to_string(), features));
                } else {
                    tracing::debug!(
                        device = path_str,
                        identity,
                        "scan: blacklisting duplicate device"
                    );
                    blacklisted_paths.insert(path_str.to_string());
                }
            }
            None => {
                tracing::debug!(device = path_str, "scan: device without identity");
                found.push((path_str.to_string(), features));
            }
        }
    }

    forget_missing_blacklisted_paths(blacklisted_paths, &current_paths);

    found
}

/// Drops blacklisted paths that no longer exist under `/dev/input`, so a
/// future device reusing a recycled `eventN` node is probed again instead of
/// being skipped forever.
fn forget_missing_blacklisted_paths(
    blacklisted_paths: &mut FxHashSet<String>,
    current_paths: &FxHashSet<String>,
) {
    blacklisted_paths.retain(|path| {
        let exists = current_paths.contains(path);
        if !exists {
            tracing::debug!(device = path, "forgetting blacklisted path: node is gone");
        }
        exists
    });
}

/// Reads raw evdev events from all input devices and forwards them as
/// [`TaggedInputEvent`]s on `tx`.
///
/// Every device under `/dev/input/event*` is probed at startup, classified into
/// a [`DeviceProfile`], and opened as a **persistent** device. Built-in nodes
/// (`/dev/input/eventN`) outlive driver hiccups, so a reader that exits from a
/// driver error is re-opened on the next scan rather than dropped for the rest
/// of the session. A periodic rescan discovers **removable** devices hotplugged
/// after startup, and when a reader stops the device's identity is released so
/// a replug is re-detected instead of being treated as a duplicate.
async fn parse_raw_events(
    cancel: &CancellationToken,
    display: Display,
    proto: TouchProto,
    tx: &UnboundedSender<TaggedInputEvent>,
) {
    /// How often `/dev/input/event*` is rescanned for hotplugged devices.
    const SCAN_INTERVAL: Duration = Duration::from_secs(5);

    let mut device = OpenInputDevices {
        watched: Vec::new(),
        readers: tokio::task::JoinSet::new(),
        known_identities: FxHashSet::default(),
        blacklisted_paths: FxHashSet::default(),
        proto,
        display,
        tx,
        next_source: SourceId::START,
    };

    device.open(&[], false).await;

    let mut last_scan = Instant::now();
    loop {
        let next_scan = SCAN_INTERVAL.saturating_sub(last_scan.elapsed());
        let readers_busy = !device.readers.is_empty();
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(next_scan) => {}
            Some(joined) = device.readers.join_next(), if readers_busy => {
                device.reap(joined);
            }
        }

        if last_scan.elapsed() >= SCAN_INTERVAL {
            last_scan = Instant::now();
            device.open(&device.tracked(), true).await;
        }
    }
}

/// Owns the evdev devices the input reader currently watches.
///
/// One reader task per open device forwards events on `tx`; when a task ends it
/// returns the device's path and dedup identity so [`Self::reap`] can release
/// them, letting a later scan re-open a persistent node or re-detect a replugged
/// removable one.
struct OpenInputDevices<'a> {
    watched: Vec<(String, Option<String>)>,
    readers: tokio::task::JoinSet<(String, Option<String>)>,
    known_identities: FxHashSet<String>,
    blacklisted_paths: FxHashSet<String>,
    proto: TouchProto,
    display: Display,
    tx: &'a UnboundedSender<TaggedInputEvent>,
    /// Monotonic counter so each opened device gets a unique [`SourceId`].
    /// Ids of devices whose readers exited are never reused, so stale entries
    /// in per-source state can never be confused with a new device.
    next_source: SourceId,
}

impl OpenInputDevices<'_> {
    /// Paths currently watched, passed to the scan as already tracked.
    fn tracked(&self) -> Vec<String> {
        self.watched.iter().map(|(path, _)| path.clone()).collect()
    }

    /// Scans for devices not already tracked and opens a reader for each.
    ///
    /// Each device is classified into a [`DeviceProfile`] from its probed
    /// features; that profile tags every event its reader forwards, so the
    /// device-event pipeline can route the device's scancodes through the right
    /// table. `hotplugged` only distinguishes the log line between the startup
    /// set and devices that appeared later.
    async fn open(&mut self, tracked: &[String], hotplugged: bool) {
        for (path, features) in scan_input_devices(
            tracked,
            &mut self.known_identities,
            &mut self.blacklisted_paths,
        )
        .await
        {
            let mut touch_axes: Option<TouchAxes> = None;
            let file = match tokio::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
                .await
            {
                Ok(file) => file.into_std().await,
                Err(error) => {
                    tracing::warn!(
                        device = %path,
                        error = %error,
                        "failed to open input device; skipping"
                    );
                    self.forget_scanned_identity(&features);
                    continue;
                }
            };
            if features.emits_abs {
                touch_axes =
                    Some(trace_touch_device_geometry(&path, self.proto, self.display).await);
            }
            let fd = match AsyncFd::with_interest(file, Interest::READABLE) {
                Ok(fd) => fd,
                Err(error) => {
                    tracing::warn!(
                        device = %path,
                        error = %error,
                        "failed to watch input device; skipping"
                    );
                    self.forget_scanned_identity(&features);
                    continue;
                }
            };
            let profile = classify_device(&features);
            let identity = features.identity();
            let source = self.next_source.next();
            tracing::info!(
                device = %path,
                profile = ?profile,
                source = ?source,
                name = features.name.as_deref().unwrap_or(""),
                bus = features.bustype,
                vendor = features.vendor,
                product = features.product,
                version = features.version,
                identity = identity.as_deref().unwrap_or(""),
                emits_key = features.emits_key,
                emits_abs = features.emits_abs,
                touch_candidate = features.has_position_axes,
                abs_x = ?touch_axes.as_ref().and_then(|(x, _)| *x),
                abs_y = ?touch_axes.as_ref().and_then(|(_, y)| *y),
                hotplugged,
                "opened input device"
            );
            let tx = self.tx.clone();
            let path_for_reader = path.clone();
            let identity_for_reader = identity.clone();
            self.readers.spawn(async move {
                let exit = read_input_events(fd, &path_for_reader, profile, source, &tx).await;
                tracing::warn!(
                    device = %path_for_reader,
                    reason = %exit.reason,
                    events_read = exit.events_read,
                    open_ms = exit.open_ms,
                    "input device reader stopped"
                );
                (path_for_reader, identity_for_reader)
            });
            self.watched.push((path, identity));
        }
    }

    /// Releases the dedup identity of a device that was scanned but not opened,
    /// so the next scan retries it instead of treating it as a duplicate.
    fn forget_scanned_identity(&mut self, features: &DeviceFeatures) {
        if let Some(identity) = features.identity() {
            self.known_identities.remove(&identity);
        }
    }

    /// Releases the device whose reader just ended.
    fn reap(&mut self, joined: Result<(String, Option<String>), tokio::task::JoinError>) {
        if let Ok((path, identity)) = joined {
            if let Some(pos) = self.watched.iter().position(|(p, _)| p == &path) {
                self.watched.remove(pos);
            }
            if let Some(identity) = identity.as_ref() {
                self.known_identities.remove(identity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn forget_missing_blacklisted_paths_drops_gone_nodes_and_keeps_existing() {
        let mut blacklisted = FxHashSet::default();
        blacklisted.insert("/dev/input/event3".to_string());
        blacklisted.insert("/dev/input/event7".to_string());

        let mut current = FxHashSet::default();
        current.insert("/dev/input/event3".to_string());
        current.insert("/dev/input/event9".to_string());

        forget_missing_blacklisted_paths(&mut blacklisted, &current);

        assert!(blacklisted.contains("/dev/input/event3"));
        assert_eq!(blacklisted.len(), 1);
    }

    /// A device that was scanned but never opened (open or watch failed) must
    /// have its dedup identity released: otherwise the next scan treats the
    /// same node as a duplicate of itself and blacklists it until restart.
    #[tokio::test]
    async fn forget_scanned_identity_releases_unopened_device() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut devices = OpenInputDevices {
            watched: Vec::new(),
            readers: tokio::task::JoinSet::new(),
            known_identities: FxHashSet::default(),
            blacklisted_paths: FxHashSet::default(),
            proto: TouchProto::MultiB,
            display: Display {
                dims: (1264, 1680),
                rotation: 1,
            },
            tx: &tx,
            next_source: SourceId::START,
        };

        let features = DeviceFeatures {
            name: Some("virtual-keyboard".to_string()),
            // USB keyboard (bustype 0x03 = BUS_USB).
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let identity = features.identity().expect("identity");
        devices.known_identities.insert(identity.clone());

        devices.forget_scanned_identity(&features);

        assert!(!devices.known_identities.contains(&identity));
    }

    // Bus type for Bluetooth devices, used only in tests to verify that
    // classification is by name (not bus type) for the Kobo Remote. The
    // production code identifies the Kobo Remote by name prefix and does not
    // consult the bustype, so this constant is test-only.
    const BUS_BLUETOOTH: u16 = 0x05;

    #[test]
    fn feed_usb_status_chunk_reassembles_split_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"usb plug ad", &tx);
        assert!(pending == b"usb plug ad");
        assert!(rx.try_recv().is_err());
        feed_usb_status_chunk(&mut pending, b"d\n", &tx);
        assert!(pending.is_empty());
        assert!(matches!(
            rx.try_recv().expect("plug event"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
    }

    #[test]
    fn feed_usb_status_chunk_handles_multiple_lines_and_crlf() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(
            &mut pending,
            b"usb ac add\r\nusb ac remove\nusb plug remove\n",
            &tx,
        );
        assert!(matches!(
            rx.try_recv().expect("wall plug"),
            DeviceEvent::Plug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("wall unplug"),
            DeviceEvent::Unplug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("host unplug"),
            DeviceEvent::Unplug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_usb_status_chunk_ignores_unknown_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"noise\nusb plug add\n", &tx);
        assert!(matches!(
            rx.try_recv().expect("host plug"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }

    /// Regression test for [GitHub issue #1089](https://github.com/OGKevin/cadmus/issues/1089):
    /// a quarter-turn rotation must swap the cached input dimensions so
    /// mirrored touch positions use the rotated screen bounds. The regression
    /// was introduced in commit `9b91d69a4`.
    #[tokio::test]
    async fn touch_mapping_swaps_dims_when_rotating_to_landscape() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (2, 1),
            swapping_scheme: 1,
            startup_rotation: 1,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        let screen = Display {
            dims: (1264, 1680),
            rotation: 1,
        };
        tokio::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(&mut rx, &ty, screen, ButtonScheme::Natural, info).await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(SourceId(0)),
        };
        for evt in [
            tagged(EV_KEY, KEY_ROTATE_DISPLAY, 2),
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(EV_ABS, ABS_MT_POSITION_X, 59),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 1178),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }

        let finger = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    position,
                    ..
                }) = ry.recv().await
                {
                    return position;
                }
            }
        })
        .await
        .expect("timed out waiting for touch position");

        assert_eq!(finger, Point { x: 1620, y: 85 });
    }

    /// Receives the next device event, skipping the `UserActivity` heartbeat
    /// triggered by the first `SYN_REPORT` after a 60 s idle window.
    async fn next_input_event(
        ry: &mut tokio::sync::mpsc::UnboundedReceiver<DeviceEvent>,
    ) -> DeviceEvent {
        loop {
            match ry.recv().await {
                Some(DeviceEvent::UserActivity) => continue,
                Some(event) => return event,
                None => panic!("input event channel closed"),
            }
        }
    }

    /// Regression: a `SYN_REPORT` from a non-touch device (e.g. a hardware
    /// button) must not lift fingers still in contact on the digitizer.
    #[tokio::test]
    async fn syn_report_from_other_source_does_not_lift_fingers() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        tokio::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |source, kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(source),
        };
        let touch = SourceId(0);
        let power_button = SourceId(1);
        for evt in [
            tagged(touch, EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(touch, EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(touch, EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(touch, EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(touch, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    ..
                }
            ),
            "finger must be down after the digitizer's report"
        );

        // Power button press: its EV_KEY report ends with its own SYN_REPORT.
        for evt in [
            tagged(power_button, EV_KEY, KEY_POWER, VAL_PRESS),
            tagged(power_button, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(next_input_event(&mut ry).await, DeviceEvent::Button { .. }),
            "the power press must arrive as a button event"
        );
        assert!(
            ry.try_recv().is_err(),
            "a foreign SYN_REPORT must not emit Finger Up events"
        );

        // The finger is still tracked: moving it produces Motion, not Down.
        for evt in [
            tagged(touch, EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(touch, EV_ABS, ABS_MT_POSITION_X, 11),
            tagged(touch, EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(touch, EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(touch, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Motion,
                    ..
                }
            ),
            "the finger must survive the foreign SYN_REPORT and report Motion"
        );
    }

    /// A dropped-event notification from the kernel discards the digitizer's
    /// report state, so in-contact fingers must be lifted explicitly instead of
    /// staying tracked forever.
    #[tokio::test]
    async fn syn_dropped_lifts_tracked_fingers() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        tokio::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(SourceId(0)),
        };
        for evt in [
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 7),
            tagged(EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        let down_position = match next_input_event(&mut ry).await {
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                position,
                ..
            } => position,
            other => panic!("expected Finger Down before the drop, got {other:?}"),
        };

        raw_tx
            .send(tagged(EV_SYN, SYN_DROPPED, 0))
            .expect("send raw event");

        match next_input_event(&mut ry).await {
            DeviceEvent::Finger {
                id,
                status: FingerStatus::Up,
                position,
                ..
            } => {
                assert_eq!(id, 7);
                assert_eq!(position, down_position);
            }
            other => panic!("expected Finger Up for the discarded finger, got {other:?}"),
        }

        for evt in [
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 7),
            tagged(EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    ..
                }
            ),
            "the discarded pipeline must not keep the finger tracked"
        );
    }

    /// Regression: a modifier held on one keyboard when it disconnects must not
    /// latch into keyboard events from another keyboard.
    #[tokio::test]
    async fn modifiers_do_not_leak_between_sources() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        tokio::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let keyboard = |source, code, value| TaggedInputEvent {
            event: event(EV_KEY, code, value),
            profile: DeviceProfile::keyboard(),
            source: EventSource::Device(source),
        };

        // Keyboard A presses Shift and never releases it (disconnects).
        let a = SourceId(0);
        raw_tx
            .send(keyboard(a, KEY_LEFTSHIFT, VAL_PRESS))
            .expect("send shift press");
        // Keyboard B types a plain key.
        let b = SourceId(1);
        raw_tx
            .send(keyboard(b, KEY_SPACE, VAL_PRESS))
            .expect("send space press");

        match ry.recv().await {
            Some(DeviceEvent::Button {
                code: ButtonCode::Keyboard(key),
                ..
            }) => {
                assert_eq!(key.keycode, KEY_SPACE as u32);
                assert_eq!(
                    key.modifiers,
                    Modifiers::default(),
                    "B must not see A's stale Shift"
                );
            }
            other => panic!("expected a keyboard button event, got {other:?}"),
        }
    }

    #[test]
    fn modifier_bit_positions_match_xkb_real_mod_order() {
        assert_eq!(Modifier::Shift.bit(), 1 << 0);
        assert_eq!(Modifier::Caps.bit(), 1 << 1);
        assert_eq!(Modifier::Ctrl.bit(), 1 << 2);
        assert_eq!(Modifier::Alt.bit(), 1 << 3);
        assert_eq!(Modifier::NumLock.bit(), 1 << 4);
        assert_eq!(Modifier::Mod3.bit(), 1 << 5);
        assert_eq!(Modifier::Super.bit(), 1 << 6);
        assert_eq!(Modifier::AltGr.bit(), 1 << 7);
    }

    #[test]
    fn modifier_from_evdev_code_maps_known_keys() {
        assert_eq!(
            Modifier::from_evdev_code(KEY_LEFTSHIFT),
            Some(Modifier::Shift)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_RIGHTSHIFT),
            Some(Modifier::Shift)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_CAPSLOCK),
            Some(Modifier::Caps)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_LEFTCTRL),
            Some(Modifier::Ctrl)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_RIGHTCTRL),
            Some(Modifier::Ctrl)
        );
        assert_eq!(Modifier::from_evdev_code(KEY_LEFTALT), Some(Modifier::Alt));
        assert_eq!(Modifier::from_evdev_code(KEY_RIGHTALT), Some(Modifier::Alt));
        assert_eq!(
            Modifier::from_evdev_code(KEY_NUMLOCK),
            Some(Modifier::NumLock)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_LEFTMETA),
            Some(Modifier::Super)
        );
        assert_eq!(
            Modifier::from_evdev_code(KEY_RIGHTMETA),
            Some(Modifier::Super)
        );
    }

    #[test]
    fn modifier_from_evdev_code_returns_none_for_non_modifier() {
        assert_eq!(Modifier::from_evdev_code(KEY_SPACE), None);
        assert_eq!(Modifier::from_evdev_code(KEY_POWER), None);
        assert_eq!(Modifier::from_evdev_code(KEY_HOME), None);
    }

    #[test]
    fn modifiers_default_is_empty() {
        let mods = Modifiers::default();
        assert_eq!(mods.mods_depressed, 0);
        assert_eq!(mods.mods_latched, 0);
        assert_eq!(mods.mods_locked, 0);
        assert_eq!(mods.group, 0);
        assert_eq!(mods.effective(), 0);
        assert!(!mods.shift());
        assert!(!mods.ctrl());
        assert!(!mods.alt());
        assert!(!mods.super_());
        assert!(!mods.caps());
        assert!(!mods.num_lock());
        assert!(!mods.alt_gr());
    }

    #[test]
    fn modifiers_set_depressed_toggles_bit() {
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Ctrl, true);
        assert!(mods.is_depressed(Modifier::Ctrl));
        assert!(mods.ctrl());
        assert_eq!(mods.mods_depressed, Modifier::Ctrl.bit());

        mods.set_depressed(Modifier::Shift, true);
        assert!(mods.shift());
        assert_eq!(
            mods.mods_depressed,
            Modifier::Ctrl.bit() | Modifier::Shift.bit()
        );

        mods.set_depressed(Modifier::Ctrl, false);
        assert!(!mods.is_depressed(Modifier::Ctrl));
        assert!(mods.shift());
        assert_eq!(mods.mods_depressed, Modifier::Shift.bit());
    }

    #[test]
    fn modifiers_effective_combines_all_sources() {
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Shift, true);
        mods.mods_latched = Modifier::Ctrl.bit();
        mods.mods_locked = Modifier::Caps.bit();
        assert_eq!(
            mods.effective(),
            Modifier::Shift.bit() | Modifier::Ctrl.bit() | Modifier::Caps.bit()
        );
        assert!(mods.shift());
        assert!(mods.ctrl());
        assert!(mods.caps());
    }

    #[test]
    fn modifiers_has_checks_effective_mask() {
        let mods = Modifiers {
            mods_locked: Modifier::NumLock.bit(),
            ..Default::default()
        };
        assert!(mods.has(Modifier::NumLock));
        assert!(mods.num_lock());
        assert!(!mods.is_depressed(Modifier::NumLock));
    }

    #[test]
    fn key_state_from_evdev_value() {
        assert_eq!(
            KeyState::from_evdev_value(VAL_RELEASE),
            Some(KeyState::Released)
        );
        assert_eq!(
            KeyState::from_evdev_value(VAL_PRESS),
            Some(KeyState::Pressed)
        );
        assert_eq!(
            KeyState::from_evdev_value(VAL_REPEAT),
            Some(KeyState::Repeated)
        );
        assert_eq!(KeyState::from_evdev_value(42), None);
        assert_eq!(KeyState::from_evdev_value(-1), None);
    }

    #[tokio::test]
    async fn raw_reader_forwards_one_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = tokio::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 4,
                tv_usec: 5,
            },
            kind: EV_KEY,
            code: KEY_HOME,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        let mut writer = writer;
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for evdev event")
            .expect("reader closed");
        assert_eq!(got.profile, DeviceProfile::built_in());
        assert_eq!(got.event.kind, EV_KEY);
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.value, VAL_PRESS);
        assert_eq!(got.event.time.tv_sec, 4);
        assert_eq!(got.event.time.tv_usec, 5);

        // The reader must report one forwarded event and a clean EOF exit once
        // the write end closes, not a silent Ok(()) — a dead device must be
        // visible in the log.
        drop(writer);
        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::Eof));
        assert_eq!(exit.events_read, 1);
        let _ = std::fs::remove_file(&path);
    }

    /// A device node that delivers zero bytes (the kernel-side EOF case) must
    /// surface as [`ExitReason::Eof`], not as a silent return.
    #[tokio::test]
    async fn read_input_events_reports_eof_for_empty_device() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader_path = path_str.clone();
        let reader = tokio::spawn(async move {
            read_input_events(
                fd,
                reader_path.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        // Open and immediately close the write end so the read end reports EOF.
        let writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(&path_str)
        })
        .await
        .expect("writer task")
        .expect("open fifo");
        drop(writer);

        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::Eof));
        assert_eq!(exit.events_read, 0);
        let _ = std::fs::remove_file(&path);
    }

    /// A reader whose consumers are all gone must stop with
    /// [`ExitReason::ChannelClosed`] and report how many events it had
    /// forwarded — previously this path returned silently.
    #[tokio::test]
    async fn read_input_events_reports_channel_closed() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-closed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader_path = path_str.clone();
        let reader = tokio::spawn(async move {
            read_input_events(
                fd,
                reader_path.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 4,
                tv_usec: 5,
            },
            kind: EV_KEY,
            code: KEY_HOME,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::ChannelClosed));
        assert_eq!(exit.events_read, 0);
        drop(writer);
        let _ = std::fs::remove_file(&path);
    }

    /// `UserActivity` is throttled to one report per activity interval, and the
    /// reference time is updated only when a report is emitted.
    #[test]
    fn user_activity_is_reported_at_the_activity_interval() {
        let (ty, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let timeval = |tv_sec| libc::timeval { tv_sec, tv_usec: 0 };
        let mut last_activity = 0;

        report_user_activity(&mut last_activity, timeval(60), &ty);
        assert!(matches!(rx.try_recv(), Ok(DeviceEvent::UserActivity)));
        assert_eq!(last_activity, 60);

        report_user_activity(&mut last_activity, timeval(30), &ty);
        assert!(
            rx.try_recv().is_err(),
            "inside the interval must stay quiet"
        );
        assert_eq!(last_activity, 60, "reference time only moves on a report");
    }

    /// Reader exits render as stable labels, and the I/O variant carries the
    /// inner error text into the log.
    #[test]
    fn exit_reason_renders_stable_label_and_io_error_text() {
        assert_eq!(ExitReason::Eof.to_string(), "eof");
        assert_eq!(ExitReason::ChannelClosed.to_string(), "channel_closed");

        let io = ExitReason::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let rendered = io.to_string();
        assert!(rendered.starts_with("io_error: "), "{rendered}");
        assert!(
            rendered.to_lowercase().contains("permission denied"),
            "{rendered}"
        );
    }

    /// An `EV_ABS` code outside the active `tc` table is the signature of a
    /// profile/protocol mismatch and must be classified as unmapped.
    #[test]
    fn unmapped_abs_codes_are_classified() {
        assert!(is_unmapped_abs_code(
            ABS_MT_TOUCH_MAJOR,
            &MULTI_TOUCH_CODES_B
        ));
        assert!(!is_unmapped_abs_code(ABS_MT_PRESSURE, &MULTI_TOUCH_CODES_B));
        assert!(!is_unmapped_abs_code(
            ABS_MT_TRACKING_ID,
            &MULTI_TOUCH_CODES_B
        ));
        assert!(is_unmapped_abs_code(ABS_MT_POSITION_X, &SINGLE_TOUCH_CODES));
        assert!(!is_unmapped_abs_code(ABS_X, &SINGLE_TOUCH_CODES));
    }

    /// The device's classified profile must tag every forwarded event, not just
    /// the built-in default: a remote's arrow keys reach the pipeline with the
    /// remote profile so they map to raw page-turn.
    #[tokio::test]
    async fn raw_reader_tags_events_with_device_profile() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = tokio::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::remote(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 7,
                tv_usec: 2,
            },
            kind: EV_KEY,
            code: KEY_UP,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for evdev event")
            .expect("reader closed");
        assert_eq!(got.profile, DeviceProfile::remote());
        assert_eq!(got.event.code, KEY_UP);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }

    /// A record split across two reads must be held until complete, and must
    /// not be mistaken for EOF.
    #[tokio::test]
    async fn raw_reader_holds_a_split_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-split-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = tokio::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(
                bytes.as_mut_ptr().cast::<InputEvent>(),
                InputEvent {
                    time: libc::timeval {
                        tv_sec: 9,
                        tv_usec: 1,
                    },
                    kind: EV_KEY,
                    code: KEY_HOME,
                    value: VAL_PRESS,
                },
            );
        }
        let split = mem::size_of::<InputEvent>() / 2;

        writer.write_all(&bytes[..split]).expect("write first half");
        writer.flush().expect("flush first half");
        assert!(
            rx.try_recv().is_err(),
            "a half record must not be forwarded"
        );

        writer
            .write_all(&bytes[split..])
            .expect("write second half");
        writer.flush().expect("flush second half");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for the joined evdev event")
            .expect("reader closed");
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.time.tv_sec, 9);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn display_rotate_event_uses_built_in_profile() {
        let event = display_rotate_event(1);
        assert_eq!(event.profile, DeviceProfile::built_in());
        assert_eq!(event.event.kind, EV_KEY);
        assert_eq!(event.event.code, KEY_ROTATE_DISPLAY);
        assert_eq!(event.event.value, 1);
    }

    #[test]
    fn button_scheme_event_uses_built_in_profile() {
        let event = button_scheme_event(VAL_PRESS);
        assert_eq!(event.profile, DeviceProfile::built_in());
        assert_eq!(event.event.code, KEY_BUTTON_SCHEME);
        assert_eq!(event.event.value, VAL_PRESS);
    }

    fn features(
        bustype: u16,
        name: Option<&str>,
        emits_key: bool,
        emits_abs: bool,
    ) -> DeviceFeatures {
        // In the probe, `emits_abs` without position axes is possible (stray
        // vendor axis), but every existing test feature set models either a
        // digitizer (abs + position axes) or a pure key device.
        DeviceFeatures {
            name: name.map(String::from),
            bustype,
            vendor: 0,
            product: 0,
            version: 0,
            emits_key,
            emits_abs,
            has_position_axes: emits_abs,
        }
    }

    #[test]
    fn built_in_profile_maps_known_scancodes() {
        let profile = DeviceProfile::built_in();
        assert_eq!(profile.lookup(KEY_POWER), Some(KeyAction::Power));
        assert_eq!(profile.lookup(KEY_HOME), Some(KeyAction::Home));
        assert_eq!(profile.lookup(KEY_LIGHT), Some(KeyAction::Light));
        assert_eq!(
            profile.lookup(KEY_BACKWARD),
            Some(KeyAction::PageTurnScheme(LinearDir::Backward))
        );
        assert_eq!(
            profile.lookup(KEY_FORWARD),
            Some(KeyAction::PageTurnScheme(LinearDir::Forward))
        );
        assert_eq!(profile.lookup(PEN_ERASE), Some(KeyAction::Erase));
        assert_eq!(profile.lookup(PEN_HIGHLIGHT), Some(KeyAction::Highlight));
        assert_eq!(profile.lookup(SLEEP_COVER[0]), Some(KeyAction::SleepCover));
        assert_eq!(profile.lookup(SLEEP_COVER[1]), Some(KeyAction::SleepCover));
        assert_eq!(
            profile.lookup(KEY_ROTATE_DISPLAY),
            Some(KeyAction::RotateDisplay)
        );
        assert_eq!(
            profile.lookup(KEY_BUTTON_SCHEME),
            Some(KeyAction::ButtonScheme)
        );
    }

    #[test]
    fn built_in_profile_returns_none_for_unknown_scancodes() {
        assert_eq!(DeviceProfile::built_in().lookup(999), None);
    }

    #[test]
    fn remote_profile_maps_arrow_keys_to_raw_page_turn() {
        let profile = DeviceProfile::remote();
        assert_eq!(
            profile.lookup(KEY_UP),
            Some(KeyAction::PageTurnRaw(LinearDir::Backward))
        );
        assert_eq!(
            profile.lookup(KEY_DOWN),
            Some(KeyAction::PageTurnRaw(LinearDir::Forward))
        );
        // Other keys are not special; they fall through to Keyboard dispatch.
        assert_eq!(profile.lookup(KEY_SPACE), None);
    }

    #[test]
    fn keyboard_profile_has_no_special_scancodes() {
        let profile = DeviceProfile::keyboard();
        assert_eq!(profile.lookup(KEY_SPACE), None);
        assert_eq!(profile.lookup(KEY_POWER), None);
        assert!(profile.tracks_modifiers);
    }

    #[test]
    fn key_action_to_button_code_maps_simple_actions() {
        // rotation=0, Natural scheme, startup_rotation=0, dir=1.
        assert_eq!(
            KeyAction::Power.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Power
        );
        assert_eq!(
            KeyAction::Home.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Home
        );
        assert_eq!(
            KeyAction::Light.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Light
        );
        assert_eq!(
            KeyAction::Erase.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Erase
        );
        assert_eq!(
            KeyAction::Highlight.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Highlight
        );
    }

    #[test]
    fn key_action_page_turn_raw_bypasses_button_scheme() {
        // With the Inverted scheme, a scheme-aware forward would become
        // Backward; a raw forward stays Forward regardless.
        assert_eq!(
            KeyAction::PageTurnRaw(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Inverted,
                0,
                1
            ),
            ButtonCode::Forward
        );
        assert_eq!(
            KeyAction::PageTurnRaw(LinearDir::Backward).to_button_code(
                0,
                ButtonScheme::Inverted,
                0,
                1
            ),
            ButtonCode::Backward
        );
    }

    #[test]
    fn key_action_page_turn_scheme_respects_button_scheme() {
        // At rotation 0 with Natural, Forward stays Forward.
        assert_eq!(
            KeyAction::PageTurnScheme(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Natural,
                0,
                1
            ),
            ButtonCode::Forward
        );
        // With Inverted, Forward flips to Backward.
        assert_eq!(
            KeyAction::PageTurnScheme(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Inverted,
                0,
                1
            ),
            ButtonCode::Backward
        );
    }

    #[test]
    fn classify_device_identifies_kobo_remote() {
        let f = features(BUS_BLUETOOTH, Some("Kobo Remote"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
        // BlueZ/uhid variant name.
        let f = features(BUS_BLUETOOTH, Some("Kobo Remote Keyboard"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
    }

    #[test]
    fn classify_device_identifies_built_in_hardware() {
        // gpio-keys reports BUS_HOST.
        let f = features(BUS_HOST, Some("gpio-keys"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        // Touch digitizer reports EV_ABS.
        let f = features(BUS_I2C, Some("ft5x06_ts"), true, true);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        // Power key on a PMIC; real device reports BUS_HOST (not BUS_I2C).
        let f = features(BUS_HOST, Some("bd71828-pwrkey"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        // Elan Touchscreen (current Kobo touch digitizer).
        let f = features(BUS_I2C, Some("Elan Touchscreen"), true, true);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
    }

    #[test]
    fn classify_device_identifies_kobo_remote_regardless_of_bustype() {
        // The Kobo Remote is identified by name; bustype is not required.
        // Real devices report BUS_BLUETOOTH, but the name match is the
        // authoritative signal.
        let f = features(0x03, Some("Kobo Remote"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
    }

    #[test]
    fn classify_device_defaults_generic_keyboards_to_keyboard_profile() {
        // USB keyboard (bustype 0x03 = BUS_USB).
        let f = features(0x03, Some("Generic Keyboard"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
        // Bluetooth keyboard that is not a Kobo Remote.
        let f = features(BUS_BLUETOOTH, Some("Logitech K380"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
        // Device with no identity and no abs.
        let f = features(0, None, true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
    }

    /// Reproduces the bug where a hotplugged Bluetooth keyboard whose HID
    /// descriptor declares a stray `ABS_MISC` axis was classified as built-in
    /// hardware: its keys went out as `ButtonCode::Raw` (ignored by views)
    /// and its 'h' key (evdev 35, `SLEEP_COVER[1]`) opened the sleep cover,
    /// locking the screen.
    #[test]
    fn classify_device_keeps_keyboard_with_stray_abs_axis_generic() {
        // Probed from a real uhid Bluetooth keyboard ("Targus Folding
        // Ergonomic Bluetooth Keyboard"): BUS_BLUETOOTH, EV_KEY + EV_ABS
        // declared, but the ABS bitmap holds only bit 40 (ABS_MISC) — no
        // position axes. Samsung vendor/product from EVIOCGID.
        let f = DeviceFeatures {
            name: Some("Targus Folding Ergonomic Bluetooth Keyboard".to_string()),
            bustype: BUS_BLUETOOTH,
            vendor: 0x04e8,
            product: 0x7021,
            version: 0x011b,
            emits_key: true,
            emits_abs: true,
            has_position_axes: false,
        };
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());

        // A USB consumer-control node can also declare stray ABS axes (e.g.
        // volume/scan codes) without being a digitizer.
        let f = DeviceFeatures {
            name: Some("Framework Laptop 16 Keyboard Module - ANSI System Control".to_string()),
            bustype: 0x03,
            vendor: 0x32ac,
            product: 0x0012,
            version: 0x0111,
            emits_key: true,
            emits_abs: true,
            has_position_axes: false,
        };
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
    }

    /// The 'h' key must never act as a sleep cover on keyboard-profile
    /// devices: evdev 35 is both `KEY_H` and `SLEEP_COVER[1]`, and the
    /// built-in-only mapping leaked into hotplugged keyboards before the
    /// classifier was fixed.
    #[test]
    fn keyboard_profile_does_not_map_the_h_key_to_a_sleep_cover() {
        assert_eq!(DeviceProfile::keyboard().lookup(SLEEP_COVER[0]), None);
        assert_eq!(DeviceProfile::keyboard().lookup(SLEEP_COVER[1]), None);
        // The built-in profile keeps its cover mapping.
        assert_eq!(
            DeviceProfile::built_in().lookup(SLEEP_COVER[1]),
            Some(KeyAction::SleepCover)
        );
    }

    /// `has_abs_axis` must decode the ABS bitmap exactly as the kernel
    /// reports it: bit 40 (ABS_MISC) in byte 5 for the uhid keyboard, and
    /// position axes in byte 0 (single-touch) or byte 6 (multi-touch) for
    /// digitizers.
    #[test]
    fn has_abs_axis_decodes_real_device_bitmaps() {
        // Targus BT keyboard: ABS_MISC only.
        let targus = [0u8, 0, 0, 0, 0, 0x01, 0, 0];
        assert!(!has_abs_axis(&targus, ABS_X));
        assert!(!has_abs_axis(&targus, ABS_MT_POSITION_X));
        assert!(has_abs_axis(&targus, 40));

        // Single-touch digitizer (e.g. ft5x06_ts): ABS_X/ABS_Y.
        let single_touch = [0x03u8, 0, 0, 0, 0, 0, 0, 0];
        assert!(has_abs_axis(&single_touch, ABS_X));
        assert!(has_abs_axis(&single_touch, ABS_Y));

        // Multi-touch protocol A/B digitizer: ABS_MT_POSITION_X (53) and
        // ABS_MT_POSITION_Y (54) in byte 6.
        let multi_touch = [0u8, 0, 0, 0, 0, 0, 0x60, 0];
        assert!(has_abs_axis(&multi_touch, ABS_MT_POSITION_X));
        assert!(has_abs_axis(&multi_touch, 54));
    }

    #[test]
    fn is_built_in_name_matches_known_devices() {
        assert!(is_built_in_name("Elan Touchscreen"));
        assert!(is_built_in_name("bd71828-pwrkey"));
        assert!(is_built_in_name("gpio-keys"));
    }

    #[test]
    fn is_built_in_name_rejects_unknown_devices() {
        assert!(!is_built_in_name("ft5x06_ts"));
        assert!(!is_built_in_name("Wacom Digitizer"));
        assert!(!is_built_in_name("kx122-accel"));
        assert!(!is_built_in_name(""));
    }

    /// Reproduces the bug where `bd71828-pwrkey` and `gpio-keys` share the
    /// same `EVIOCGID` tuple on current Kobo hardware: the deduplicator
    /// treated one as a duplicate of the other and dropped the power key.
    /// With the name included in the identity, both devices are kept.
    #[test]
    fn identity_distinguishes_pwrkey_from_gpio_keys() {
        let shared_id = (BUS_HOST, 0x1, 0x1, 0x100);
        let pwrkey = DeviceFeatures {
            name: Some("bd71828-pwrkey".to_string()),
            bustype: shared_id.0,
            vendor: shared_id.1,
            product: shared_id.2,
            version: shared_id.3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let gpio_keys = DeviceFeatures {
            name: Some("gpio-keys".to_string()),
            bustype: shared_id.0,
            vendor: shared_id.1,
            product: shared_id.2,
            version: shared_id.3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        assert_ne!(pwrkey.identity(), gpio_keys.identity());
    }

    #[test]
    fn identity_returns_none_for_anonymous_zero_id() {
        let f = DeviceFeatures {
            name: None,
            bustype: 0,
            vendor: 0,
            product: 0,
            version: 0,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        assert_eq!(f.identity(), None);
    }

    #[test]
    fn identity_includes_name_in_output() {
        let f = DeviceFeatures {
            name: Some("gpio-keys".to_string()),
            bustype: BUS_HOST,
            vendor: 0x1,
            product: 0x1,
            version: 0x100,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let id = f.identity().expect("named device has an identity");
        assert!(id.contains("gpio-keys"), "identity must include name: {id}");
    }

    #[test]
    fn is_kobo_remote_name_matches_kobo_and_bluez_variants() {
        assert!(is_kobo_remote_name("Kobo Remote"));
        assert!(is_kobo_remote_name("Kobo Remote Keyboard"));
    }

    #[test]
    fn is_kobo_remote_name_rejects_unrelated_devices() {
        assert!(!is_kobo_remote_name("Azulle | Lynk Remote"));
        assert!(!is_kobo_remote_name("Generic Keyboard"));
        assert!(!is_kobo_remote_name(""));
    }

    #[test]
    fn key_event_serializes_with_camel_case() {
        let mut modifiers = Modifiers::default();
        modifiers.set_depressed(Modifier::Ctrl, true);
        let key_event = KeyEvent {
            keycode: KEY_SPACE as u32,
            modifiers,
        };
        let json = serde_json::to_string(&key_event).unwrap();
        assert!(json.contains("\"keycode\""));
        assert!(json.contains("\"modifiers\""));
        assert!(json.contains("\"modsDepressed\""));
    }

    #[test]
    fn modifiers_serialize_with_camel_case() {
        let mods = Modifiers {
            mods_depressed: 1,
            mods_latched: 2,
            mods_locked: 4,
            group: 1,
        };
        let json = serde_json::to_string(&mods).unwrap();
        assert!(json.contains("\"modsDepressed\""));
        assert!(json.contains("\"modsLatched\""));
        assert!(json.contains("\"modsLocked\""));
        assert!(json.contains("\"group\""));
    }
}
