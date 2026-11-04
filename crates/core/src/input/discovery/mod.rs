mod classify;
mod probe;
mod watch;

use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::framebuffer::Display;
use crate::input::event::{SourceId, TaggedInputEvent};
use crate::input::reader::read_input_events;
use crate::input::touch::TouchProto;

use self::classify::classify_device;
use self::probe::{DeviceFeatures, TouchAxes, probe_device, trace_touch_device_geometry};
use self::watch::DeviceWatch;

/// How often `/dev/input` is polled for hotplugged devices when the inotify
/// watch is unavailable.
const SCAN_INTERVAL: Duration = Duration::from_secs(5);

/// How often the device set is rescanned even when the watch reports nothing,
/// covering `IN_Q_OVERFLOW`, missed events and dead readers.
const FALLBACK_RESCAN_INTERVAL: Duration = Duration::from_secs(60);

/// How long a watch-reported change is held before scanning, so udev has
/// applied ownership and permissions to the new node before it is probed.
/// Scanning on the create event alone can hit `EACCES` and blacklist a healthy
/// device.
const HOTPLUG_DEBOUNCE: Duration = Duration::from_millis(100);

/// How long to wait before rescanning after a device reader exits.
///
/// A reader that dies while its node stays in `/dev/input` — a driver hiccup —
/// produces no directory event, so the watch alone would leave that device
/// closed until [`FALLBACK_RESCAN_INTERVAL`]. Retrying on the old poll cadence
/// keeps recovery at the latency this loop had before the watch existed, and
/// waiting first means a node that fails to open cannot drive back-to-back
/// scans.
const READER_RETRY_DELAY: Duration = SCAN_INTERVAL;

/// One pass over `/dev/input`.
#[derive(Default)]
struct Scan {
    /// Devices that are neither tracked nor blacklisted, in directory order.
    devices: Vec<(String, DeviceFeatures)>,
    /// Set when the pass could not inspect every node, so the caller retries.
    error: Option<DiscoverError>,
}

/// Why a discovery pass could not see every device.
///
/// A scan is partial, not failed: `devices` still holds what was found. Each
/// failure is already logged where it happens, so this value only tells the
/// loop that a retry is owed.
#[derive(Debug, thiserror::Error)]
enum DiscoverError {
    /// `/dev/input` could not be read at all.
    #[error("cannot read /dev/input: {0}")]
    ReadDir(#[source] std::io::Error),
    /// Nodes that exist but could not be probed, typically a udev permission
    /// race.
    #[error("{0} input node(s) could not be probed")]
    Unprobed(usize),
    /// A probed node could not be opened or watched.
    #[error("cannot set up {path}: {source}")]
    Setup {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Scans `/dev/input/event*` for input devices not already tracked.
///
/// Returns each new device's path alongside its probed [`DeviceFeatures`] so the
/// caller can classify it without re-probing. Excludes paths in `tracked_paths`
/// (already opened persistent + removable devices) and `blacklisted_paths`
/// (known duplicates). Deduplicates by device identity so a device exposed
/// under multiple event nodes is only opened once; duplicates are added to
/// `blacklisted_paths` to avoid re-querying them on every scan.
///
/// Blacklisted paths whose node no longer exists are dropped during the scan:
/// the kernel reuses `eventN` numbers, so a stale entry would otherwise cause
/// a different device later assigned the same node to be skipped forever. The
/// current path set comes from the same directory listing the scan walks, so
/// the cleanup adds no extra filesystem access.
///
/// Unlike the previous keyboard-only scan, this opens any device that emits
/// `EV_KEY` or `EV_ABS`, so the touch digitizer and built-in buttons are
/// discovered at startup alongside hotplugged keyboards.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(known_identities, blacklisted_paths))
)]
async fn scan_input_devices(
    tracked_paths: &[String],
    known_identities: &mut FxHashSet<String>,
    blacklisted_paths: &mut FxHashSet<String>,
) -> Scan {
    let mut unprobed = 0_usize;
    let mut scan = Scan::default();
    let mut entries = match tokio::fs::read_dir("/dev/input").await {
        Ok(entries) => entries,
        Err(error) => {
            scan.error = Some(DiscoverError::ReadDir(error));
            return scan;
        }
    };

    let mut current_paths = FxHashSet::default();
    while let Some(entry) = entries.next_entry().await.ok().flatten() {
        let path = entry.path();
        let Some(path_str) = path.to_str() else {
            continue;
        };
        current_paths.insert(path_str.to_string());
        if !path_str.starts_with("/dev/input/event") {
            continue;
        }
        if tracked_paths.iter().any(|p| p == path_str) || blacklisted_paths.contains(path_str) {
            continue;
        }
        let Some(features) = probe_device(path_str).await else {
            tracing::debug!(device = path_str, "scan: probe failed; will retry");
            unprobed += 1;
            continue;
        };
        if !features.emits_key && !features.emits_abs {
            continue;
        }
        match features.identity() {
            Some(identity) => {
                if known_identities.insert(identity.clone()) {
                    tracing::debug!(device = path_str, identity, "scan: new device");
                    scan.devices.push((path_str.to_string(), features));
                } else {
                    tracing::debug!(
                        device = path_str,
                        identity,
                        "scan: blacklisting duplicate device"
                    );
                    blacklisted_paths.insert(path_str.to_string());
                }
            }
            None => {
                tracing::debug!(device = path_str, "scan: device without identity");
                scan.devices.push((path_str.to_string(), features));
            }
        }
    }

    forget_missing_blacklisted_paths(blacklisted_paths, &current_paths);

    if unprobed > 0 {
        scan.error = Some(DiscoverError::Unprobed(unprobed));
    }
    scan
}

/// Drops blacklisted paths that no longer exist under `/dev/input`, so a
/// future device reusing a recycled `eventN` node is probed again instead of
/// being skipped forever.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(blacklisted_paths, current_paths))
)]
fn forget_missing_blacklisted_paths(
    blacklisted_paths: &mut FxHashSet<String>,
    current_paths: &FxHashSet<String>,
) {
    blacklisted_paths.retain(|path| {
        let exists = current_paths.contains(path);
        if !exists {
            tracing::debug!(device = path, "forgetting blacklisted path: node is gone");
        }
        exists
    });
}

/// Reads raw evdev events from all input devices and forwards them as
/// [`TaggedInputEvent`]s on `tx`.
///
/// Every device under `/dev/input/event*` is probed at startup, classified into
/// a [`DeviceProfile`], and opened as a **persistent** device. Built-in nodes
/// (`/dev/input/eventN`) outlive driver hiccups, so a reader that exits from a
/// driver error is re-opened on the next scan rather than dropped for the rest
/// of the session. An inotify watch on `/dev/input` discovers **removable**
/// devices hotplugged after startup, falling back to the periodic [`SCAN_INTERVAL`]
/// rescan when the watch cannot be created, and when a reader stops the device's
/// identity is released so a replug is re-detected instead of being treated as a
/// duplicate.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, screen)))]
pub(super) async fn parse_raw_events(
    cancel: &CancellationToken,
    screen: Display,
    proto: TouchProto,
    tx: &UnboundedSender<TaggedInputEvent>,
) {
    let mut device = OpenInputDevices {
        watched: Vec::new(),
        readers: tokio::task::JoinSet::new(),
        known_identities: FxHashSet::default(),
        blacklisted_paths: FxHashSet::default(),
        proto,
        display: screen,
        tx,
        next_source: SourceId::START,
    };

    let watch = DeviceWatch::new("/dev/input");
    let startup_failed = device.open(&[], false).await.is_err();

    match watch {
        Ok(watch) => run_with_watch(cancel, &mut device, watch, startup_failed).await,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "inotify device watch unavailable; falling back to periodic rescan"
            );
            run_with_polling(cancel, &mut device).await;
        }
    }
}

/// Runs the scan/open/reap loop with a directory watch as the rescan trigger.
///
/// A watch-reported change is held for [`HOTPLUG_DEBOUNCE`] before scanning,
/// because udev creates the event node and only then applies ownership and
/// permissions. [`FALLBACK_RESCAN_INTERVAL`] rescans regardless of the watch,
/// so `IN_Q_OVERFLOW` and missed events self-heal. A failed scan, and a reader
/// exit, arm a one-shot retry [`READER_RETRY_DELAY`] later: a node that could
/// not be opened yet, or one that died to a driver hiccup, produces no further
/// directory event, and waiting keeps a node that keeps failing from driving a
/// scan loop.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(device, watch)))]
async fn run_with_watch(
    cancel: &CancellationToken,
    device: &mut OpenInputDevices<'_>,
    watch: DeviceWatch,
    startup_failed: bool,
) {
    let mut retry_at = schedule_retry(startup_failed);

    loop {
        let readers_busy = !device.readers.is_empty();
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            rescan = watch.next_change() => {
                if rescan {
                    tokio::time::sleep(HOTPLUG_DEBOUNCE).await;
                    retry_at = schedule_retry(device.open(&device.tracked(), true).await.is_err());
                }
            }
            () = tokio::time::sleep(FALLBACK_RESCAN_INTERVAL) => {
                retry_at = schedule_retry(device.open(&device.tracked(), true).await.is_err());
            }
            Some(joined) = device.readers.join_next(), if readers_busy => {
                device.reap(joined);
                retry_at = schedule_retry(true);
            }
            () = wait_for_retry(retry_at) => {
                retry_at = schedule_retry(device.open(&device.tracked(), true).await.is_err());
            }
        }
    }
}

/// Returns the deadline of the next reader retry when `failed`, or `None` when
/// the last scan was complete and no retry is owed.
fn schedule_retry(failed: bool) -> Option<Instant> {
    failed.then(|| Instant::now() + READER_RETRY_DELAY)
}

/// Resolves at `at`, or never while no retry is armed.
///
/// The deadline is absolute, so an unrelated loop iteration cannot push the
/// retry back, and it is cancel-safe, so a watch or fallback scan can win the
/// race without leaving a half-consumed timer behind.
async fn wait_for_retry(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// Runs the scan/open/reap loop with the pre-watch periodic rescan, used when
/// the directory watch cannot be created.
///
/// Same cadence and structure as before the watch existed: every
/// [`SCAN_INTERVAL`] a full scan runs, whether or not anything changed.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(device)))]
async fn run_with_polling(cancel: &CancellationToken, device: &mut OpenInputDevices<'_>) {
    let mut last_scan = Instant::now();
    loop {
        let next_scan = SCAN_INTERVAL.saturating_sub(last_scan.elapsed());
        let readers_busy = !device.readers.is_empty();
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(next_scan) => {}
            Some(joined) = device.readers.join_next(), if readers_busy => {
                device.reap(joined);
            }
        }

        if last_scan.elapsed() >= SCAN_INTERVAL {
            last_scan = Instant::now();
            let _ = device.open(&device.tracked(), true).await;
        }
    }
}

/// Owns the evdev devices the input reader currently watches.
///
/// One reader task per open device forwards events on `tx`; when a task ends it
/// returns the device's path and dedup identity so [`Self::reap`] can release
/// them, letting a later scan re-open a persistent node or re-detect a replugged
/// removable one.
struct OpenInputDevices<'a> {
    watched: Vec<(String, Option<String>)>,
    readers: tokio::task::JoinSet<(String, Option<String>)>,
    known_identities: FxHashSet<String>,
    blacklisted_paths: FxHashSet<String>,
    proto: TouchProto,
    display: Display,
    tx: &'a UnboundedSender<TaggedInputEvent>,
    /// Monotonic counter so each opened device gets a unique [`SourceId`].
    /// Ids of devices whose readers exited are never reused, so stale entries
    /// in per-source state can never be confused with a new device.
    next_source: SourceId,
}

impl OpenInputDevices<'_> {
    /// Paths currently watched, passed to the scan as already tracked.
    fn tracked(&self) -> Vec<String> {
        self.watched.iter().map(|(path, _)| path.clone()).collect()
    }

    /// Scans for devices not already tracked and opens a reader for each.
    ///
    /// Each device is classified into a [`DeviceProfile`] from its probed
    /// features; that profile tags every event its reader forwards, so the
    /// device-event pipeline can route the device's scancodes through the right
    /// table. `hotplugged` only distinguishes the log line between the startup
    /// set and devices that appeared later.
    ///
    /// Fails with the first problem encountered: an incomplete scan, or a node
    /// that could not be opened or watched. The fallible steps are typically a
    /// udev permission race, so the caller arms a retry instead of waiting for
    /// the next directory event. Every failure is logged where it happens; the
    /// returned error only carries the retry signal.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn open(&mut self, tracked: &[String], hotplugged: bool) -> Result<(), DiscoverError> {
        let Scan { devices, mut error } = scan_input_devices(
            tracked,
            &mut self.known_identities,
            &mut self.blacklisted_paths,
        )
        .await;
        for (path, features) in devices {
            let mut touch_axes: Option<TouchAxes> = None;
            let file = match tokio::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
                .await
            {
                Ok(file) => file.into_std().await,
                Err(source) => {
                    tracing::warn!(
                        device = %path,
                        error = %source,
                        "failed to open input device; skipping"
                    );
                    self.forget_scanned_identity(&features);
                    error.get_or_insert(DiscoverError::Setup { path, source });
                    continue;
                }
            };
            if features.emits_abs {
                touch_axes =
                    Some(trace_touch_device_geometry(&path, self.proto, self.display).await);
            }
            let fd = match AsyncFd::with_interest(file, Interest::READABLE) {
                Ok(fd) => fd,
                Err(source) => {
                    tracing::warn!(
                        device = %path,
                        error = %source,
                        "failed to watch input device; skipping"
                    );
                    self.forget_scanned_identity(&features);
                    error.get_or_insert(DiscoverError::Setup { path, source });
                    continue;
                }
            };
            let profile = classify_device(&features);
            let identity = features.identity();
            let source = self.next_source.next();
            tracing::info!(
                device = %path,
                profile = ?profile,
                source = ?source,
                name = features.name.as_deref().unwrap_or(""),
                bus = features.bustype,
                vendor = features.vendor,
                product = features.product,
                version = features.version,
                identity = identity.as_deref().unwrap_or(""),
                emits_key = features.emits_key,
                emits_abs = features.emits_abs,
                touch_candidate = features.has_position_axes,
                abs_x = ?touch_axes.as_ref().and_then(|(x, _)| *x),
                abs_y = ?touch_axes.as_ref().and_then(|(_, y)| *y),
                hotplugged,
                "opened input device"
            );
            let tx = self.tx.clone();
            let path_for_reader = path.clone();
            let identity_for_reader = identity.clone();
            self.readers.spawn(async move {
                let exit = read_input_events(fd, &path_for_reader, profile, source, &tx).await;
                tracing::warn!(
                    device = %path_for_reader,
                    reason = %exit.reason,
                    events_read = exit.events_read,
                    open_ms = exit.open_ms,
                    "input device reader stopped"
                );
                (path_for_reader, identity_for_reader)
            });
            self.watched.push((path, identity));
        }

        error.map_or(Ok(()), Err)
    }

    /// Releases the dedup identity of a device that was scanned but not opened,
    /// so the next scan retries it instead of treating it as a duplicate.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, features)))]
    fn forget_scanned_identity(&mut self, features: &DeviceFeatures) {
        if let Some(identity) = features.identity() {
            self.known_identities.remove(&identity);
        }
    }

    /// Releases the device whose reader just ended.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn reap(&mut self, joined: Result<(String, Option<String>), tokio::task::JoinError>) {
        if let Ok((path, identity)) = joined {
            if let Some(pos) = self.watched.iter().position(|(p, _)| p == &path) {
                self.watched.remove(pos);
            }
            if let Some(identity) = identity.as_ref() {
                self.known_identities.remove(identity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forget_missing_blacklisted_paths_drops_gone_nodes_and_keeps_existing() {
        let mut blacklisted = FxHashSet::default();
        blacklisted.insert("/dev/input/event3".to_string());
        blacklisted.insert("/dev/input/event7".to_string());

        let mut current = FxHashSet::default();
        current.insert("/dev/input/event3".to_string());
        current.insert("/dev/input/event9".to_string());

        forget_missing_blacklisted_paths(&mut blacklisted, &current);

        assert!(blacklisted.contains("/dev/input/event3"));
        assert_eq!(blacklisted.len(), 1);
    }
    /// A device that was scanned but never opened (open or watch failed) must
    /// have its dedup identity released: otherwise the next scan treats the
    /// same node as a duplicate of itself and blacklists it until restart.
    #[tokio::test]
    async fn forget_scanned_identity_releases_unopened_device() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut devices = OpenInputDevices {
            watched: Vec::new(),
            readers: tokio::task::JoinSet::new(),
            known_identities: FxHashSet::default(),
            blacklisted_paths: FxHashSet::default(),
            proto: TouchProto::MultiB,
            display: Display {
                dims: (1264, 1680),
                rotation: 1,
            },
            tx: &tx,
            next_source: SourceId::START,
        };

        let features = DeviceFeatures {
            name: Some("virtual-keyboard".to_string()),
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let identity = features.identity().expect("identity");
        devices.known_identities.insert(identity.clone());

        devices.forget_scanned_identity(&features);

        assert!(!devices.known_identities.contains(&identity));
    }
}
