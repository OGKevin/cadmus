//! WiFi session: named leases over [`LeaseTracker`] plus radio bring-up.

use crate::device::inhibitor::{Inhibitor, InhibitorGuard, Kind, SoftSuspendName};
use crate::device::wifi::{WifiError, WifiManager};
use crate::input::DeviceEvent;
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker, WeakLeaseTracker};
use crate::settings::WifiMode;
use crate::view::{Event, Hub};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Notify;

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
    /// Fired when the last lease is released into idle, so the idle-disable
    /// check can run without waiting for the next poll tick.
    idle_wake: Arc<Notify>,
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
                state.idle_wake.notify_one();
            }
        }
    }
}

/// Coordinates named WiFi leases, radio power, and online waiters.
pub struct WifiSession {
    tracker: LeaseTracker,
    wifi: Arc<dyn WifiManager>,
    state: Arc<Mutex<SessionState>>,
    /// Wakes [`WifiSession::acquire`] waiters to re-read [`SessionState::online`].
    ///
    /// `notify_waiters` runs after online flips to `true` ([`Self::notify_online`],
    /// enable bookkeeping) and after it flips to `false` while waiters may still
    /// be parked ([`Self::mark_offline_pending`], disable bookkeeping). Waiters
    /// loop until `online` is `true` or their acquire timeout expires; the notify
    /// is not an "online only" signal, it means session connectivity state changed.
    ///
    /// Tokio [`Notify`] replaces the old `std::sync::Condvar`: waiters
    /// `.await` without parking an OS thread, and `notify_waiters` fans
    /// out to every pending acquire. A Condvar still needs a Mutex and
    /// a blocking wait, which does not fit the async lease path.
    online: Arc<Notify>,
    /// Serialises radio transitions.
    ///
    /// Enable and disable can be requested from different tasks (a resume
    /// spawn and a suspend path, say). Without this, a disable can interleave
    /// with an enable and leave the interface up with no `wpa_supplicant`,
    /// after which `is_enabled()` reports true and every lease times out.
    ///
    /// Shared so the *spawned* transition can hold it. A caller-held guard
    /// would be dropped when that caller is cancelled, releasing the lock while
    /// the hardware sequence is still mid-flight.
    radio: Arc<tokio::sync::Mutex<()>>,
}

/// Applies [`SessionState::desired_radio_on`] under the radio lock.
///
/// Bookkeeping runs inside the detached task so a cancelled caller cannot skip
/// `radio_on` / online state updates while the hardware transition still
/// finishes. The desired flag is re-read after acquiring the lock so a quick
/// enable→disable sequence applies the latest intent, not spawn order.
async fn apply_desired_radio_power(session: &WifiSession) -> Result<(), WifiError> {
    let radio = Arc::clone(&session.radio);
    let state = Arc::clone(&session.state);
    let tracker = session.tracker.clone();
    let wifi = Arc::clone(&session.wifi);
    let online = session.online.clone();
    let task = tokio::spawn(async move {
        let _held = radio.lock().await;
        let desired = state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .desired_radio_on;
        let result = if desired {
            wifi.enable().await
        } else {
            wifi.disable().await
        };
        if result.is_ok() {
            if desired {
                finish_enable_bookkeeping(&state, &tracker, &wifi, &online).await;
            } else {
                finish_disable_bookkeeping(&state, &tracker, &online);
            }
        }
        result
    });
    match task.await {
        Ok(result) => result,
        Err(join) => Err(WifiError::Ioctl(join.to_string())),
    }
}

/// Session bookkeeping after a successful radio enable.
///
/// When the manager already reports an association, sets `online` and wakes
/// [`WifiSession::acquire`] loops awaiting [`Notify::notified`].
async fn finish_enable_bookkeeping(
    state: &Arc<Mutex<SessionState>>,
    tracker: &LeaseTracker,
    wifi: &Arc<dyn WifiManager>,
    online: &Arc<Notify>,
) {
    {
        let mut locked = state.lock().unwrap_or_else(|e| e.into_inner());
        locked.radio_on = true;
        sync_inhibitor_lease(&mut locked, !tracker.is_empty());
    }
    if wifi.is_enabled().await && matches!(wifi.network_info().await, Ok(Some(_))) {
        {
            let mut locked = state.lock().unwrap_or_else(|e| e.into_inner());
            locked.online = true;
            if locked.mode == WifiMode::Auto && tracker.is_empty() {
                locked.idle_since = Some(Instant::now());
            } else {
                locked.idle_since = None;
            }
        }
        online.notify_waiters();
    }
}

fn finish_disable_bookkeeping(
    state: &Arc<Mutex<SessionState>>,
    tracker: &LeaseTracker,
    online: &Arc<Notify>,
) {
    {
        let mut locked = state.lock().unwrap_or_else(|e| e.into_inner());
        locked.radio_on = false;
        sync_inhibitor_lease(&mut locked, !tracker.is_empty());
        locked.online = false;
        locked.idle_since = None;
    }
    online.notify_waiters();
}

/// Fallback manager when the device cannot provide WiFi.
struct UnavailableWifi;

#[async_trait::async_trait]
impl WifiManager for UnavailableWifi {
    async fn enable(&self) -> Result<(), WifiError> {
        Err(WifiError::Disabled)
    }

    async fn disable(&self) -> Result<(), WifiError> {
        Ok(())
    }

    async fn is_enabled(&self) -> bool {
        false
    }

    async fn network_info(&self) -> Result<Option<crate::device::wifi::NetworkInfo>, WifiError> {
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
            idle_wake: Arc::new(Notify::new()),
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
            online: Arc::new(Notify::new()),
            radio: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Handle that fires when the last lease is released into idle, so the
    /// Kobo idle poller can check without waiting for the next tick.
    ///
    /// Returns the shared [`Notify`] rather than a future so a repeating poller
    /// can re-arm it each pass; [`Notify::notified`] borrows, and a resolved
    /// future cannot be reused.
    #[cfg(feature = "kobo")]
    pub(crate) fn idle_wake_notify(&self) -> Arc<Notify> {
        Arc::clone(
            &self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .idle_wake,
        )
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
        self.online.notify_waiters();
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
        self.online.notify_waiters();
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

    /// Whether the radio is up, according to this session's own bookkeeping.
    ///
    /// Read without touching the device, so it is safe from the synchronous
    /// event handlers. Use [`WifiManager::is_enabled`] when a fresh probe of the
    /// kernel module and interface is required.
    pub fn is_radio_on(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .radio_on
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
    pub async fn acquire(&self, name: impl Into<LeaseName>) -> Result<WifiLease, WifiSessionError> {
        let name = name.into();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("name", tracing::field::display(&name));
        self.acquire_with_timeout(name, DEFAULT_ACQUIRE_TIMEOUT)
            .await
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
    pub async fn acquire_with_timeout(
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

        {
            let mut locked = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if locked.mode != WifiMode::Off {
                locked.desired_radio_on = true;
            }
        }
        if !self.is_radio_on() {
            tracing::info!(name = %name, "enabling wifi radio for lease");
            if let Err(error) = apply_desired_radio_power(self).await {
                tracing::error!(name = %name, error = %error, "failed to enable wifi radio");
                return Err(error.into());
            }
        }

        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.radio_on = true;
            sync_inhibitor_lease(&mut state, !self.tracker.is_empty());
        }

        if matches!(self.wifi.network_info().await, Ok(Some(_))) {
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
        loop {
            let notified = self.online.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .state
                .lock()
                .map_err(|_| WifiSessionError::Lock)?
                .online
            {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timed_out =
                remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err();
            let online = self
                .state
                .lock()
                .map_err(|_| WifiSessionError::Lock)?
                .online;
            if timed_out && !online {
                tracing::warn!(
                    name = %name,
                    timeout_secs = timeout.as_secs_f32(),
                    "timed out waiting for wifi online"
                );
                drop(inner);
                return Err(WifiSessionError::Timeout);
            }
        }

        tracing::debug!(name = %name, "wifi lease acquired after wait");
        Ok(WifiLease { inner: Some(inner) })
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
    pub async fn enable_radio(&self) -> Result<bool, WifiError> {
        tracing::info!("enabling wifi radio");
        {
            let mut locked = self.state.lock().unwrap_or_else(|e| e.into_inner());
            locked.desired_radio_on = true;
        }
        let enabled = apply_desired_radio_power(self).await;
        match enabled {
            Ok(()) => {
                let connected = self.wifi.is_enabled().await
                    && matches!(self.wifi.network_info().await, Ok(Some(_)));
                tracing::debug!(connected, "wifi radio enabled");
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
    pub async fn disable_radio(&self) -> Result<(), WifiError> {
        tracing::info!("disabling wifi radio");
        {
            let mut locked = self.state.lock().unwrap_or_else(|e| e.into_inner());
            locked.desired_radio_on = false;
        }
        let result = apply_desired_radio_power(self).await;
        if let Err(error) = &result {
            tracing::error!(error = %error, "failed to disable wifi radio");
        } else {
            tracing::debug!("wifi radio disabled");
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

    /// A manager that finishes `enable` only after `release` is signalled, so a
    /// test can drop the caller while the sequence is still in flight.
    struct SlowWifi {
        finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
        gate: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl WifiManager for SlowWifi {
        async fn enable(&self) -> Result<(), WifiError> {
            self.gate.notified().await;
            self.finished
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn disable(&self) -> Result<(), WifiError> {
            Ok(())
        }

        async fn is_enabled(&self) -> bool {
            self.finished.load(std::sync::atomic::Ordering::SeqCst)
        }

        async fn network_info(
            &self,
        ) -> Result<Option<crate::device::wifi::NetworkInfo>, WifiError> {
            Ok(None)
        }
    }

    /// Slow disable used to test bookkeeping when the caller is cancelled.
    struct SlowDisableWifi {
        gate: tokio::sync::Notify,
        enabled: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl WifiManager for SlowDisableWifi {
        async fn enable(&self) -> Result<(), WifiError> {
            self.enabled
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn disable(&self) -> Result<(), WifiError> {
            self.gate.notified().await;
            self.enabled
                .store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn is_enabled(&self) -> bool {
            self.enabled.load(std::sync::atomic::Ordering::SeqCst)
        }

        async fn network_info(
            &self,
        ) -> Result<Option<crate::device::wifi::NetworkInfo>, WifiError> {
            Ok(None)
        }
    }

    /// A cancelled disable caller must still clear online/radio bookkeeping once
    /// the detached hardware transition finishes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_disable_still_updates_session_state() {
        let wifi = Arc::new(SlowDisableWifi {
            gate: tokio::sync::Notify::new(),
            enabled: std::sync::atomic::AtomicBool::new(true),
        });
        let session = WifiSession::new(wifi.clone(), WifiMode::AlwaysOn);
        session.enable_radio().await.expect("enable radio");
        session.notify_online();
        assert!(session.is_online());
        assert!(session.is_radio_on());

        let caller = {
            let session = Arc::clone(&session);
            tokio::spawn(async move { session.disable_radio().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        caller.abort();
        let _ = caller.await;
        wifi.gate.notify_one();

        for _ in 0..200 {
            if !session.is_online() && !session.is_radio_on() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("session state must reflect a completed disable after caller cancellation");
    }

    /// A dropped caller must detach the enable sequence, not abort it halfway.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_caller_does_not_abort_the_enable_sequence() {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wifi = Arc::new(SlowWifi {
            finished: std::sync::Arc::clone(&finished),
            gate: tokio::sync::Notify::new(),
        });
        let session = WifiSession::new(wifi.clone(), WifiMode::AlwaysOn);

        let caller = {
            let session = Arc::clone(&session);
            tokio::spawn(async move { session.enable_radio().await })
        };
        // Let the spawned sequence reach its gate, then drop the caller.
        tokio::time::sleep(Duration::from_millis(20)).await;
        caller.abort();
        let _ = caller.await;

        wifi.gate.notify_one();
        for _ in 0..200 {
            if finished.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            finished.load(std::sync::atomic::Ordering::SeqCst),
            "the enable sequence must complete even though its caller was dropped"
        );
    }

    /// A caller-held lock would be released when that caller is dropped, letting
    /// a disable interleave with an enable that is still mid-sequence. The lock
    /// must belong to the sequence, not the caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_caller_does_not_release_the_transition_lock() {
        use std::sync::atomic::AtomicBool;

        let finished = Arc::new(AtomicBool::new(false));
        let wifi = Arc::new(SlowWifi {
            finished: Arc::clone(&finished),
            gate: tokio::sync::Notify::new(),
        });
        let session = WifiSession::new(wifi.clone(), WifiMode::AlwaysOn);

        let caller = {
            let session = Arc::clone(&session);
            tokio::spawn(async move { session.enable_radio().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        caller.abort();
        let _ = caller.await;

        // The detached enable still holds the lock, so a disable must wait.
        let disable = {
            let session = Arc::clone(&session);
            tokio::spawn(async move { session.disable_radio().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !disable.is_finished(),
            "disable must not interleave with an enable that is still running"
        );

        wifi.gate.notify_one();
        let _ = disable.await;
    }

    /// Enable and disable are serialised, so they cannot interleave.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enable_and_disable_are_serialised() {
        let (session, wifi) = session(WifiMode::AlwaysOn);
        let (up, down) = tokio::join!(session.enable_radio(), session.disable_radio());
        assert!(up.is_ok() && down.is_ok());
        assert_eq!(
            wifi.enable_call_count() + wifi.disable_call_count(),
            2,
            "both transitions must run, one after the other"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn off_rejects_acquire() {
        let (session, _) = session(WifiMode::Off);
        let err = session.acquire("x").await.unwrap_err();
        assert!(matches!(err, WifiSessionError::ModeOff));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_when_already_online() {
        let (session, _) = session(WifiMode::Auto);
        session.notify_online();
        let lease = session.acquire("a").await.unwrap();
        assert!(session.has_holders());
        drop(lease);
        assert!(!session.has_holders());
        assert!(session.idle_since().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_holders_idle_only_after_last() {
        let (session, _) = session(WifiMode::Auto);
        session.notify_online();
        let a = session.acquire("a").await.unwrap();
        let b = session.acquire("b").await.unwrap();
        drop(a);
        assert!(session.idle_since().is_none());
        drop(b);
        assert!(session.idle_since().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn always_on_does_not_arm_idle() {
        let (session, _) = session(WifiMode::AlwaysOn);
        session.notify_online();
        let lease = session.acquire("a").await.unwrap();
        drop(lease);
        assert!(session.idle_since().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_online_unblocks_waiter() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(None));
        let session2 = Arc::clone(&session);
        let runtime = tokio::runtime::Handle::current();
        let handle = thread::spawn(move || {
            runtime.block_on(session2.acquire_with_timeout("wait", Duration::from_secs(2)))
        });
        thread::sleep(Duration::from_millis(50));
        session.notify_online();
        let lease = handle.join().unwrap().unwrap();
        drop(lease);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_enables_radio_when_disabled() {
        let (session, wifi) = session(WifiMode::Auto);
        assert!(!wifi.is_enabled().await);
        let session2 = Arc::clone(&session);
        let runtime = tokio::runtime::Handle::current();
        let handle = thread::spawn(move || {
            runtime.block_on(session2.acquire_with_timeout("en", Duration::from_millis(200)))
        });
        thread::sleep(Duration::from_millis(30));
        assert!(wifi.is_enabled().await || handle.is_finished());
        session.notify_online();
        let _ = handle.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_timeout_releases_lease_without_deadlock() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(None));
        let err = session
            .acquire_with_timeout("t", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(err, WifiSessionError::Timeout));
        assert!(!session.has_holders());
        assert!(session.idle_since().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enable_radio_reports_connected_when_associated() {
        let (session, wifi) = session(WifiMode::AlwaysOn);
        wifi.set_network_info(Ok(Some(crate::device::wifi::NetworkInfo {
            ip: "192.168.1.1".parse().unwrap(),
            essid: crate::device::wifi::Essid::new("test"),
        })));
        assert!(session.enable_radio().await.unwrap());
        assert!(session.is_online());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_skips_wait_when_already_associated() {
        let (session, wifi) = session(WifiMode::Auto);
        wifi.set_network_info(Ok(Some(crate::device::wifi::NetworkInfo {
            ip: "192.168.1.1".parse().unwrap(),
            essid: crate::device::wifi::Essid::new("test"),
        })));
        let lease = session
            .acquire_with_timeout("fast", Duration::from_millis(50))
            .await
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn always_on_holds_soft_suspend_only_while_radio_on() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        assert!(soft.is_empty());

        session.set_mode(WifiMode::AlwaysOn);
        assert!(
            soft.is_empty(),
            "AlwaysOn without radio must not pin soft-suspend"
        );

        session.enable_radio().await.unwrap();
        assert!(!soft.is_empty());

        session.disable_radio().await.unwrap();
        assert!(
            soft.is_empty(),
            "disable_radio must drop soft-suspend wifi lease"
        );

        session.set_mode(WifiMode::Off);
        assert!(soft.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn always_on_keeps_soft_suspend_after_last_wifi_holder() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().await.unwrap();
        session.notify_online();
        assert!(!soft.is_empty());

        let lease = session.acquire("a").await.unwrap();
        drop(lease);
        assert!(!soft.is_empty());
        assert!(!session.has_holders());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leaving_always_on_keeps_soft_suspend_while_holders_remain() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().await.unwrap();
        session.notify_online();
        let lease = session.acquire("a").await.unwrap();

        session.set_mode(WifiMode::Auto);
        assert!(!soft.is_empty());

        drop(lease);
        assert!(soft.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disable_radio_drops_always_on_soft_suspend_lease() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::AlwaysOn);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().await.unwrap();
        assert!(!soft.is_empty());

        session.disable_radio().await.unwrap();
        assert!(soft.is_empty());
        assert_eq!(session.mode(), WifiMode::AlwaysOn);

        session.enable_radio().await.unwrap();
        assert!(!soft.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_holder_pins_soft_suspend_while_radio_on() {
        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().await.unwrap();
        session.notify_online();

        let lease = session.acquire("ntp").await.unwrap();
        assert!(
            !soft.is_empty(),
            "Auto-mode WiFi holder must pin soft-suspend while radio is on"
        );
        drop(lease);
        assert!(soft.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_suspend_stays_pinned_while_holder_active_under_churn() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (_dir, soft) = soft_suspend_inhibitor();
        let (session, _) = session(WifiMode::Auto);
        session.set_inhibitor(Arc::clone(&soft));
        session.enable_radio().await.unwrap();
        session.notify_online();

        let failed = Arc::new(AtomicBool::new(false));
        let runtime = tokio::runtime::Handle::current();
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let session = Arc::clone(&session);
                let soft = Arc::clone(&soft);
                let failed = Arc::clone(&failed);
                let runtime = runtime.clone();
                thread::spawn(move || {
                    for n in 0..250 {
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let lease = runtime
                            .block_on(session.acquire(format!("t{i}-{n}")))
                            .unwrap();
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
