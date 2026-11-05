use std::os::fd::RawFd;

/// Reads from `fd` into `buf`, retrying on `EINTR`; partial reads are
/// returned rather than retried.
///
/// # Safety
///
/// `fd` must be a valid open file descriptor (as handed out by `AsyncFd`)
/// and `buf` must remain valid and writable for the duration of the call.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(buf), level = tracing::Level::TRACE))]
pub(super) fn read_fd(fd: RawFd, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len() as libc::size_t,
            )
        };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}
