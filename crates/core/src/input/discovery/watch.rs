//! Asynchronous directory-change notification for input device discovery.
//!
//! [`DeviceWatch`] watches a directory (for Cadmus: `/dev/input`) with inotify
//! instead of polling it. The watch targets the directory, not each device
//! node: a per-node watch dies with the node it observes, while a directory
//! watch survives create/delete/replug cycles. Callers drive it from a
//! `select!` arm and rescan when [`DeviceWatch::next_change`] reports a change,
//! debouncing briefly so udev has applied ownership and permissions to a
//! just-created node before it is probed, and keep a slow periodic rescan
//! alongside the watch so `IN_Q_OVERFLOW` and missed events self-heal.

use std::io;
use std::os::fd::{AsFd, AsRawFd, RawFd};

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use tokio::io::unix::AsyncFd;

/// `AsyncFd` requires `AsRawFd`, which nix's `Inotify` does not implement even
/// though it implements `AsFd`; this adapter bridges the two.
struct WatchFd(Inotify);

impl AsRawFd for WatchFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_fd().as_raw_fd()
    }
}

/// An inotify watch on a directory of input device nodes.
///
/// The watch is on the directory rather than the nodes inside it, so it keeps
/// reporting across the full hotplug cycle instead of being dropped together
/// with a removed node. Dropping the watch closes the inotify descriptor and
/// releases its watch, so no manual cleanup is needed.
pub(super) struct DeviceWatch {
    fd: AsyncFd<WatchFd>,
}

impl DeviceWatch {
    /// Watches `path` for node creation, deletion, rename and metadata changes.
    ///
    /// `IN_ATTRIB` is included because udev applies ownership and permissions
    /// through metadata changes after a node appears. `IN_NONBLOCK` makes the
    /// descriptor safe to poll through [`AsyncFd`], and `IN_CLOEXEC` keeps it
    /// out of spawned processes. Failure means the kernel has no inotify
    /// support or the path cannot be watched; the caller should then fall back
    /// to polling the directory so hotplug support degrades to the previous
    /// behaviour instead of disappearing.
    ///
    /// Registering with [`AsyncFd`] is unsafe: `WatchFd` owns the inotify
    /// descriptor, so it stays open and unchanged until the `AsyncFd` is
    /// dropped.
    #[cfg_attr(feature = "tracing", tracing::instrument(fields(watch = path)))]
    pub(super) fn new(path: &str) -> io::Result<Self> {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(io::Error::from)?;
        inotify
            .add_watch(
                path,
                AddWatchFlags::IN_CREATE
                    | AddWatchFlags::IN_DELETE
                    | AddWatchFlags::IN_MOVED_TO
                    | AddWatchFlags::IN_MOVED_FROM
                    | AddWatchFlags::IN_ATTRIB,
            )
            .map_err(io::Error::from)?;
        Ok(Self {
            fd: unsafe { AsyncFd::register(WatchFd(inotify))? },
        })
    }

    /// Waits for the next directory change and reports whether a rescan is
    /// warranted.
    ///
    /// Readiness is cleared through the [`AsyncFd`] guard, then the event queue
    /// is drained, so a burst of events from one udev transaction costs a single
    /// wakeup. Spurious readiness with an empty or `EAGAIN` queue reports
    /// `false`, leaving the caller to keep waiting.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(watch = "/dev/input"))
    )]
    pub(super) async fn next_change(&self) -> bool {
        loop {
            let Ok(mut guard) = self.fd.readable().await else {
                return true;
            };
            match guard.try_io(|inner| inner.get_ref().0.read_events().map_err(io::Error::from)) {
                Ok(Ok(events)) if !events.is_empty() => return true,
                Ok(Ok(_)) | Err(_) => continue,
                Ok(Err(_)) => return true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    /// A directory watch must report a node appearing and, afterwards, the same
    /// node disappearing, as rescan-worthy changes on the same watch instance.
    #[tokio::test]
    async fn watch_reports_create_and_delete() {
        let dir = std::env::temp_dir().join(format!(
            "cadmus-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create watch directory");
        let file = dir.join("event0");

        let watch =
            Arc::new(DeviceWatch::new(dir.to_str().expect("utf8 path")).expect("watch directory"));

        let waiter = {
            let watch = Arc::clone(&watch);
            crate::runtime::spawn(async move { watch.next_change().await })
        };
        std::fs::write(&file, b"").expect("create file");
        let reported = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("timed out waiting for the create event")
            .expect("watch task panicked");
        assert!(reported);

        let waiter = {
            let watch = Arc::clone(&watch);
            crate::runtime::spawn(async move { watch.next_change().await })
        };
        std::fs::remove_file(&file).expect("delete file");
        let reported = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("timed out waiting for the delete event")
            .expect("watch task panicked");
        assert!(reported);

        drop(watch);
        std::fs::remove_dir_all(&dir).expect("remove watch directory");
    }
}
