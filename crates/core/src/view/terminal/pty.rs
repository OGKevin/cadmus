use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::unix::io::RawFd;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl From<TerminalSize> for PtySize {
    fn from(size: TerminalSize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

pub(super) trait TerminalPty {
    fn take_reader(&self) -> Result<Box<dyn Read + Send>>;
    fn as_raw_fd(&self) -> Option<RawFd>;
    fn write(&mut self, data: &[u8]) -> Result<usize>;
    fn resize(&self, size: TerminalSize) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
}

pub(super) struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl Pty {
    pub(super) fn spawn(shell: Option<&str>, size: TerminalSize) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size.into())
            .context("Failed to open PTY")?;

        let shell_path = shell.unwrap_or("/bin/sh");
        let mut cmd = CommandBuilder::new(shell_path);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;

        let writer = pair.master.take_writer().context("Failed to get writer")?;

        Ok(Pty {
            writer,
            master: pair.master,
            child: Some(child),
        })
    }
}

impl TerminalPty for Pty {
    fn take_reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master
            .try_clone_reader()
            .context("Failed to create reader")
    }

    fn as_raw_fd(&self) -> Option<RawFd> {
        self.master.as_raw_fd()
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = self.writer.write(data).context("PTY write failed")?;
        self.writer.flush().context("PTY flush failed")?;
        Ok(n)
    }

    fn resize(&self, size: TerminalSize) -> Result<()> {
        self.master
            .resize(size.into())
            .context("PTY resize failed")?;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                self.child = None;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %error, "Failed to query terminal shell process");
            }
        }

        let kill_result = child.kill().context("Failed to terminate shell process");
        let wait_result = child.wait().context("Failed to reap shell process");
        if wait_result.is_ok() {
            self.child = None;
        }
        wait_result?;
        kill_result
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Err(error) = TerminalPty::shutdown(self) {
            tracing::warn!(error = %error, "Failed to shut down dropped terminal PTY");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Pty;

    #[test]
    fn shutdown_terminates_and_reaps_the_shell_once() -> anyhow::Result<()> {
        let mut pty = Pty::spawn(
            Some("/bin/sh"),
            super::TerminalSize {
                rows: 24,
                cols: 80,
                pixel_width: 800,
                pixel_height: 600,
            },
        )?;

        super::TerminalPty::shutdown(&mut pty)?;
        super::TerminalPty::shutdown(&mut pty)?;

        assert!(pty.child.is_none());
        Ok(())
    }
}
