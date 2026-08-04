//! Soft-suspend session: named leases over one `cadmus` wake lock.

use crate::device::leds::DeviceLeds;
use crate::device::soft_suspend::WAKE_LOCK_NAME;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::device::soft_suspend::paths::{
    SoftSuspendPaths, SysfsWrite, discover_available_modes, write_sysfs,
};
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Writes soft-suspend sysfs and logs. Returns whether session state may advance.
fn write_sysfs_applied(path: &Path, value: &str) -> bool {
    match write_sysfs(path, value) {
        Ok(SysfsWrite::Written) => {
            tracing::debug!(path = %path.display(), value, "wrote soft-suspend sysfs");
            true
        }
        Ok(SysfsWrite::Missing) => {
            tracing::debug!(path = %path.display(), value, "soft-suspend sysfs path missing");
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, value, "failed to write soft-suspend sysfs");
            false
        }
    }
}

struct SessionState {
    mode: AutosleepMode,
    indicate_autosleep_led: bool,
    autosleep_grace: Duration,
}

struct UnlockState {
    /// When to write `wake_unlock` after the last lease dropped; `None` if idle
    /// or a holder re-armed the lock.
    due_at: Option<Instant>,
    /// Whether the kernel `cadmus` wake lock is currently taken.
    ///
    /// Stays true across the release-grace window after the last lease drops
    /// (`due_at` set, tracker empty) until unlock or session teardown.
    held: bool,
    /// Sysfs paths used for `wake_lock` / `wake_unlock` writes.
    paths: SoftSuspendPaths,
    /// Set by [`UnlockInner::shutdown`]; the worker exits its loop when true.
    shutdown: bool,
}

struct UnlockInner {
    /// Shared unlock / wake-lock state for the grace worker and arming path.
    state: Mutex<UnlockState>,
    /// Wakes the grace worker when `due_at`, `shutdown`, or arming changes.
    cv: Condvar,
    /// Count of in-flight [`SoftSuspendSession::acquire`] calls.
    ///
    /// Distinct from [`UnlockState::held`]: pins cover only the acquire stack
    /// frame (tracker insert + `take_wake_lock`), so the grace worker can skip
    /// or re-arm unlock when a concurrent acquire races `due_at` expiry.
    /// Zero while leases are merely held, and during grace with no acquire.
    pins: AtomicUsize,
}

impl UnlockInner {
    fn new(paths: SoftSuspendPaths) -> Arc<Self> {
        let inner = Arc::new(Self {
            state: Mutex::new(UnlockState {
                due_at: None,
                held: false,
                paths,
                shutdown: false,
            }),
            cv: Condvar::new(),
            pins: AtomicUsize::new(0),
        });
        let worker = Arc::clone(&inner);
        thread::spawn(move || worker.run());
        inner
    }

    fn run(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.shutdown {
                break;
            }

            let Some(due) = state.due_at else {
                state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            };

            let now = Instant::now();
            if due > now {
                let wait = due - now;
                let (guard, _) = self
                    .cv
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|e| e.into_inner());
                state = guard;
                continue;
            }

            if state.due_at.is_some_and(|d| d <= Instant::now()) {
                if self.pins.load(Ordering::SeqCst) != 0 {
                    state.due_at = None;
                    continue;
                }
                let paths = state.paths.clone();
                state.due_at = None;
                tracing::info!(
                    wake_lock = WAKE_LOCK_NAME,
                    "soft-suspend release grace elapsed; unlocking"
                );
                let unlocked = write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
                if self.pins.load(Ordering::SeqCst) != 0 {
                    tracing::debug!(
                        wake_lock = WAKE_LOCK_NAME,
                        "soft-suspend re-arm after unlock raced with acquire"
                    );
                    if write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME) {
                        state.held = true;
                    } else if unlocked {
                        state.held = false;
                    }
                } else if unlocked {
                    state.held = false;
                }
            }
        }
    }

    /// Marks an in-flight acquire so the grace worker will not unlock.
    fn pin(&self) {
        self.pins.fetch_add(1, Ordering::SeqCst);
        self.cancel_pending_unlock();
    }

    /// Ends an in-flight acquire pin from [`Self::pin`].
    fn unpin(&self) {
        self.pins.fetch_sub(1, Ordering::SeqCst);
    }

    fn take_wake_lock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        if write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.held = true;
        }
    }

    fn release_wake_lock_now(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        tracing::info!(
            wake_lock = WAKE_LOCK_NAME,
            "soft-suspend writing wake_unlock"
        );
        if write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.held = false;
        }
    }

    fn schedule_unlock(&self, grace: Duration) {
        if grace.is_zero() {
            self.release_wake_lock_now();
            return;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let due_at = Instant::now() + grace;
        state.due_at = Some(due_at);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            grace_secs = grace.as_secs_f32(),
            "soft-suspend last holder released; scheduling unlock"
        );
        self.cv.notify_one();
    }

    fn cancel_pending_unlock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.due_at.take().is_some() {
            tracing::debug!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend pending unlock cancelled"
            );
        }
        self.cv.notify_one();
    }

    fn is_held(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).held
    }

    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        let should_unlock = state.held;
        state.shutdown = true;
        state.due_at = None;
        state.held = false;
        self.cv.notify_one();
        drop(state);
        if should_unlock {
            tracing::info!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend session teardown; unlocking"
            );
            write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
        }
    }
}

struct WakeLockArmer {
    state: Arc<Mutex<SessionState>>,
    unlock: Arc<UnlockInner>,
    leds: Option<Arc<dyn DeviceLeds>>,
}

impl WakeLockArmer {
    fn sync_led_awake(&self, state: &SessionState) {
        let Some(leds) = self.leds.as_ref() else {
            return;
        };
        if state.indicate_autosleep_led
            && state.mode.is_armed()
            && let Err(error) = leds.on()
        {
            tracing::warn!(error = %error, "failed to turn autosleep indicator LED on");
        }
    }

    fn turn_led_off(&self) {
        let Some(leds) = self.leds.as_ref() else {
            return;
        };
        if let Err(error) = leds.off() {
            tracing::warn!(error = %error, "failed to turn autosleep indicator LED off");
        }
    }

    fn schedule_unlock(&self, name: &LeaseName) {
        let grace = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .autosleep_grace;
        if grace.is_zero() {
            tracing::debug!(
                name = %name,
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend last holder released; unlocking immediately"
            );
        } else {
            tracing::debug!(
                name = %name,
                wake_lock = WAKE_LOCK_NAME,
                grace_secs = grace.as_secs_f32(),
                "soft-suspend last holder released; scheduling unlock"
            );
        }
        self.unlock.schedule_unlock(grace);
    }
}

impl Drop for WakeLockArmer {
    fn drop(&mut self) {
        self.unlock.shutdown();
    }
}

impl LeaseObserver for WakeLockArmer {
    fn on_first_acquire(&self, name: &LeaseName) {
        tracing::debug!(name = %name, wake_lock = WAKE_LOCK_NAME, "soft-suspend first holder");
        self.unlock.take_wake_lock();
        if let Ok(state) = self.state.lock() {
            self.sync_led_awake(&state);
        }
    }

    fn on_last_release(&self, name: &LeaseName) {
        self.schedule_unlock(name);
    }
}

/// Coordinates named soft-suspend leases, autosleep mode, and optional LED indicator.
pub struct SoftSuspendSession {
    tracker: LeaseTracker,
    state: Arc<Mutex<SessionState>>,
    paths: SoftSuspendPaths,
    armer: Arc<WakeLockArmer>,
}

/// RAII guard holding a soft-suspend lease.
#[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
pub struct SoftSuspendLease {
    inner: Option<Lease>,
}

impl SoftSuspendSession {
    /// Creates a session using system sysfs paths and optional LED controller.
    pub fn new(leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        Self::with_paths(SoftSuspendPaths::system(), leds)
    }

    /// Creates a session with injectable sysfs paths (tests / unavailable hosts).
    pub fn with_paths(paths: SoftSuspendPaths, leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        let available = paths.is_available();
        tracing::debug!(
            available,
            autosleep = %paths.autosleep.display(),
            "creating soft-suspend session"
        );
        let state = Arc::new(Mutex::new(SessionState {
            mode: AutosleepMode::Off,
            indicate_autosleep_led: false,
            autosleep_grace: Duration::ZERO,
        }));
        let unlock = UnlockInner::new(paths.clone());
        let armer = Arc::new(WakeLockArmer {
            state: Arc::clone(&state),
            unlock,
            leds,
        });
        Arc::new(Self {
            tracker: LeaseTracker::with_observer(Arc::clone(&armer) as Arc<dyn LeaseObserver>),
            state,
            paths,
            armer,
        })
    }

    /// Acquires a named soft-suspend lease (holds the `cadmus` wake lock while any exist).
    #[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name),
            fields(name = tracing::field::Empty, holders = tracing::field::Empty),
            level = tracing::Level::TRACE,
        )
    )]
    pub fn acquire(&self, name: impl Into<LeaseName>) -> SoftSuspendLease {
        let name = name.into();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("name", tracing::field::display(&name));
        self.armer.unlock.pin();
        let lease = self.tracker.acquire(name);
        self.armer.unlock.unpin();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("holders", self.tracker.len());
        SoftSuspendLease { inner: Some(lease) }
    }

    /// Runs `f` while holding a named soft-suspend lease.
    pub fn with<R>(&self, name: impl Into<LeaseName>, f: impl FnOnce() -> R) -> R {
        let _lease = self.acquire(name);
        f()
    }

    /// Returns current lease holder count.
    pub fn len(&self) -> usize {
        self.tracker.len()
    }

    /// Returns whether any soft-suspend leases are held.
    pub fn is_empty(&self) -> bool {
        self.tracker.is_empty()
    }

    /// Returns whether any soft-suspend leases are held.
    pub fn has_holders(&self) -> bool {
        !self.is_empty()
    }

    /// Returns the names of all active soft-suspend lease holders.
    pub fn holders(&self) -> Vec<crate::lease::LeaseName> {
        self.tracker.holders()
    }

    /// Returns the current autosleep mode.
    pub fn mode(&self) -> AutosleepMode {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).mode
    }

    /// Returns whether the status LED indicates armed soft suspend while awake.
    ///
    /// The LED tracks mode + this setting, not the `cadmus` wake lock. The kernel
    /// clears it on suspend; Cadmus turns it off only when mode is [`AutosleepMode::Off`]
    /// or this setting is disabled.
    pub fn indicate_autosleep_led(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .indicate_autosleep_led
    }

    /// Returns the delay after the last lease drops before writing `wake_unlock`.
    pub fn autosleep_grace(&self) -> Duration {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .autosleep_grace
    }

    /// Modes supported by this device (`Off` plus tokens from `/sys/power/state`).
    pub fn available_modes(&self) -> Vec<AutosleepMode> {
        discover_available_modes(&self.paths.state)
    }

    /// Sanitizes `mode` against discovery; unsupported values become [`AutosleepMode::Off`].
    pub fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode {
        if mode == AutosleepMode::Off {
            return AutosleepMode::Off;
        }
        if self.available_modes().contains(&mode) {
            mode
        } else {
            tracing::warn!(mode = %mode, "autosleep mode unsupported; falling back to off");
            AutosleepMode::Off
        }
    }

    /// Sets autosleep mode and writes `/sys/power/autosleep`.
    ///
    /// In-memory mode and LED policy update only after the sysfs write succeeds
    /// (or the node is missing — a no-op on hosts without autosleep).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(mode = %mode), level = tracing::Level::TRACE)
    )]
    pub fn set_mode(&self, mode: AutosleepMode) {
        let mode = self.sanitize_mode(mode);
        let value = mode.as_sysfs();
        match write_sysfs(&self.paths.autosleep, value) {
            Ok(SysfsWrite::Written) => {
                tracing::debug!(
                    path = %self.paths.autosleep.display(),
                    value,
                    "wrote soft-suspend sysfs"
                );
            }
            Ok(SysfsWrite::Missing) => {
                tracing::debug!(
                    path = %self.paths.autosleep.display(),
                    value,
                    "soft-suspend sysfs path missing"
                );
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    mode = %mode,
                    "soft-suspend autosleep write failed; keeping previous mode"
                );
                return;
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = state.mode;
        state.mode = mode;
        tracing::info!(previous = %previous, mode = %mode, "soft-suspend mode updated");
        let snapshot = SessionState {
            mode: state.mode,
            indicate_autosleep_led: state.indicate_autosleep_led,
            autosleep_grace: state.autosleep_grace,
        };
        let sync_awake = mode.is_armed() && snapshot.indicate_autosleep_led;
        let turn_off = !mode.is_armed() || !snapshot.indicate_autosleep_led;
        drop(state);
        if sync_awake {
            self.armer.sync_led_awake(&snapshot);
        } else if turn_off {
            self.armer.turn_led_off();
        }
    }

    /// Enables or disables the status LED while soft suspend is armed.
    ///
    /// Independent of lease holders: unlock does not clear the LED. The kernel
    /// clears it on suspend.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn set_indicate_autosleep_led(&self, enabled: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.indicate_autosleep_led = enabled;
        let mode = state.mode;
        let autosleep_grace = state.autosleep_grace;
        tracing::info!(enabled, mode = %mode, "soft-suspend LED indicator updated");
        drop(state);
        if enabled && mode.is_armed() {
            self.armer.sync_led_awake(&SessionState {
                mode,
                indicate_autosleep_led: true,
                autosleep_grace,
            });
        } else if !enabled {
            self.armer.turn_led_off();
        }
    }

    /// Sets how long to keep the wake lock after the last lease drops.
    ///
    /// Zero means unlock immediately. Changing grace cancels any pending unlock
    /// and, if the wake lock is still held with no leases, reschedules.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn set_autosleep_grace(&self, grace: Duration) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.autosleep_grace = grace;
        tracing::info!(
            grace_secs = grace.as_secs_f32(),
            "soft-suspend release grace updated"
        );
        drop(state);
        self.armer.unlock.cancel_pending_unlock();
        if self.tracker.is_empty() && self.armer.unlock.is_held() {
            self.armer.schedule_unlock(&LeaseName::from("grace-update"));
        }
    }

    /// Applies mode, LED policy, and release grace from settings (boot / settings change).
    pub fn apply_settings(
        &self,
        mode: AutosleepMode,
        indicate_autosleep_led: bool,
        autosleep_grace: Duration,
    ) {
        self.set_autosleep_grace(autosleep_grace);
        self.set_indicate_autosleep_led(indicate_autosleep_led);
        self.set_mode(mode);
    }
}

impl SoftSuspendLease {
    /// Returns whether this guard still holds the lease.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
}

impl Drop for SoftSuspendLease {
    fn drop(&mut self) {
        self.inner.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::leds::{DeviceLeds, LedsError};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingLeds {
        on_calls: AtomicU32,
        off_calls: AtomicU32,
    }

    impl DeviceLeds for CountingLeds {
        fn on(&self) -> Result<(), LedsError> {
            self.on_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn off(&self) -> Result<(), LedsError> {
            self.off_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn temp_paths() -> (tempfile::TempDir, SoftSuspendPaths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = SoftSuspendPaths {
            state: dir.path().join("state"),
            autosleep: dir.path().join("autosleep"),
            wake_lock: dir.path().join("wake_lock"),
            wake_unlock: dir.path().join("wake_unlock"),
        };
        fs::write(&paths.state, "freeze mem\n").expect("state");
        fs::write(&paths.autosleep, "off\n").expect("autosleep");
        fs::write(&paths.wake_lock, "").expect("wake_lock");
        fs::write(&paths.wake_unlock, "").expect("wake_unlock");
        (dir, paths)
    }

    fn unlock_name(paths: &SoftSuspendPaths) -> String {
        fs::read_to_string(&paths.wake_unlock)
            .expect("read")
            .trim()
            .to_string()
    }

    fn make_unwritable(path: &std::path::Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[test]
    fn first_acquire_writes_wake_lock() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);

        let lease = session.acquire("main-loop");

        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME
        );
        drop(lease);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn set_mode_writes_autosleep() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);

        session.set_mode(AutosleepMode::Freeze);

        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
        assert_eq!(session.mode(), AutosleepMode::Freeze);
    }

    #[test]
    fn set_mode_keeps_previous_when_autosleep_write_fails() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Freeze);
        assert_eq!(session.mode(), AutosleepMode::Freeze);

        make_unwritable(&paths.autosleep);
        session.set_mode(AutosleepMode::Mem);

        assert_eq!(session.mode(), AutosleepMode::Freeze);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
    }

    #[test]
    fn failed_wake_lock_write_does_not_claim_held() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_secs(60));
        make_unwritable(&paths.wake_lock);

        let lease = session.acquire("main-loop");
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            ""
        );
        drop(lease);
        fs::write(&paths.wake_unlock, "").expect("clear unlock");

        session.set_autosleep_grace(Duration::from_millis(30));
        thread::sleep(Duration::from_millis(80));
        assert_eq!(
            unlock_name(&paths),
            "",
            "failed wake_lock must leave held=false so grace updates do not unlock"
        );
    }

    #[test]
    fn unsupported_mode_falls_back_to_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = SoftSuspendPaths {
            state: dir.path().join("state"),
            autosleep: dir.path().join("autosleep"),
            wake_lock: dir.path().join("wake_lock"),
            wake_unlock: dir.path().join("wake_unlock"),
        };
        fs::write(&paths.state, "mem\n").expect("state");
        fs::write(&paths.autosleep, "off\n").expect("autosleep");
        fs::write(&paths.wake_lock, "").expect("wake_lock");
        fs::write(&paths.wake_unlock, "").expect("wake_unlock");
        let session = SoftSuspendSession::with_paths(paths.clone(), None);

        session.set_mode(AutosleepMode::Freeze);

        assert_eq!(session.mode(), AutosleepMode::Off);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "off"
        );
    }

    #[test]
    fn led_indicator_on_while_armed_when_enabled() {
        let (_dir, paths) = temp_paths();
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let session = SoftSuspendSession::with_paths(
            paths.clone(),
            Some(leds.clone() as Arc<dyn DeviceLeds>),
        );
        session.apply_settings(AutosleepMode::Mem, true, Duration::ZERO);

        assert_eq!(leds.on_calls.load(Ordering::SeqCst), 1);
        assert!(session.is_empty());

        let lease = session.acquire("wifi");
        drop(lease);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
        assert_eq!(leds.off_calls.load(Ordering::SeqCst), 0);

        session.set_indicate_autosleep_led(false);
        assert!(leds.off_calls.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn nested_leases_keep_single_wake_lock_cycle() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);

        let a = session.acquire("a");
        let b = session.acquire("b");
        assert_eq!(session.len(), 2);
        drop(a);
        assert_eq!(unlock_name(&paths), "");
        drop(b);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn grace_delays_wake_unlock() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(80));

        let lease = session.acquire("main-loop");
        drop(lease);

        assert_eq!(unlock_name(&paths), "");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn lease_during_grace_cancels_pending_unlock() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(80));

        let lease = session.acquire("main-loop");
        drop(lease);
        let _again = session.acquire("main-loop");
        thread::sleep(Duration::from_millis(120));

        assert_eq!(unlock_name(&paths), "");
        assert!(session.has_holders());
    }

    #[test]
    fn reacquire_from_other_thread_during_grace_keeps_lock() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(100));

        drop(session.acquire("main-loop"));

        let session_thread = Arc::clone(&session);
        let join = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            session_thread.acquire("worker")
        });
        let lease = join.join().expect("acquire thread");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(unlock_name(&paths), "");
        assert!(session.has_holders());
        drop(lease);
    }

    #[test]
    fn set_grace_while_empty_reschedules_deadline() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(50));

        let lease = session.acquire("main-loop");
        drop(lease);
        session.set_autosleep_grace(Duration::from_millis(150));

        thread::sleep(Duration::from_millis(80));
        assert_eq!(unlock_name(&paths), "");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn repeated_empty_cycles_reuse_worker() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(40));

        for _ in 0..2 {
            fs::write(&paths.wake_unlock, "").expect("clear unlock");
            let lease = session.acquire("main-loop");
            drop(lease);
            assert_eq!(unlock_name(&paths), "");
            thread::sleep(Duration::from_millis(80));
            assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
        }
    }

    #[test]
    fn drop_session_mid_grace_shuts_down_cleanly() {
        let (_dir, paths) = temp_paths();
        let session = SoftSuspendSession::with_paths(paths.clone(), None);
        session.set_mode(AutosleepMode::Mem);
        session.set_autosleep_grace(Duration::from_millis(200));

        let lease = session.acquire("main-loop");
        drop(lease);
        drop(session);

        thread::sleep(Duration::from_millis(50));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }
}
