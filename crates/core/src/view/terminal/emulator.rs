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
}
