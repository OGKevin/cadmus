use serde::Serialize;

use super::codes::{
    KEY_CAPSLOCK, KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_NUMLOCK,
    KEY_RIGHTALT, KEY_RIGHTCTRL, KEY_RIGHTMETA, KEY_RIGHTSHIFT, VAL_PRESS, VAL_RELEASE, VAL_REPEAT,
};

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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
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
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
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
/// The `ButtonStatus` carried alongside this in [`crate::input::ButtonCode::Keyboard`]
/// provides the press/release/repeat state, and the `time` field of
/// [`crate::input::DeviceEvent::Button`] provides the timestamp — matching the split
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::codes::{KEY_HOME, KEY_POWER, KEY_SPACE};

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
