//! WiFi session: named leases over [`LeaseTracker`] plus radio bring-up.

use crate::device::inhibitor::{Inhibitor, InhibitorGuard, Kind, SoftSuspendName};
use crate::device::wifi::{WifiError, WifiManager};
use crate::input::DeviceEvent;
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker, WeakLeaseTracker};
use crate::settings::WifiMode;
use crate::view::{Event, Hub};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Default wait for association / DHCP after enabling the radio.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors from [`WifiSession::acquire`].
#[derive(Error, Debug)]
pub enum WifiSessionError {
    /// WiFi mode is [`WifiMode::Off`].
    #[error("WiFi is turned off")]
    ModeOff,

    /// Underlying radio operation failed.
    #[error(transparent)]
    Wifi(#[from] WifiError),

    /// Timed out waiting for the network to come up.
    #[error("timed out waiting for WiFi to come online")]
    Timeout,

    /// Internal lock poisoned.
    #[error("WiFi session lock poisoned")]
    Lock,
}

struct SessionState {
    mode: WifiMode,
    online: bool,
    /// Whether the radio was last successfully enabled (cleared by [`WifiSession::disable_radio`]).
    radio_on: bool,
    idle_since: Option<Instant>,
    idle_wake: Option<Sender<()>>,
    hub: Option<Hub>,
    inhibitor: Option<Arc<Inhibitor>>,
    inhibitor_lease: Option<InhibitorGuard>,
}

struct IdleArmer {
    state: Arc<Mutex<SessionState>>,
    tracker: OnceLock<WeakLeaseTracker>,
}

impl IdleArmer {
    fn has_holders(&self) -> bool {
        self.tracker
            .get()
            .is_some_and(|tracker| !tracker.is_empty())
    }
}

fn sync_inhibitor_lease(state: &mut SessionState, has_holders: bool) {
    let should_hold = state.radio_on && (state.mode == WifiMode::AlwaysOn || has_holders);
    if should_hold {
        if state.inhibitor_lease.is_none()
            && let Some(inhibitor) = state.inhibitor.clone()
        {
            match inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Wifi) {
                Ok(guard) => state.inhibitor_lease = Some(guard),
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        soft_suspend_lease = %SoftSuspendName::Wifi,
                        "failed to acquire soft-suspend lease for WiFi radio"
                    );
                }
            }
        }
    } else {
        state.inhibitor_lease = None;
    }
}

impl LeaseObserver for IdleArmer {
    fn on_first_acquire(&self, name: &LeaseName) {
        tracing::debug!(name = %name, "wifi lease first holder");
        if let Ok(mut state) = self.state.lock() {
            state.idle_since = None;
            sync_inhibitor_lease(&mut state, self.has_holders());
        }
    }

    fn on_last_release(&self, name: &LeaseName) {
        tracing::debug!(name = %name, "wifi lease last holder released");
        if let Ok(mut state) = self.state.lock() {
            let has_holders = self.has_holders();
            sync_inhibitor_lease(&mut state, has_holders);
            if !has_holders && state.mode == WifiMode::Auto {
                state.idle_since = Some(Instant::now());
                if let Some(idle_wake) = state.idle_wake.as_ref() {
                    idle_wake.send(()).ok();
                }
            }
        }
    }
}

/// Coordinates named WiFi leases, radio power, and online waiters.
pub struct WifiSession {
    tracker: LeaseTracker,
    wifi: Arc<dyn WifiManager>,
    state: Arc<Mutex<SessionState>>,
    online_cv: Condvar,
}

/// Fallback manager when the device cannot provide WiFi.
struct UnavailableWifi;

impl WifiManager for UnavailableWifi {
    fn enable(&self) -> Result<(), WifiError> {
        Err(WifiError::Disabled)
    }

    fn disable(&self) -> Result<(), WifiError> {
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        false
    }

    fn network_info(&self) -> Result<Option<crate::device::wifi::NetworkInfo>, WifiError> {
        Err(WifiError::Disabled)
    }
}

impl WifiSession {
    /// Creates a session that cannot enable WiFi (device has no manager).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(fields(mode = %mode), level = tracing::Level::TRACE)
    )]
    pub fn unavailable(mode: WifiMode) -> Arc<Self> {
        tracing::debug!(mode = %mode, "creating unavailable wifi session");
        Self::new(Arc::new(UnavailableWifi), mode)
    }

    /// Creates a session wrapping `wifi`, starting in `mode`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(wifi), fields(mode = %mode), level = tracing::Level::TRACE)
    )]
    pub fn new(wifi: Arc<dyn WifiManager>, mode: WifiMode) -> Arc<Self> {
        tracing::debug!(mode = %mode, "creating wifi session");
        let state = Arc::new(Mutex::new(SessionState {
            mode,
            online: false,
            radio_on: false,
            idle_since: None,
            idle_wake: None,
            hub: None,
            inhibitor: None,
            inhibitor_lease: None,
        }));
        let observer = Arc::new(IdleArmer {
            state: Arc::clone(&state),
            tracker: OnceLock::new(),
        });
        let tracker = LeaseTracker::with_observer(observer.clone());
        observer
            .tracker
            .set(tracker.downgrade())
            .expect("wifi IdleArmer tracker already set");
        Arc::new(Self {
            tracker,
            wifi,
            state,
            online_cv: Condvar::new(),
        })
    }

    /// Wakes the idle poller so an idle-disable check can run immediately.
    pub fn set_idle_wake_sender(&self, sender: Sender<()>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .idle_wake = Some(sender);
    }

    /// Stores the app event hub for emitting device events from lease paths.
    pub fn set_hub(&self, hub: Hub) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).hub = Some(hub);
    }

    /// Links the inhibitor so AlwaysOn or WiFi holders keep SoftSuspend armed.
    pub fn set_inhibitor(&self, inhibitor: Arc<Inhibitor>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inhibitor = Some(inhibitor);
        sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
    }

    /// Updates the configured WiFi mode (from settings).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(mode = %mode), level = tracing::Level::TRACE)
    )]
    pub fn set_mode(&self, mode: WifiMode) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = state.mode;
        state.mode = mode;
        if mode != WifiMode::Auto {
            state.idle_since = None;
        } else if self.tracker.is_empty() && state.online {
            state.idle_since = Some(Instant::now());
        }
        sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
        tracing::debug!(
            previous = %previous,
            mode = %mode,
            online = state.online,
            holders = self.tracker.len(),
            "wifi mode updated"
        );
    }

    /// Returns the current mode snapshot.
    pub fn mode(&self) -> WifiMode {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).mode
    }

    /// Returns whether the session believes the network is up.
    pub fn is_online(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).online
    }

    /// Called when DHCP / association reports the network is up.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn notify_online(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.online = true;
            if state.mode == WifiMode::Auto && self.tracker.is_empty() {
                state.idle_since = Some(Instant::now());
            } else {
                state.idle_since = None;
            }
            tracing::info!(
                mode = %state.mode,
                holders = self.tracker.len(),
                idle = state.idle_since.is_some(),
                "wifi session online"
            );
        }
        self.online_cv.notify_all();
    }

    /// Marks the session offline without clearing the idle timer (pending disable).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn mark_offline_pending(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.online = false;
            tracing::info!(
                mode = %state.mode,
                holders = self.tracker.len(),
                "wifi session marked offline pending disable"
            );
        }
        self.online_cv.notify_all();
    }

    /// Marks the session offline (after disable / suspend).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn notify_offline(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.online = false;
        state.idle_since = None;
        tracing::info!(
            mode = %state.mode,
            holders = self.tracker.len(),
            "wifi session offline"
        );
    }

    /// Returns named holders currently leasing WiFi.
    pub fn holders(&self) -> Vec<LeaseName> {
        self.tracker.holders()
    }

    /// Returns whether any lease is held.
    pub fn has_holders(&self) -> bool {
        !self.tracker.is_empty()
    }

    /// Instant when idle started after the last Auto-mode lease dropped.
    pub fn idle_since(&self) -> Option<Instant> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .idle_since
    }

    /// Clears the idle deadline (e.g. after disabling or leaving Auto).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub fn clear_idle(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let had_idle = state.idle_since.take().is_some();
        if had_idle {
            tracing::debug!(mode = %state.mode, "wifi idle deadline cleared");
        }
    }

    /// Acquires a named lease with [`DEFAULT_ACQUIRE_TIMEOUT`].
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name),
            fields(name = tracing::field::Empty),
            err,
            level = tracing::Level::TRACE,
        )
    )]
    pub fn acquire(&self, name: impl Into<LeaseName>) -> Result<WifiLease, WifiSessionError> {
        let name = name.into();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("name", tracing::field::display(&name));
        self.acquire_with_timeout(name, DEFAULT_ACQUIRE_TIMEOUT)
    }

    /// Acquires a named lease, enabling WiFi and waiting until online if needed.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name),
            fields(
                name = tracing::field::Empty,
                mode = tracing::field::Empty,
                already_online = tracing::field::Empty,
            ),
            err,
            level = tracing::Level::TRACE,
        )
    )]
    pub fn acquire_with_timeout(
        &self,
        name: impl Into<LeaseName>,
        timeout: Duration,
    ) -> Result<WifiLease, WifiSessionError> {
        let name = name.into();
        let mode = self.mode();
        #[cfg(feature = "tracing")]
        {
            tracing::Span::current().record("name", tracing::field::display(&name));
            tracing::Span::current().record("mode", tracing::field::display(&mode));
        }

        if mode == WifiMode::Off {
            tracing::debug!(name = %name, "wifi acquire rejected: mode off");
            return Err(WifiSessionError::ModeOff);
        }

        tracing::debug!(
            name = %name,
            mode = %mode,
            timeout_secs = timeout.as_secs_f32(),
            "wifi lease acquire started"
        );

        let inner = self.tracker.acquire(name.clone());
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
        }

        if self.is_online() {
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("already_online", true);
            tracing::debug!(name = %name, "wifi lease acquired while online");
            return Ok(WifiLease { inner: Some(inner) });
        }

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("already_online", false);

        if !self.wifi.is_enabled() {
            tracing::info!(name = %name, "enabling wifi radio for lease");
            if let Err(error) = self.wifi.enable() {
                tracing::error!(name = %name, error = %error, "failed to enable wifi radio");
                return Err(error.into());
            }
        }

        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.radio_on = true;
            sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
        }

        if matches!(self.wifi.network_info(), Ok(Some(_))) {
            tracing::debug!(name = %name, "wifi lease acquired; already associated");
            self.notify_online();
            if let Some(hub) = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hub
                .as_ref()
            {
                hub.send((Event::Device(DeviceEvent::NetUp)).into()).ok();
            }
            return Ok(WifiLease { inner: Some(inner) });
        }

        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().map_err(|_| WifiSessionError::Lock)?;
        while !state.online {
            let now = Instant::now();
            if now >= deadline {
                tracing::warn!(
                    name = %name,
                    timeout_secs = timeout.as_secs_f32(),
                    "timed out waiting for wifi online"
                );
                drop(state);
                drop(inner);
                return Err(WifiSessionError::Timeout);
            }
            let wait = deadline - now;
            let (guard, wait_result) = self
                .online_cv
                .wait_timeout(state, wait)
                .map_err(|_| WifiSessionError::Lock)?;
            state = guard;
            if wait_result.timed_out() && !state.online {
                tracing::warn!(
                    name = %name,
                    timeout_secs = timeout.as_secs_f32(),
                    "timed out waiting for wifi online"
                );
                drop(state);
                drop(inner);
                return Err(WifiSessionError::Timeout);
            }
        }

        tracing::debug!(name = %name, "wifi lease acquired after wait");
        Ok(WifiLease { inner: Some(inner) })
    }

    /// Runs `f` while holding a named WiFi lease.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name, f),
            fields(name = tracing::field::Empty),
            err,
            level = tracing::Level::TRACE,
        )
    )]
    pub fn with<R>(
        &self,
        name: impl Into<LeaseName>,
        f: impl FnOnce() -> R,
    ) -> Result<R, WifiSessionError> {
        let name = name.into();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("name", tracing::field::display(&name));
        let _lease = self.acquire(name)?;
        Ok(f())
    }

    /// Enables the radio without taking a lease (AlwaysOn / resume).
    ///
    /// Returns `Ok(true)` when the radio is enabled and
    /// [`WifiManager::network_info`] already reports an association (so the
    /// caller can emit [`crate::input::DeviceEvent::NetUp`] without waiting for
    /// a dhcpcd signal).
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), err, level = tracing::Level::TRACE)
    )]
    pub fn enable_radio(&self) -> Result<bool, WifiError> {
        tracing::info!("enabling wifi radio");
        match self.wifi.enable() {
            Ok(()) => {
                {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.radio_on = true;
                    sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
                }
                let connected =
                    self.wifi.is_enabled() && matches!(self.wifi.network_info(), Ok(Some(_)));
                tracing::debug!(connected, "wifi radio enabled");
                if connected {
                    self.notify_online();
                }
                Ok(connected)
            }
            Err(error) => {
                tracing::error!(error = %error, "failed to enable wifi radio");
                Err(error)
            }
        }
    }

    /// Disables the radio and marks the session offline.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), err, level = tracing::Level::TRACE)
    )]
    pub fn disable_radio(&self) -> Result<(), WifiError> {
        tracing::info!("disabling wifi radio");
        let result = self.wifi.disable();
        if let Err(error) = &result {
            tracing::error!(error = %error, "failed to disable wifi radio");
        } else {
            tracing::debug!("wifi radio disabled");
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.radio_on = false;
                sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
            }
            self.notify_offline();
            self.clear_idle();
        }
        result
    }

    /// Returns the underlying manager (tests / direct queries).
    pub fn wifi_manager(&self) -> &Arc<dyn WifiManager> {
        &self.wifi
    }
}

/// RAII guard that keeps a WiFi lease (and thus the radio demand) alive.
#[must_use = "WiFi lease is released immediately if unused"]
#[derive(Debug)]
pub struct WifiLease {
    inner: Option<Lease>,
}

impl Drop for WifiLease {
    fn drop(&mut self) {
        self.inner.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::inhibitor::Inhibitor;
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use crate::device::test_device::TestWifiManager;
    use std::thread;

    fn session(mode: WifiMode) -> (Arc<WifiSession>, Arc<TestWifiManager>) {
        let wifi = Arc::new(TestWifiManager::new());
        let session = WifiSession::new(wifi.clone(), mode);
        (session, wifi)
    }

    #[test]
    fn off_rejects_acquire() {
        let (session, _) = session(WifiMode::Off);
        let err = session.acquire("x").unwrap_err();
        assert!(matches!(err, WifiSessionError::ModeOff));
    }

    #[test]
    fn acquire_when_already_online() {
        let (session, _) = session(WifiMode::Auto);
        session.notify_online();
        let lease = session.acquire("a").unwrap();
        assert!(session.has_holders());
        drop(lease);
        assert!(!session.has_holders());
        assert!(session.idle_since().is_some());
    }

    #[test]
    fn two_holders_idle_only_after_last() {
        let (session, _) = session(WifiMode::Auto);
        session.notify_online();
        let a = session.acquire("a").unwrap();
        let b = session.acquire("b").unwrap();
        drop(a);
        assert!(session.idle_since().is_none());
        drop(b);
        assert!(session.idle_since().is_some());
    }

    #[test]
    fn always_on_does_not_arm_idle() {
        let (session, _) = session(WifiMode::AlwaysOn);
        session.notify_online();
        let lease = session.acquire("a").unwrap();
        drop(lease);
        assert!(session.idle_since().is_none());
    }

    #[test]
    fn notify_online_unblocks_waiter() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(None));
        let session2 = Arc::clone(&session);
        let handle =
            thread::spawn(move || session2.acquire_with_timeout("wait", Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(50));
        session.notify_online();
        let lease = handle.join().unwrap().unwrap();
        drop(lease);
    }

    #[test]
    fn acquire_enables_radio_when_disabled() {
        let (session, wifi) = session(WifiMode::Auto);
        assert!(!wifi.is_enabled());
        let session2 = Arc::clone(&session);
        let handle =
            thread::spawn(move || session2.acquire_with_timeout("en", Duration::from_millis(200)));
        thread::sleep(Duration::from_millis(30));
        assert!(wifi.is_enabled() || handle.is_finished());
        session.notify_online();
        let _ = handle.join().unwrap();
    }

    #[test]
    fn acquire_timeout_releases_lease_without_deadlock() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(None));
        let err = session
            .acquire_with_timeout("t", Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(err, WifiSessionError::Timeout));
        assert!(!session.has_holders());
        assert!(session.idle_since().is_some());
    }

    #[test]
    fn enable_radio_reports_connected_when_associated() {
        let (session, wifi) = session(WifiMode::AlwaysOn);
        wifi.set_network_info(Ok(Some(crate::device::wifi::NetworkInfo {
            ip: "192.168.1.1".parse().unwrap(),
            essid: crate::device::wifi::Essid::new("test"),
        })));
        assert!(session.enable_radio().unwrap());
        assert!(session.is_online());
    }

    #[test]
    fn acquire_skips_wait_when_already_associated() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(Some(crate::device::wifi::NetworkInfo {
            ip: "192.168.1.1".parse().unwrap(),
            essid: crate::device::wifi::Essid::new("test"),
        })));
        let lease = session
            .acquire_with_timeout("fast", Duration::from_millis(50))
            .unwrap();
        assert!(session.is_online());
        drop(lease);
    }

    fn soft_suspend_inhibitor() -> (tempfile::TempDir, Arc<Inhibitor>) {
        use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
        let (dir, paths) = SoftSuspendPaths::test_fixture();
        let inhibitor = Inhibitor::with_paths(
            paths,
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        (dir, inhibitor)
    }

    #[test]
    fn always_on_holds_soft_suspend_only_while_radio_on() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        assert!(soft.is_empty());

        session.set_mode(WifiMode::AlwaysOn);
        assert!(
            soft.is_empty(),
            "AlwaysOn without radio must not pin soft-suspend"
        );

        session.enable_radio().unwrap();
        assert!(!soft.is_empty());

        session.disable_radio().unwrap();
        assert!(
            soft.is_empty(),
            "disable_radio must drop soft-suspend wifi lease"
        );

        session.set_mode(WifiMode::Off);
        assert!(soft.is_empty());
    }

    #[test]
    fn always_on_keeps_soft_suspend_after_last_wifi_holder() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().unwrap();
        session.notify_online();
        assert!(!soft.is_empty());

        let lease = session.acquire("a").unwrap();
        drop(lease);
        assert!(!soft.is_empty());
        assert!(!session.has_holders());
    }

    #[test]
    fn leaving_always_on_keeps_soft_suspend_while_holders_remain() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().unwrap();
        session.notify_online();
        let lease = session.acquire("a").unwrap();

        session.set_mode(WifiMode::Auto);
        assert!(!soft.is_empty());

        drop(lease);
        assert!(soft.is_empty());
    }

    #[test]
    fn disable_radio_drops_always_on_soft_suspend_lease() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().unwrap();
        assert!(!soft.is_empty());

        session.disable_radio().unwrap();
        assert!(soft.is_empty());
        assert_eq!(session.mode(), WifiMode::AlwaysOn);

        session.enable_radio().unwrap();
        assert!(!soft.is_empty());
    }

    #[test]
    fn auto_holder_pins_soft_suspend_while_radio_on() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().unwrap();
        session.notify_online();

        let lease = session.acquire("ntp").unwrap();
        assert!(
            !soft.is_empty(),
            "Auto-mode WiFi holder must pin soft-suspend while radio is on"
        );
        drop(lease);
        assert!(soft.is_empty());
    }

    #[test]
    fn soft_suspend_stays_pinned_while_holder_active_under_churn() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().unwrap();
        session.notify_online();

        let failed = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let session = Arc::clone(&session);
                let soft = Arc::clone(&soft);
                let failed = Arc::clone(&failed);
                thread::spawn(move || {
                    for n in 0..250 {
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let lease = session.acquire(format!("t{i}-{n}")).unwrap();
                        if session.has_holders() && soft.is_empty() {
                            failed.store(true, Ordering::Relaxed);
                            break;
                        }
                        drop(lease);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("churn thread panicked");
        }
        assert!(
            !failed.load(Ordering::Relaxed),
            "soft-suspend wifi lease dropped while a WiFi holder was still active"
        );
    }
}
