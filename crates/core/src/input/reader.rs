use std::fs::File;
use std::mem::{self, MaybeUninit};
use std::os::fd::AsRawFd;
use std::ptr;
use std::time::{Duration, Instant};

use tokio::io::unix::{AsyncFd, AsyncFdReadyGuard};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::framebuffer::Display;

use super::discovery::parse_raw_events;
use super::event::{EventSource, InputEvent, SourceId, TaggedInputEvent};
use super::profile::DeviceProfile;
use super::sys::read_fd;
use super::touch::TouchProto;

#[cfg_attr(feature = "tracing", tracing::instrument(skip(screen)))]
pub fn raw_events(
    screen: Display,
    proto: TouchProto,
    cancel: CancellationToken,
) -> (
    UnboundedSender<TaggedInputEvent>,
    UnboundedReceiver<TaggedInputEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_reader = tx.clone();
    crate::runtime::spawn(async move {
        parse_raw_events(&cancel, screen, proto, &tx_reader).await;
    });
    (tx, rx)
}
/// How long an opened device may stay silent before it is first reported.
const SILENCE_REPORT_AFTER: Duration = Duration::from_secs(60);

/// How often a still-silent device repeats its report after the first one.
const SILENCE_REPEAT_EVERY: Duration = Duration::from_secs(600);

/// Reports a device that has forwarded nothing since it was opened.
///
/// Only devices that have forwarded nothing at all are reported, so an
/// idle-but-working device cannot trip it. Reports are trace-level, so the
/// watch is only constructed when trace logging is enabled.
///
/// Timing comes from [`Instant`] (`CLOCK_MONOTONIC`), which does not advance
/// while the system is suspended, and `tokio::time::sleep` shares that clock,
/// so sleep and soft suspend cannot inflate the measured silence. The first
/// window is [`SILENCE_REPORT_AFTER`]; later ones repeat every
/// [`SILENCE_REPEAT_EVERY`].
struct SilenceWatch<'a> {
    path: &'a str,
    profile: DeviceProfile,
    opened_at: Instant,
    reported: bool,
}

impl<'a> SilenceWatch<'a> {
    fn new(path: &'a str, profile: DeviceProfile, opened_at: Instant) -> Self {
        Self {
            path,
            profile,
            opened_at,
            reported: false,
        }
    }

    /// Waits one silence window, then logs the absence of events.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn next_report(&mut self, events_read: u64) {
        let window = if self.reported {
            SILENCE_REPEAT_EVERY
        } else {
            SILENCE_REPORT_AFTER
        };
        tokio::time::sleep(window).await;

        let open_ms = self.opened_at.elapsed().as_millis() as u64;
        tracing::trace!(
            device = self.path,
            profile = ?self.profile,
            events_read,
            open_ms,
            "input device produced no events since open"
        );
        self.reported = true;
    }
}

/// Waits for `fd` to become readable.
///
/// When `silence` holds a watch, the wait races that device's silence timer and
/// returns `None` whenever it elapses, so the caller loops and keeps waiting.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(fd, silence)))]
async fn wait_readable<'a>(
    fd: &'a AsyncFd<File>,
    silence: &mut Option<SilenceWatch<'_>>,
    events_read: u64,
) -> std::io::Result<Option<AsyncFdReadyGuard<'a, File>>> {
    match silence {
        None => fd.readable().await.map(Some),
        Some(watch) => tokio::select! {
            guard = fd.readable() => guard.map(Some),
            () = watch.next_report(events_read) => Ok(None),
        },
    }
}

/// Forwards evdev events from one nonblocking device until it closes, errors,
/// or its consumers vanish.
///
/// Each forwarded event is tagged with `profile`, the scancode classification of
/// the device it was read from, and `source`, the identity of this reader's
/// device among all open devices.
///
/// `AsyncFd` only says the fd is readable. A single `read` may still return
/// fewer bytes than `size_of::<InputEvent>()`. Treating
/// that short read as EOF would drop the device for the rest of the session.
/// `pending` / `filled` keep those bytes across `readable()` waits until one
/// full record is available. A zero-length read is the only EOF.
///
/// While the device has forwarded nothing yet, its readiness wait races a
/// [`SilenceWatch`] so "opened but never delivered a byte" is observable.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(fd, tx)))]
pub(super) async fn read_input_events(
    fd: AsyncFd<File>,
    path: &str,
    profile: DeviceProfile,
    source: SourceId,
    tx: &UnboundedSender<TaggedInputEvent>,
) -> ReaderExit {
    let opened_at = Instant::now();
    let mut pending = [0_u8; mem::size_of::<InputEvent>()];
    let mut filled = 0usize;
    let mut events_read = 0u64;
    let mut silence = if tracing::enabled!(tracing::Level::TRACE) {
        Some(SilenceWatch::new(path, profile, opened_at))
    } else {
        None
    };

    loop {
        let mut guard = loop {
            match wait_readable(&fd, &mut silence, events_read).await {
                Ok(Some(guard)) => break guard,
                Ok(None) => continue,
                Err(error) => {
                    return reader_exit(ExitReason::Io(error), events_read, opened_at);
                }
            }
        };
        loop {
            match guard.try_io(|inner| read_input_event(inner.get_ref(), &mut pending, &mut filled))
            {
                Ok(Ok(event)) => {
                    if tx
                        .send(TaggedInputEvent {
                            event,
                            profile,
                            source: EventSource::Device(source),
                        })
                        .is_err()
                    {
                        return reader_exit(ExitReason::ChannelClosed, events_read, opened_at);
                    }
                    events_read += 1;
                    if events_read == 1 {
                        silence = None;
                        tracing::debug!(
                            device = path,
                            first_event_after_ms = opened_at.elapsed().as_millis() as u64,
                            "input device produced its first event"
                        );
                    }
                }
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return reader_exit(ExitReason::Eof, events_read, opened_at);
                }
                Ok(Err(err)) => return reader_exit(ExitReason::Io(err), events_read, opened_at),
                Err(_would_block) => break,
            }
        }
    }
}

/// Why a device reader task stopped forwarding events.
///
/// Logged with the reader's totals: before this existed, the channel-closed and
/// end-of-file paths returned silently, so a dead touch digitizer was
/// indistinguishable in a log from a device that never sent anything.
#[derive(Debug)]
pub(super) enum ExitReason {
    /// The device node reported end-of-file.
    Eof,
    /// Every consumer of the tagged-event stream is gone.
    ChannelClosed,
    /// A read or readiness error other than end-of-file.
    Io(std::io::Error),
}

impl std::fmt::Display for ExitReason {
    /// Renders as a stable label, with the inner error text appended for the
    /// I/O variant so the log carries the kernel message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => f.write_str("eof"),
            Self::ChannelClosed => f.write_str("channel_closed"),
            Self::Io(error) => write!(f, "io_error: {error}"),
        }
    }
}

/// Totals a reader reports when it stops forwarding events.
#[derive(Debug)]
pub(super) struct ReaderExit {
    pub(super) reason: ExitReason,
    pub(super) events_read: u64,
    pub(super) open_ms: u64,
}

#[cfg_attr(feature = "tracing", tracing::instrument)]
fn reader_exit(reason: ExitReason, events_read: u64, opened_at: Instant) -> ReaderExit {
    ReaderExit {
        reason,
        events_read,
        open_ms: opened_at.elapsed().as_millis() as u64,
    }
}

/// Reads one `InputEvent`, keeping a short read in `pending` until the next
/// readiness wake. A zero-length read is EOF. `WouldBlock` leaves `filled` as-is.
///
/// `InputEvent` is a C layout with no safe constructor from raw bytes, so this
/// builds one with `MaybeUninit`. The copy is sound because `pending` only
/// reaches `assume_init` once the read has filled all
/// `size_of::<InputEvent>()` bytes, so every byte is written first.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pending), level = tracing::Level::TRACE))]
fn read_input_event(
    file: &File,
    pending: &mut [u8; mem::size_of::<InputEvent>()],
    filled: &mut usize,
) -> std::io::Result<InputEvent> {
    debug_assert!(*filled < pending.len(), "pending record is never left full");
    let n = read_fd(file.as_raw_fd(), &mut pending[*filled..])?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "evdev closed",
        ));
    }
    *filled += n;
    if *filled < pending.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "partial evdev event",
        ));
    }
    let bytes = *pending;
    *filled = 0;
    let mut input_event = MaybeUninit::<InputEvent>::uninit();
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            input_event.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Ok(input_event.assume_init())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    use tokio::io::Interest;

    use crate::input::codes::{EV_KEY, KEY_HOME, KEY_UP, VAL_PRESS};

    /// The reader must report one forwarded event and a clean EOF exit once
    /// the write end closes, not a silent `Ok(())`: a dead device must be
    /// visible in the log.
    #[tokio::test]
    async fn raw_reader_forwards_one_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = crate::runtime::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let writer = crate::runtime::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 4,
                tv_usec: 5,
            },
            kind: EV_KEY,
            code: KEY_HOME,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        let mut writer = writer;
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for evdev event")
            .expect("reader closed");
        assert_eq!(got.profile, DeviceProfile::built_in());
        assert_eq!(got.event.kind, EV_KEY);
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.value, VAL_PRESS);
        assert_eq!(got.event.time.tv_sec, 4);
        assert_eq!(got.event.time.tv_usec, 5);

        drop(writer);
        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::Eof));
        assert_eq!(exit.events_read, 1);
        let _ = std::fs::remove_file(&path);
    }
    /// A device node that delivers zero bytes (the kernel-side EOF case) must
    /// surface as [`ExitReason::Eof`], not as a silent return.
    #[tokio::test]
    async fn read_input_events_reports_eof_for_empty_device() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader_path = path_str.clone();
        let reader = crate::runtime::spawn(async move {
            read_input_events(
                fd,
                reader_path.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer = crate::runtime::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(&path_str)
        })
        .await
        .expect("writer task")
        .expect("open fifo");
        drop(writer);

        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::Eof));
        assert_eq!(exit.events_read, 0);
        let _ = std::fs::remove_file(&path);
    }
    /// A reader whose consumers are all gone must stop with
    /// [`ExitReason::ChannelClosed`] and report how many events it had
    /// forwarded — previously this path returned silently.
    #[tokio::test]
    async fn read_input_events_reports_channel_closed() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-closed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader_path = path_str.clone();
        let reader = crate::runtime::spawn(async move {
            read_input_events(
                fd,
                reader_path.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = crate::runtime::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 4,
                tv_usec: 5,
            },
            kind: EV_KEY,
            code: KEY_HOME,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let exit = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("timed out waiting for reader exit")
            .expect("reader task panicked");
        assert!(matches!(exit.reason, ExitReason::ChannelClosed));
        assert_eq!(exit.events_read, 0);
        drop(writer);
        let _ = std::fs::remove_file(&path);
    }
    /// Reader exits render as stable labels, and the I/O variant carries the
    /// inner error text into the log.
    #[test]
    fn exit_reason_renders_stable_label_and_io_error_text() {
        assert_eq!(ExitReason::Eof.to_string(), "eof");
        assert_eq!(ExitReason::ChannelClosed.to_string(), "channel_closed");

        let io = ExitReason::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let rendered = io.to_string();
        assert!(rendered.starts_with("io_error: "), "{rendered}");
        assert!(
            rendered.to_lowercase().contains("permission denied"),
            "{rendered}"
        );
    }
    /// The device's classified profile must tag every forwarded event, not just
    /// the built-in default: a remote's arrow keys reach the pipeline with the
    /// remote profile so they map to raw page-turn.
    #[tokio::test]
    async fn raw_reader_tags_events_with_device_profile() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = crate::runtime::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::remote(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = crate::runtime::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 7,
                tv_usec: 2,
            },
            kind: EV_KEY,
            code: KEY_UP,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for evdev event")
            .expect("reader closed");
        assert_eq!(got.profile, DeviceProfile::remote());
        assert_eq!(got.event.code, KEY_UP);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }
    /// A record split across two reads must be held until complete, and must
    /// not be mistaken for EOF.
    #[tokio::test]
    async fn raw_reader_holds_a_split_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-split-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = crate::runtime::spawn(async move {
            read_input_events(
                fd,
                path_str.as_str(),
                DeviceProfile::built_in(),
                SourceId(0),
                &tx,
            )
            .await
        });

        let writer_path = path.clone();
        let mut writer = crate::runtime::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(
                bytes.as_mut_ptr().cast::<InputEvent>(),
                InputEvent {
                    time: libc::timeval {
                        tv_sec: 9,
                        tv_usec: 1,
                    },
                    kind: EV_KEY,
                    code: KEY_HOME,
                    value: VAL_PRESS,
                },
            );
        }
        let split = mem::size_of::<InputEvent>() / 2;

        writer.write_all(&bytes[..split]).expect("write first half");
        writer.flush().expect("flush first half");
        assert!(
            rx.try_recv().is_err(),
            "a half record must not be forwarded"
        );

        writer
            .write_all(&bytes[split..])
            .expect("write second half");
        writer.flush().expect("flush second half");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for the joined evdev event")
            .expect("reader closed");
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.time.tv_sec, 9);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }
}
