use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::os::fd::{FromRawFd as _, OwnedFd};

pub(super) struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl Pty {
    pub(super) fn spawn(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<Self> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        };

        let pair = pty_system.openpty(size).context("Failed to open PTY")?;

        // The master feeds `AsyncFd` (tokio requires non-blocking fds) and
        // shares one open-file description with the reader dup and the
        // writer, so this flag covers both. A blocking master would park a
        // worker inside `read(2)` the moment the shell goes quiet.
        let master_fd = pair
            .master
            .as_raw_fd()
            .context("PTY master has no file descriptor")?;
        // SAFETY: `master_fd` is owned by `pair.master` and open. nix 0.31 has
        // no safe `fcntl`; the calls only read/replace the descriptor flags.
        let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(std::io::Error::last_os_error())
                .context("Failed to make the PTY master non-blocking");
        }

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

    /// Duplicates the PTY master into an owned fd, safe to register with `AsyncFd`.
    ///
    /// The duplicate is independent of `master`'s lifetime, so the reader job can
    /// own it while the session keeps `Pty` for writes.
    pub(super) fn take_reader_fd(&self) -> Result<OwnedFd> {
        // SAFETY: `as_raw_fd` returns the live, owned PTY master fd; the
        // duplicate created here is an independent file description.
        let raw = self
            .master
            .as_raw_fd()
            .context("PTY master has no file descriptor")?;
        // nix 0.31 has no safe `dup` that takes a raw fd; the libc call is
        // sound because the raw fd is owned by `self.master` and open.
        match unsafe { libc::dup(raw) } {
            fd if fd >= 0 => Ok(unsafe { OwnedFd::from_raw_fd(fd) }),
            _ => Err(std::io::Error::last_os_error()).context("Failed to duplicate PTY master fd"),
        }
    }

    pub(super) fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = self.writer.write(data).context("PTY write failed")?;
        self.writer.flush().context("PTY flush failed")?;
        Ok(n)
    }

    pub(super) fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        };
        self.master.resize(size).context("PTY resize failed")?;
        Ok(())
    }

    pub(super) fn shutdown(&mut self) -> Result<()> {
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
        if let Err(error) = self.shutdown() {
            tracing::warn!(error = %error, "Failed to shut down dropped terminal PTY");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Pty;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::io::RawFd;

    fn is_non_blocking(fd: RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        flags & libc::O_NONBLOCK != 0
    }

    #[test]
    fn master_fd_is_non_blocking_for_the_async_reader() {
        let pty = Pty::spawn(Some("/bin/sh"), 24, 80, 800, 600).expect("failed to spawn test PTY");

        // `AsyncFd` requires a non-blocking fd: a blocking master parks a
        // worker inside `read(2)` once the shell goes quiet, so the reader
        // never drains past the first output and never wakes the main loop.
        let master_fd = pty
            .master
            .as_raw_fd()
            .expect("PTY master has no file descriptor");
        assert!(is_non_blocking(master_fd));
        // The reader job owns a dup sharing the master's open-file description.
        let reader_fd = pty.take_reader_fd().expect("failed to dup reader fd");
        assert!(is_non_blocking(reader_fd.as_raw_fd()));
    }

    #[test]
    fn shutdown_terminates_and_reaps_the_shell_once() -> anyhow::Result<()> {
        let mut pty = Pty::spawn(Some("/bin/sh"), 24, 80, 800, 600)?;

        pty.shutdown()?;
        pty.shutdown()?;

        assert!(pty.child.is_none());
        Ok(())
    }
}
