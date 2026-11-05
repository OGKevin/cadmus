use vt100::{Callbacks, Parser, Screen};

/// A reply the terminal owes the program in answer to a device query that
/// arrived in its output stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeviceResponse {
    /// Primary device attributes: VT220 level 2 with ANSI color.
    PrimaryAttributes,
    /// Cursor position report (DSR-CPR), in 1-based coordinates.
    CursorPosition { row: u16, col: u16 },
}

impl DeviceResponse {
    pub(super) fn escape_sequence(&self) -> Vec<u8> {
        match self {
            Self::PrimaryAttributes => b"\x1b[?62;22c".to_vec(),
            Self::CursorPosition { row, col } => {
                format!("\x1b[{};{}R", row + 1, col + 1).into_bytes()
            }
        }
    }
}

/// Collects device queries seen in the output stream so the session can answer
/// them on the PTY input; shells like fish stall at startup until the primary
/// device attribute query is answered.
#[derive(Default)]
struct QueryCallbacks {
    pending: Vec<DeviceResponse>,
}

impl Callbacks for QueryCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // Per DEC STD 070, only an omitted or zero parameter makes DA a
        // request. Replies from other terminals carry a private marker
        // (`?`, `>`, `=`) in the intermediate and must not be answered.
        let request_param = params.first().and_then(|param| param.first());
        match (i1, c) {
            (None, 'c') if request_param.is_none_or(|&p| p == 0) => {
                self.pending.push(DeviceResponse::PrimaryAttributes);
            }
            (None, 'n') if request_param == Some(&6) => {
                let (row, col) = screen.cursor_position();
                self.pending
                    .push(DeviceResponse::CursorPosition { row, col });
            }
            _ => {}
        }
    }
}

pub(super) struct Emulator {
    parser: Parser<QueryCallbacks>,
}

impl Emulator {
    pub(super) fn new(rows: u16, cols: u16) -> Self {
        Emulator {
            parser: Parser::new_with_callbacks(rows, cols, 1000, QueryCallbacks::default()),
        }
    }

    /// Drains the device queries collected by the last feeds; each returned
    /// response must be written to the PTY input exactly once.
    pub(super) fn take_device_responses(&mut self) -> Vec<DeviceResponse> {
        std::mem::take(&mut self.parser.callbacks_mut().pending)
    }

    pub(super) fn feed(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub(super) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub(super) fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub(super) fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub(super) fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub(super) fn set_scrollback(&mut self, rows: usize) -> bool {
        let previous = self.scrollback();
        self.parser.screen_mut().set_scrollback(rows);
        self.scrollback() != previous
    }

    pub(super) fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceResponse, Emulator};

    fn responses_after_feed(emulator: &mut Emulator, data: &[u8]) -> Vec<DeviceResponse> {
        emulator.feed(data);
        emulator.take_device_responses()
    }

    #[test]
    fn primary_device_attribute_query_is_collected() {
        let mut emulator = Emulator::new(2, 10);
        // fish sends the explicit-zero form; the omitted-parameter form is a
        // request per DEC STD 070 as well.
        emulator.feed(b"\x1b[0c\x1b[c");
        assert_eq!(
            emulator.take_device_responses(),
            vec![
                DeviceResponse::PrimaryAttributes,
                DeviceResponse::PrimaryAttributes,
            ]
        );
        // Collected queries are drained, not repeated.
        assert!(emulator.take_device_responses().is_empty());
    }

    #[test]
    fn primary_device_attribute_reply_is_not_reanswered() {
        let mut emulator = Emulator::new(2, 10);
        // xterm's own DA1 reply carries the `?` intermediate; answering it back
        // would loop replies between two terminals.
        assert!(responses_after_feed(&mut emulator, b"\x1b[?62;1;2c").is_empty());
        assert!(responses_after_feed(&mut emulator, b"\x1b[>0;276;0c").is_empty());
        assert!(responses_after_feed(&mut emulator, b"\x1b[=c").is_empty());
    }

    #[test]
    fn nonzero_primary_device_attribute_parameter_is_not_a_request() {
        let mut emulator = Emulator::new(2, 10);
        assert!(responses_after_feed(&mut emulator, b"\x1b[1c").is_empty());
    }

    #[test]
    fn cursor_position_request_is_answered_from_the_live_screen() {
        let mut emulator = Emulator::new(5, 20);
        emulator.feed(b"\x1b[3;7H");
        assert_eq!(
            responses_after_feed(&mut emulator, b"\x1b[6n"),
            vec![DeviceResponse::CursorPosition { row: 2, col: 6 }]
        );
        assert_eq!(
            DeviceResponse::CursorPosition { row: 2, col: 6 }.escape_sequence(),
            b"\x1b[3;7R"
        );
    }

    #[test]
    fn cursor_position_request_ignores_scrollback_view() {
        let mut emulator = Emulator::new(2, 10);
        emulator.feed(b"one\r\ntwo\r\nthree\r\nfour");
        assert!(emulator.set_scrollback(1));
        assert_eq!(
            responses_after_feed(&mut emulator, b"\x1b[6n"),
            vec![DeviceResponse::CursorPosition { row: 1, col: 4 }]
        );
    }

    #[test]
    fn device_responses_use_ansi_escape_sequences() {
        assert_eq!(
            DeviceResponse::PrimaryAttributes.escape_sequence(),
            b"\x1b[?62;22c"
        );
    }

    #[test]
    fn scrollback_position_changes_on_the_normal_screen() {
        let mut emulator = Emulator::new(2, 10);
        emulator.feed(b"one\r\ntwo\r\nthree\r\nfour");

        assert!(emulator.set_scrollback(1));
        assert_eq!(emulator.scrollback(), 1);
        assert!(emulator.set_scrollback(0));
        assert_eq!(emulator.scrollback(), 0);
        assert!(!emulator.set_scrollback(0));
    }

    #[test]
    fn alternate_screen_mode_is_reported() {
        let mut emulator = Emulator::new(2, 10);

        emulator.feed(b"\x1b[?1049h");
        assert!(emulator.alternate_screen());

        emulator.feed(b"\x1b[?1049l");
        assert!(!emulator.alternate_screen());
    }

    #[test]
    fn application_cursor_mode_is_reported() {
        let mut emulator = Emulator::new(2, 10);

        emulator.feed(b"\x1b[?1h");
        assert!(emulator.application_cursor());

        emulator.feed(b"\x1b[?1l");
        assert!(!emulator.application_cursor());
    }
}
