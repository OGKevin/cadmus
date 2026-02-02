use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::unix::io::RawFd;

pub struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

impl Pty {
    pub fn spawn(shell: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = native_pty_system();
        
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        
        let pair = pty_system
            .openpty(size)
            .context("Failed to open PTY")?;
        
        let shell_path = shell.unwrap_or("/bin/sh");
        let mut cmd = CommandBuilder::new(shell_path);
        cmd.env("TERM", "xterm-256color");
        
        pair.slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;
        
        let writer = pair.master.take_writer()
            .context("Failed to get writer")?;
        
        Ok(Pty {
            writer,
            master: pair.master,
        })
    }

    pub fn take_reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master.try_clone_reader().context("Failed to create reader")
    }

    pub fn as_raw_fd(&self) -> Option<RawFd> {
        self.master.as_raw_fd()
    }
    
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = self.writer.write(data)
            .context("PTY write failed")?;
        self.writer.flush()
            .context("PTY flush failed")?;
        Ok(n)
    }
}
