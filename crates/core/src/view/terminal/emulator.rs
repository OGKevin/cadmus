use vt100::Parser;

pub struct Emulator {
    parser: Parser,
}

impl Emulator {
    pub fn new(rows: u16, cols: u16) -> Self {
        Emulator {
            parser: Parser::new(rows, cols, 1000),
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn set_scrollback(&mut self, rows: usize) -> bool {
        let previous = self.scrollback();
        self.parser.screen_mut().set_scrollback(rows);
        self.scrollback() != previous
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }
}

#[cfg(test)]
mod tests {
    use super::Emulator;

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
}
