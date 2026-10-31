//! Translates physical keyboard events into terminal input bytes.
//!
//! [`crate::input`] delivers key presses from hotplugged keyboards as evdev
//! scancodes wrapped in `ButtonCode::Keyboard` (see [`crate::input::KeyEvent`]).
//! This module maps those scancodes, combined with the modifier snapshot, to
//! the byte sequences a terminal expects.

use crate::geom::Dir;
use crate::input::{ButtonStatus, KeyEvent, Modifier};

// Evdev keycodes not exposed as constants by `crate::input`.
const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_ENTER: u16 = 28;
const KEY_KP7: u16 = 71;
const KEY_KP8: u16 = 72;
const KEY_KP4: u16 = 75;
const KEY_KP6: u16 = 77;
const KEY_KP1: u16 = 79;
const KEY_KP2: u16 = 80;
const KEY_KP3: u16 = 81;
const KEY_RIGHT: u16 = 106;
const KEY_LEFT: u16 = 105;
const KEY_KPENTER: u16 = 96;
const KEY_HOME: u16 = 102;
const KEY_PAGEUP: u16 = 104;
const KEY_END: u16 = 107;
const KEY_PAGEDOWN: u16 = 109;
const KEY_INSERT: u16 = 110;
const KEY_DELETE: u16 = 111;
const KEY_UP: u16 = 103;
const KEY_DOWN: u16 = 108;
const KEY_SPACE: u16 = 57;
const KEY_SCROLLLOCK: u16 = 70;

/// Arrow-key sequence in CSI or SS3 form, matching the terminal's DECCKM
/// (application cursor keys) mode.
pub(super) fn cursor_key_sequence(dir: Dir, application_cursor: bool) -> &'static [u8] {
    match (dir, application_cursor) {
        (Dir::North, false) => b"\x1b[A",
        (Dir::South, false) => b"\x1b[B",
        (Dir::East, false) => b"\x1b[C",
        (Dir::West, false) => b"\x1b[D",
        (Dir::North, true) => b"\x1bOA",
        (Dir::South, true) => b"\x1bOB",
        (Dir::East, true) => b"\x1bOC",
        (Dir::West, true) => b"\x1bOD",
    }
}

/// Home/End sequence in CSI or SS3 form; DECCKM switches these like arrows.
fn home_end_sequence(csi: &'static [u8], ss3: &'static [u8], application_cursor: bool) -> Vec<u8> {
    (if application_cursor { ss3 } else { csi }).to_vec()
}

/// Keys that never produce terminal input on their own. This is
/// [`Modifier::from_evdev_code`] plus Scroll Lock, which the input pipeline
/// does not model as a modifier.
fn is_modifier(code: u16) -> bool {
    Modifier::from_evdev_code(code).is_some() || code == KEY_SCROLLLOCK
}

/// Bytes sent for a key event, or `None` when the transition produces no
/// terminal input (key release, modifier press, or unsupported key).
///
/// Key repeats produce the same bytes as the initial press. Arrow keys follow
/// the terminal's DECCKM mode (`application_cursor`), so full-screen programs
/// receive the sequences they expect.
pub fn bytes(key: &KeyEvent, status: ButtonStatus, application_cursor: bool) -> Option<Vec<u8>> {
    if status == ButtonStatus::Released {
        return None;
    }

    let code = key.keycode as u16;
    let mods = key.modifiers;
    if is_modifier(code) || mods.has(Modifier::Super) {
        return None;
    }

    // Ctrl alone produces control characters; Ctrl+Alt is reserved for
    // system shortcuts, so it produces no terminal input.
    if mods.ctrl() && mods.alt() {
        return None;
    }
    if mods.ctrl() {
        let ch = base_char(code)?;
        if !ch.is_ascii_alphabetic() && !(b'@'..=b'_').contains(&ch) {
            return None;
        }
        return Some(vec![(ch.to_ascii_uppercase()) & 0x1f]);
    }

    let shifted = mods.shift();
    match code {
        KEY_ESC => Some(b"\x1b".to_vec()),
        KEY_BACKSPACE => Some(b"\x7f".to_vec()),
        KEY_TAB => Some(b"\t".to_vec()),
        KEY_ENTER | KEY_KPENTER => Some(b"\r".to_vec()),
        KEY_UP | KEY_KP8 => Some(cursor_key_sequence(Dir::North, application_cursor).to_vec()),
        KEY_DOWN | KEY_KP2 => Some(cursor_key_sequence(Dir::South, application_cursor).to_vec()),
        KEY_RIGHT | KEY_KP6 => Some(cursor_key_sequence(Dir::East, application_cursor).to_vec()),
        KEY_LEFT | KEY_KP4 => Some(cursor_key_sequence(Dir::West, application_cursor).to_vec()),
        KEY_HOME | KEY_KP7 => Some(home_end_sequence(b"\x1b[H", b"\x1bOH", application_cursor)),
        KEY_END | KEY_KP1 => Some(home_end_sequence(b"\x1b[F", b"\x1bOF", application_cursor)),
        KEY_INSERT => Some(b"\x1b[2~".to_vec()),
        KEY_DELETE | KEY_KP3 => Some(b"\x1b[3~".to_vec()),
        KEY_PAGEUP => Some(b"\x1b[5~".to_vec()),
        KEY_PAGEDOWN => Some(b"\x1b[6~".to_vec()),
        _ => pair(code).map(|(lo, hi)| vec![if shifted { hi } else { lo }]),
    }
}

/// Unshifted ASCII produced by `code` (modifiers ignored).
fn base_char(code: u16) -> Option<u8> {
    pair(code).map(|(lo, _)| lo)
}

/// Default unshifted/shifted ASCII pair for a keycode, if printable.
fn pair(code: u16) -> Option<(u8, u8)> {
    Some(match code {
        KEY_SPACE => (b' ', b' '),
        KEY_1 => (b'1', b'!'),
        3 => (b'2', b'@'),
        4 => (b'3', b'#'),
        5 => (b'4', b'$'),
        6 => (b'5', b'%'),
        7 => (b'6', b'^'),
        8 => (b'7', b'&'),
        9 => (b'8', b'*'),
        10 => (b'9', b'('),
        11 => (b'0', b')'),
        12 => (b'-', b'_'),
        13 => (b'=', b'+'),
        16..=25 => ROW_LETTERS[code as usize - 16], // Q W E R T Y U I O P
        26 => (b'[', b'{'),
        27 => (b']', b'}'),
        39 => (b';', b':'),
        40 => (b'\'', b'"'),
        41 => (b'`', b'~'),
        43 => (b'\\', b'|'),
        30..=38 => ROW_LETTERS[code as usize - 30 + 10], // A S D F G H J K L
        44..=50 => ROW_LETTERS[code as usize - 44 + 19], // Z X C V B N M
        51 => (b',', b'<'),
        52 => (b'.', b'>'),
        53 => (b'/', b'?'),
        _ => return None,
    })
}

/// Unshifted/shifted ASCII pairs for the letter keys, in evdev code order:
/// Q W E R T Y U I O P (16–25), A S D F G H J K L (30–38), Z X C V B N M (44–50).
/// Evdev codes follow the keyboard rows, not the alphabet, so the sequence
/// cannot be derived arithmetically.
const ROW_LETTERS: [(u8, u8); 26] = [
    (b'q', b'Q'),
    (b'w', b'W'),
    (b'e', b'E'),
    (b'r', b'R'),
    (b't', b'T'),
    (b'y', b'Y'),
    (b'u', b'U'),
    (b'i', b'I'),
    (b'o', b'O'),
    (b'p', b'P'),
    (b'a', b'A'),
    (b's', b'S'),
    (b'd', b'D'),
    (b'f', b'F'),
    (b'g', b'G'),
    (b'h', b'H'),
    (b'j', b'J'),
    (b'k', b'K'),
    (b'l', b'L'),
    (b'z', b'Z'),
    (b'x', b'X'),
    (b'c', b'C'),
    (b'v', b'V'),
    (b'b', b'B'),
    (b'n', b'N'),
    (b'm', b'M'),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ButtonCode, DeviceEvent, Modifiers};

    fn key(code: u16, mods: Modifiers) -> KeyEvent {
        KeyEvent {
            keycode: code as u32,
            modifiers: mods,
        }
    }

    fn pressed(code: u16, mods: Modifiers) -> Option<Vec<u8>> {
        bytes(&key(code, mods), ButtonStatus::Pressed, false)
    }

    #[test]
    fn letters_follow_the_evdev_layout() {
        assert_eq!(
            pressed(16, Modifiers::default()).as_deref(),
            Some(b"q".as_slice())
        );
        assert_eq!(
            pressed(30, Modifiers::default()).as_deref(),
            Some(b"a".as_slice())
        );
        assert_eq!(
            pressed(44, Modifiers::default()).as_deref(),
            Some(b"z".as_slice())
        );
        assert_eq!(
            pressed(25, Modifiers::default()).as_deref(),
            Some(b"p".as_slice())
        );
        assert_eq!(
            pressed(38, Modifiers::default()).as_deref(),
            Some(b"l".as_slice())
        );
        assert_eq!(
            pressed(50, Modifiers::default()).as_deref(),
            Some(b"m".as_slice())
        );
    }

    #[test]
    fn every_letter_key_maps_to_its_evdev_code() {
        // Q W E R T Y U I O P (16–25), A S D F G H J K L (30–38),
        // Z X C V B N M (44–50).
        let expected = "qwertyuiopasdfghjklzxcvbnm";
        let codes = (16..=25)
            .chain(30..=38)
            .chain(44..=50)
            .zip(expected.bytes());
        for (code, ch) in codes {
            assert_eq!(
                pressed(code, Modifiers::default()),
                Some(vec![ch]),
                "evdev code {code}"
            );
        }
    }

    #[test]
    fn shift_selects_the_shifted_pair() {
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Shift, true);
        assert_eq!(pressed(17, mods).as_deref(), Some(b"W".as_slice()));
        assert_eq!(pressed(3, mods).as_deref(), Some(b"@".as_slice()));
        assert_eq!(pressed(12, mods).as_deref(), Some(b"_".as_slice()));
        assert_eq!(pressed(41, mods).as_deref(), Some(b"~".as_slice()));
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Ctrl, true);
        assert_eq!(pressed(30, mods).as_deref(), Some(b"\x01".as_slice())); // Ctrl-A
        assert_eq!(pressed(33, mods).as_deref(), Some(b"\x06".as_slice())); // Ctrl-F
        assert_eq!(pressed(46, mods).as_deref(), Some(b"\x03".as_slice())); // Ctrl-C
    }

    #[test]
    fn special_keys_send_escape_sequences() {
        let mods = Modifiers::default();
        assert_eq!(pressed(1, mods).as_deref(), Some(b"\x1b".as_slice()));
        assert_eq!(pressed(14, mods).as_deref(), Some(b"\x7f".as_slice()));
        assert_eq!(pressed(28, mods).as_deref(), Some(b"\r".as_slice()));
        assert_eq!(pressed(15, mods).as_deref(), Some(b"\t".as_slice()));
        assert_eq!(pressed(103, mods).as_deref(), Some(b"\x1b[A".as_slice()));
        assert_eq!(pressed(111, mods).as_deref(), Some(b"\x1b[3~".as_slice()));
    }

    #[test]
    fn modifiers_and_releases_produce_nothing() {
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Shift, true);
        assert_eq!(pressed(42, mods), None);
        assert_eq!(
            bytes(
                &key(16, Modifiers::default()),
                ButtonStatus::Released,
                false
            ),
            None
        );
    }

    #[test]
    fn modifier_snapshot_is_honored() {
        // Ctrl+Alt is reserved for system shortcuts and produces no input.
        let mut mods = Modifiers::default();
        mods.set_depressed(Modifier::Ctrl, true);
        mods.set_depressed(Modifier::Alt, true);
        assert_eq!(pressed(30, mods), None);
    }

    #[test]
    fn repeats_replay_the_press_bytes() {
        let mods = Modifiers::default();
        // Holding a key makes evdev emit value 2 repeats; the terminal must
        // keep receiving the pressed sequence.
        assert_eq!(
            bytes(&key(30, mods), ButtonStatus::Repeated, false).as_deref(),
            Some(b"a".as_slice())
        );
        assert_eq!(
            bytes(&key(103, mods), ButtonStatus::Repeated, false).as_deref(),
            Some(b"\x1b[A".as_slice())
        );
        assert_eq!(bytes(&key(16, mods), ButtonStatus::Released, false), None);
    }

    #[test]
    fn numpad_keys_send_their_navigation_sequences() {
        let mods = Modifiers::default();
        // Real evdev codes: KP7=71, KP8=72, KP4=75, KP6=77, KP1=79, KP2=80,
        // KP3=81, KPENTER=96.
        assert_eq!(pressed(71, mods).as_deref(), Some(b"\x1b[H".as_slice()));
        assert_eq!(pressed(72, mods).as_deref(), Some(b"\x1b[A".as_slice()));
        assert_eq!(pressed(75, mods).as_deref(), Some(b"\x1b[D".as_slice()));
        assert_eq!(pressed(77, mods).as_deref(), Some(b"\x1b[C".as_slice()));
        assert_eq!(pressed(79, mods).as_deref(), Some(b"\x1b[F".as_slice()));
        assert_eq!(pressed(80, mods).as_deref(), Some(b"\x1b[B".as_slice()));
        assert_eq!(pressed(81, mods).as_deref(), Some(b"\x1b[3~".as_slice()));
        assert_eq!(pressed(96, mods).as_deref(), Some(b"\r".as_slice()));
        // The old constants pointed at unrelated keys: 83=KPDOT, 84=KPJPCOMMA,
        // 85=ZENKAKUHANKAKU, 86=102ND, 89=RO — none produce arrows.
        assert_eq!(pressed(83, mods), None);
        assert_eq!(pressed(89, mods), None);
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        let mods = Modifiers::default();
        assert_eq!(
            bytes(&key(103, mods), ButtonStatus::Pressed, true).as_deref(),
            Some(b"\x1bOA".as_slice())
        );
        assert_eq!(
            bytes(&key(108, mods), ButtonStatus::Pressed, true).as_deref(),
            Some(b"\x1bOB".as_slice())
        );
        assert_eq!(
            bytes(&key(105, mods), ButtonStatus::Pressed, true).as_deref(),
            Some(b"\x1bOD".as_slice())
        );
        // Home/End switch to their SS3 forms under DECCKM, like arrows.
        assert_eq!(
            bytes(&key(102, mods), ButtonStatus::Pressed, true).as_deref(),
            Some(b"\x1bOH".as_slice())
        );
        assert_eq!(
            bytes(&key(107, mods), ButtonStatus::Pressed, true).as_deref(),
            Some(b"\x1bOF".as_slice())
        );
    }

    #[test]
    fn device_event_button_keyboard_roundtrip() {
        let evt = DeviceEvent::Button {
            time: 0.0,
            code: ButtonCode::Keyboard(key(45, Modifiers::default())),
            status: ButtonStatus::Pressed,
        };
        let DeviceEvent::Button { code, status, .. } = evt else {
            panic!("expected button event");
        };
        let ButtonCode::Keyboard(key) = code else {
            panic!("expected keyboard code");
        };
        assert_eq!(bytes(&key, status, false).as_deref(), Some(b"x".as_slice()));
    }
}
