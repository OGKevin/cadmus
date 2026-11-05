use crate::geom::LinearDir;
use crate::settings::ButtonScheme;

use super::codes::{
    KEY_BACKWARD, KEY_BUTTON_SCHEME, KEY_FORWARD, KEY_HOME, KEY_LIGHT, KEY_POWER,
    KEY_ROTATE_DISPLAY, PEN_ERASE, PEN_HIGHLIGHT, SLEEP_COVER,
};
#[cfg(any(target_os = "linux", test))]
use super::codes::{KEY_DOWN, KEY_UP};
use super::event::ButtonCode;

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
pub(super) struct DeviceProfile {
    key_map: &'static [(u16, KeyAction)],
    pub(super) tracks_modifiers: bool,
}

impl DeviceProfile {
    /// Profile for built-in device hardware (touch digitizer, gpio-keys,
    /// power key): maps the standard built-in scancodes to their functions,
    /// including the synthetic control codes injected via
    /// [`display_rotate_event`] / [`button_scheme_event`]. Unknown scancodes
    /// become [`ButtonCode::Raw`].
    pub(super) const fn built_in() -> Self {
        Self {
            key_map: BUILT_IN_KEY_MAP,
            tracks_modifiers: false,
        }
    }

    /// Profile for the Kobo Remote Bluetooth keyboard: up/down arrows map to
    /// page-turn buttons (bypassing the button-scheme, since the remote is
    /// orientation-agnostic); all other keys are forwarded as
    /// [`ButtonCode::Keyboard`] with modifier tracking.
    #[cfg(any(target_os = "linux", test))]
    pub(super) const fn remote() -> Self {
        Self {
            key_map: REMOTE_KEY_MAP,
            tracks_modifiers: true,
        }
    }

    /// Profile for a generic hotplugged keyboard: no special scancodes; all
    /// keys are forwarded as [`ButtonCode::Keyboard`] with modifier tracking.
    #[cfg(any(target_os = "linux", test))]
    pub(super) const fn keyboard() -> Self {
        Self {
            key_map: &[],
            tracks_modifiers: true,
        }
    }

    /// Returns the [`KeyAction`] mapped to `code`, if any.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub(super) fn lookup(self, code: u16) -> Option<KeyAction> {
        self.key_map
            .iter()
            .find_map(|(c, action)| (*c == code).then_some(*action))
    }
}

/// A semantic action mapped to a specific evdev scancode within a
/// [`DeviceProfile`].
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(super) enum KeyAction {
    Power,
    Home,
    Light,
    /// Page-turn that respects the button-scheme (rotation-based inversion),
    /// used by built-in page-turn buttons.
    PageTurnScheme(LinearDir),
    /// Page-turn that bypasses the button-scheme, used by orientation-agnostic
    /// remotes.
    #[cfg(any(target_os = "linux", test))]
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
    /// inversion); `PageTurnRaw` bypasses it. Control actions
    /// ([`SleepCover`], [`ButtonScheme`], [`RotateDisplay`]) are handled by
    /// their own match arms in [`parse_device_events`] before this is called,
    /// so reaching them here is a logic error.
    ///
    /// [`PageTurnScheme`]: KeyAction::PageTurnScheme
    /// [`SleepCover`]: KeyAction::SleepCover
    /// [`ButtonScheme`]: KeyAction::ButtonScheme
    /// [`RotateDisplay`]: KeyAction::RotateDisplay
    #[cfg_attr(feature = "tracing", tracing::instrument)]
    pub(super) fn to_button_code(
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
            #[cfg(any(target_os = "linux", test))]
            KeyAction::PageTurnRaw(LinearDir::Forward) => ButtonCode::Forward,
            #[cfg(any(target_os = "linux", test))]
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
#[cfg(any(target_os = "linux", test))]
const REMOTE_KEY_MAP: &[(u16, KeyAction)] = &[
    (KEY_UP, KeyAction::PageTurnRaw(LinearDir::Backward)),
    (KEY_DOWN, KeyAction::PageTurnRaw(LinearDir::Forward)),
];

#[cfg_attr(feature = "tracing", tracing::instrument)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::codes::KEY_SPACE;

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
    /// With the Inverted scheme, a scheme-aware forward would become
    /// Backward; a raw forward stays Forward regardless.
    #[test]
    fn key_action_page_turn_raw_bypasses_button_scheme() {
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
        assert_eq!(
            KeyAction::PageTurnScheme(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Natural,
                0,
                1
            ),
            ButtonCode::Forward
        );
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
    /// The 'h' key must never act as a sleep cover on keyboard-profile
    /// devices: evdev 35 is both `KEY_H` and `SLEEP_COVER[1]`, and the
    /// built-in-only mapping leaked into hotplugged keyboards before the
    /// classifier was fixed.
    #[test]
    fn keyboard_profile_does_not_map_the_h_key_to_a_sleep_cover() {
        assert_eq!(DeviceProfile::keyboard().lookup(SLEEP_COVER[0]), None);
        assert_eq!(DeviceProfile::keyboard().lookup(SLEEP_COVER[1]), None);
        assert_eq!(
            DeviceProfile::built_in().lookup(SLEEP_COVER[1]),
            Some(KeyAction::SleepCover)
        );
    }
}
