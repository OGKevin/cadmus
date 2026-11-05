//! Evdev event types, event codes and values used by the input pipeline.
//!
//! Everything here that hardware can produce is copied verbatim from the Linux
//! UAPI header `include/uapi/linux/input-event-codes.h` (installed as
//! `/usr/include/linux/input-event-codes.h`, also reachable through
//! `<linux/input.h>`). Names are kept identical to the header so a constant can
//! be checked against it by name; the semantics of each event type are
//! described in `Documentation/input/event-codes.rst` in the kernel tree.
//!
//! Two groups are not from that header:
//!
//! - The `MSC_RAW_GSENSOR_*` values are Kobo-specific. The `kx122-accel` driver
//!   reports screen orientation as raw `EV_MSC` / `MSC_RAW` values, whose
//!   meaning is defined by that kernel driver, not by the input subsystem.
//!   Confirm them on a device by reading the accelerometer node
//!   (`evtest /dev/input/event2`) while rotating it.
//! - `KEY_ROTATE_DISPLAY` and `KEY_BUTTON_SCHEME` are synthetic codes this
//!   design injects itself; no kernel ever emits them.
//!
//! `KEY_*` code numbers are the evdev ones, which Wayland also uses directly.
//! XKB keycodes are `code + 8`.

pub(super) const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub(super) const EV_ABS: u16 = 0x03;
pub(super) const EV_MSC: u16 = 0x04;

pub(super) const ABS_MT_TRACKING_ID: u16 = 0x39;
pub(super) const ABS_MT_POSITION_X: u16 = 0x35;
pub(super) const ABS_MT_POSITION_Y: u16 = 0x36;
pub(super) const ABS_MT_PRESSURE: u16 = 0x3a;
pub(super) const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub(super) const ABS_X: u16 = 0x00;
pub(super) const ABS_Y: u16 = 0x01;
pub(super) const ABS_PRESSURE: u16 = 0x18;
pub(super) const MSC_RAW: u16 = 0x03;
pub(super) const SYN_REPORT: u16 = 0x00;
pub(super) const SYN_DROPPED: u16 = 0x03;

pub(super) const MSC_RAW_GSENSOR_PORTRAIT_DOWN: i32 = 0x17;
pub(super) const MSC_RAW_GSENSOR_PORTRAIT_UP: i32 = 0x18;
pub(super) const MSC_RAW_GSENSOR_LANDSCAPE_RIGHT: i32 = 0x19;
pub(super) const MSC_RAW_GSENSOR_LANDSCAPE_LEFT: i32 = 0x1a;

/// Gsensor values in clockwise order; the index of a raw sensor value in
/// this array matches the Forma's rotation value.
pub(super) const GYROSCOPE_ROTATIONS: [i32; 4] = [
    MSC_RAW_GSENSOR_LANDSCAPE_LEFT,
    MSC_RAW_GSENSOR_PORTRAIT_UP,
    MSC_RAW_GSENSOR_LANDSCAPE_RIGHT,
    MSC_RAW_GSENSOR_PORTRAIT_DOWN,
];

pub const VAL_RELEASE: i32 = 0;
pub const VAL_PRESS: i32 = 1;
pub(super) const VAL_REPEAT: i32 = 2;

pub(super) const KEY_POWER: u16 = 116;
pub(super) const KEY_HOME: u16 = 102;
pub(super) const KEY_LIGHT: u16 = 90;
pub(super) const KEY_BACKWARD: u16 = 193;
pub(super) const KEY_FORWARD: u16 = 194;
#[cfg(test)]
pub(super) const KEY_SPACE: u16 = 57;
#[cfg(any(target_os = "linux", test))]
pub(super) const KEY_UP: u16 = 103;
#[cfg(any(target_os = "linux", test))]
pub(super) const KEY_DOWN: u16 = 108;
pub(super) const KEY_LEFTCTRL: u16 = 29;
pub(super) const KEY_RIGHTCTRL: u16 = 97;
pub(super) const KEY_LEFTSHIFT: u16 = 42;
pub(super) const KEY_RIGHTSHIFT: u16 = 54;
pub(super) const KEY_LEFTALT: u16 = 56;
pub(super) const KEY_RIGHTALT: u16 = 100;
pub(super) const KEY_LEFTMETA: u16 = 125;
pub(super) const KEY_RIGHTMETA: u16 = 126;
pub(super) const KEY_CAPSLOCK: u16 = 58;
pub(super) const KEY_NUMLOCK: u16 = 69;
pub(super) const PEN_ERASE: u16 = 331;
pub(super) const PEN_HIGHLIGHT: u16 = 332;
pub(super) const SLEEP_COVER: [u16; 2] = [59, 35];
pub(super) const BTN_TOUCH: u16 = 330;

/// Synthetic key code never produced by hardware: injected to support
/// software toggles within this design, via
/// [`display_rotate_event`](crate::input::display_rotate_event).
pub const KEY_ROTATE_DISPLAY: u16 = 0xffff;
/// Synthetic key code never produced by hardware: injected to support
/// software toggles within this design, via
/// [`button_scheme_event`](crate::input::button_scheme_event).
pub(super) const KEY_BUTTON_SCHEME: u16 = 0xfffe;
