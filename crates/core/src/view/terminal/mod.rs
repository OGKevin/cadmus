//! Interactive terminal view backed by a pseudo-terminal and VT100 emulator.
//!
//! The module owns the child shell, translates Cadmus input into terminal input,
//! and renders the emulated screen through a double buffer. Layout changes keep
//! the PTY, emulator, and renderer on the same character and pixel geometry.

mod buffer;
mod emulator;
mod pty;
mod render;
mod session;

pub use session::Terminal;
