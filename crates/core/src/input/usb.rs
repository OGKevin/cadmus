use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::event::{DeviceEvent, PowerSource};
use super::sys::read_fd;

#[cfg_attr(feature = "tracing", tracing::instrument)]
pub fn usb_events() -> UnboundedReceiver<DeviceEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::spawn(async move {
        read_usb_events(&tx).await;
    });
    rx
}

/// Per-read size for `/tmp/nickel-hardware-status` (matches typical FIFO chunking).
const USB_STATUS_READ_BYTES: usize = 256;
/// Cap for an incomplete line held across reads: two read buffers, enough for any
/// real nickel status string split across wakes (messages are short ASCII tokens).
const USB_STATUS_PENDING_MAX: usize = USB_STATUS_READ_BYTES * 2;

/// Opens the nickel hardware-status FIFO.
///
/// Uses `OpenOptions` + `custom_flags` rather than a raw `libc::open` +
/// `OwnedFd::from_raw_fd`: the owned `File` closes the fd on drop, so there
/// is no unsafe here and no leak if the `AsyncFd` setup in the caller fails.
#[cfg_attr(feature = "tracing", tracing::instrument)]
fn open_status_fifo() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/tmp/nickel-hardware-status")
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx)))]
async fn read_usb_events(tx: &UnboundedSender<DeviceEvent>) {
    let file = match open_status_fifo() {
        Ok(file) => file,
        Err(error) => {
            tracing::debug!(error = %error, "usb status fifo unavailable");
            return;
        }
    };
    let owned = OwnedFd::from(file);
    let async_fd = match AsyncFd::with_interest(owned, Interest::READABLE) {
        Ok(async_fd) => async_fd,
        Err(err) => {
            tracing::warn!(error = %err, "usb status reader failed");
            return;
        }
    };

    let mut pending = Vec::new();
    loop {
        let mut guard = match async_fd.readable().await {
            Ok(guard) => guard,
            Err(err) => {
                tracing::warn!(error = %err, "usb status wait failed");
                return;
            }
        };
        loop {
            match guard
                .try_io(|inner| read_usb_status(inner.get_ref().as_raw_fd(), &mut pending, tx))
            {
                Ok(Ok(0)) => return,
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "usb status read failed");
                    return;
                }
                Err(_would_block) => break,
            }
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(pending, tx), level = tracing::Level::TRACE))]
fn read_usb_status(
    fd: RawFd,
    pending: &mut Vec<u8>,
    tx: &UnboundedSender<DeviceEvent>,
) -> std::io::Result<usize> {
    let mut buf = [0u8; USB_STATUS_READ_BYTES];
    let n = read_fd(fd, &mut buf)?;
    if n == 0 {
        return Ok(0);
    }
    let end = buf[..n].iter().position(|byte| *byte == 0).unwrap_or(n);
    feed_usb_status_chunk(pending, &buf[..end], tx);
    Ok(n)
}

/// Appends `chunk` to `pending`, emits complete newline-delimited lines, and
/// retains any trailing fragment for the next read.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pending, tx), level = tracing::Level::TRACE, fields(bytes = chunk.len())))]
fn feed_usb_status_chunk(pending: &mut Vec<u8>, chunk: &[u8], tx: &UnboundedSender<DeviceEvent>) {
    pending.extend_from_slice(chunk);
    if pending.len() > USB_STATUS_PENDING_MAX {
        tracing::warn!(
            pending_len = pending.len(),
            max = USB_STATUS_PENDING_MAX,
            "usb status pending buffer overflow; discarding partial line"
        );
        pending.clear();
        return;
    }

    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let line = pending.drain(..=newline).collect::<Vec<u8>>();
        let line = &line[..line.len() - 1];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        match std::str::from_utf8(line) {
            Ok(message) => send_usb_hardware_message(tx, message),
            Err(_) => tracing::warn!(bytes = ?line, "usb status line is not valid utf-8"),
        }
    }
}
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx)))]
fn send_usb_hardware_message(tx: &UnboundedSender<DeviceEvent>, message: &str) {
    let event = match message {
        "usb plug add" => DeviceEvent::Plug(PowerSource::Host),
        "usb plug remove" => DeviceEvent::Unplug(PowerSource::Host),
        "usb ac add" => DeviceEvent::Plug(PowerSource::Wall),
        "usb ac remove" => DeviceEvent::Unplug(PowerSource::Wall),
        _ => return,
    };
    tx.send(event).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_usb_status_chunk_reassembles_split_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"usb plug ad", &tx);
        assert!(pending == b"usb plug ad");
        assert!(rx.try_recv().is_err());
        feed_usb_status_chunk(&mut pending, b"d\n", &tx);
        assert!(pending.is_empty());
        assert!(matches!(
            rx.try_recv().expect("plug event"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
    }
    #[test]
    fn feed_usb_status_chunk_handles_multiple_lines_and_crlf() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(
            &mut pending,
            b"usb ac add\r\nusb ac remove\nusb plug remove\n",
            &tx,
        );
        assert!(matches!(
            rx.try_recv().expect("wall plug"),
            DeviceEvent::Plug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("wall unplug"),
            DeviceEvent::Unplug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("host unplug"),
            DeviceEvent::Unplug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn feed_usb_status_chunk_ignores_unknown_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"noise\nusb plug add\n", &tx);
        assert!(matches!(
            rx.try_recv().expect("host plug"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }
}
