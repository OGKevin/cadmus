//! Test device stub for use in unit tests.
//!
//! Provides a `Device` implementation that uses mock hardware components,
//! replacing the `Box<dyn>` parameters in `create_test_context().await`.

use crate::device::battery::FakeBattery;
use crate::device::inhibitor::Inhibitor;
use crate::device::rtc::TestRtc;
use crate::device::types::FrontlightKind;
use crate::device::{AppContext, Model};
use crate::device::{
    AppDevice, DeviceCapabilities, DeviceIdentity, DeviceInput, DeviceLifecycle, DevicePaths,
    DeviceRotation, DeviceRuntime, DeviceTask, EventOutcome, ExitStatus, InputSource,
    ShutdownContext,
};
use crate::framebuffer::Pixmap;
use crate::frontlight::LightLevels;
use crate::input::TouchProto;
use crate::view::{Bus, Event, Hub, RenderQueue};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct TestWifiState {
    on_enable: Option<Box<dyn FnOnce() + Send + 'static>>,
    #[cfg(all(test, target_os = "linux"))]
    on_disable: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
    enabled: Option<bool>,
    enable_calls: u32,
    disable_calls: u32,
    network_info: Result<Option<crate::device::wifi::NetworkInfo>, crate::device::wifi::WifiError>,
}

impl Default for TestWifiState {
    fn default() -> Self {
        Self {
            on_enable: None,
            enabled: None,
            #[cfg(all(test, target_os = "linux"))]
            on_disable: None,
            enable_calls: 0,
            disable_calls: 0,
            network_info: Err(crate::device::wifi::WifiError::Disabled),
        }
    }
}

/// Assertable WiFi manager test double.
///
/// Records enable/disable calls for lifecycle and settings tests. Default
/// behavior is a cooperative no-op that returns `Ok(())`.
/// [`network_info`](crate::device::wifi::WifiManager::network_info) defaults to
/// [`WifiError::Disabled`](crate::device::wifi::WifiError::Disabled).
#[derive(Clone)]
pub struct TestWifiManager {
    state: Arc<Mutex<TestWifiState>>,
}

impl TestWifiManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestWifiState::default())),
        }
    }

    /// Returns the last requested WiFi state, if any call was made.
    pub fn enabled(&self) -> Option<bool> {
        self.state.lock().ok().and_then(|s| s.enabled)
    }

    pub fn enable_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.enable_calls).unwrap_or(0)
    }

    pub fn disable_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.disable_calls).unwrap_or(0)
    }

    pub fn set_on_enable(&self, hook: impl FnOnce() + Send + 'static) {
        if let Ok(mut state) = self.state.lock() {
            state.on_enable = Some(Box::new(hook));
        }
    }

    pub fn was_disable_called(&self) -> bool {
        self.disable_call_count() > 0
    }

    /// Returns a notification of the next disable and the sender that releases
    /// its transition to the disabled state.
    #[cfg(all(test, feature = "kobo", target_os = "linux"))]
    pub(crate) fn set_on_disable(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (released_tx, released_rx) = tokio::sync::oneshot::channel();
        if let Ok(mut state) = self.state.lock() {
            state.on_disable = Some((started_tx, released_rx));
        }
        (started_rx, released_tx)
    }

    /// Sets the value returned by subsequent [`network_info`](crate::device::wifi::WifiManager::network_info) calls.
    ///
    /// [`Ok`] results also mark the manager as enabled so [`is_enabled`](crate::device::wifi::WifiManager::is_enabled)
    /// matches the stubbed connection state. [`WifiError::Disabled`](crate::device::wifi::WifiError::Disabled)
    /// marks it disabled.
    pub fn set_network_info(
        &self,
        info: Result<Option<crate::device::wifi::NetworkInfo>, crate::device::wifi::WifiError>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            match &info {
                Ok(_) => state.enabled = Some(true),
                Err(crate::device::wifi::WifiError::Disabled) => state.enabled = Some(false),
                Err(_) => {}
            }
            state.network_info = info;
        }
    }
}

impl Default for TestWifiManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl crate::device::wifi::WifiManager for TestWifiManager {
    async fn enable(&self) -> Result<(), crate::device::wifi::WifiError> {
        let hook = match self.state.lock() {
            Ok(mut state) => {
                state.enabled = Some(true);
                state.enable_calls += 1;
                state.on_enable.take()
            }
            Err(_) => None,
        };
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    async fn disable(&self) -> Result<(), crate::device::wifi::WifiError> {
        #[cfg(all(test, target_os = "linux"))]
        {
            let gate = match self.state.lock() {
                Ok(mut state) => state.on_disable.take(),
                Err(_) => None,
            };
            if let Some((started, released)) = gate {
                started.send(()).expect("notify disable start");
                released.await.expect("release fake disable");
            }
        }
        if let Ok(mut state) = self.state.lock() {
            state.enabled = Some(false);
            state.disable_calls += 1;
        }
        Ok(())
    }

    async fn is_enabled(&self) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.enabled)
            .unwrap_or(false)
    }

    async fn network_info(
        &self,
    ) -> Result<Option<crate::device::wifi::NetworkInfo>, crate::device::wifi::WifiError> {
        if !self.is_enabled().await {
            return Err(crate::device::wifi::WifiError::Disabled);
        }
        let state = self.state.lock().map_err(|e| {
            crate::device::wifi::WifiError::Lock(format!("Failed to acquire lock: {e}"))
        })?;
        match &state.network_info {
            Ok(info) => Ok(info.clone()),
            Err(e) => Err(crate::device::wifi::clone_wifi_error(e)),
        }
    }
}

#[derive(Debug, Default)]
struct TestUsbState {
    enabled: Option<bool>,
    enable_calls: u32,
    disable_calls: u32,
    fail_disable: bool,
    fail_enable: bool,
}

/// Assertable USB manager test double.
#[derive(Clone)]
pub struct TestUsbManager {
    state: Arc<Mutex<TestUsbState>>,
}

impl TestUsbManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestUsbState::default())),
        }
    }

    pub fn enabled(&self) -> Option<bool> {
        self.state.lock().ok().and_then(|s| s.enabled)
    }

    pub fn enable_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.enable_calls).unwrap_or(0)
    }

    pub fn disable_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.disable_calls).unwrap_or(0)
    }

    /// Makes subsequent [`disable`](crate::device::usb::UsbManager::disable)
    /// calls fail without changing the recorded enabled state.
    pub fn set_fail_disable(&self, fail: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.fail_disable = fail;
        }
    }

    /// Makes subsequent [`enable`](crate::device::usb::UsbManager::enable)
    /// calls fail without changing the recorded enabled state.
    pub fn set_fail_enable(&self, fail: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.fail_enable = fail;
        }
    }
}

impl Default for TestUsbManager {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::device::usb::UsbManager for TestUsbManager {
    async fn enable(&self) -> Result<(), crate::device::usb::UsbError> {
        let fail = match self.state.lock() {
            Ok(mut state) => {
                state.enable_calls += 1;
                if !state.fail_enable {
                    state.enabled = Some(true);
                }
                state.fail_enable
            }
            Err(_) => false,
        };
        if fail {
            return Err(crate::device::usb::UsbError::GadgetConfig(
                "injected enable failure".to_string(),
            ));
        }
        Ok(())
    }

    async fn disable(&self) -> Result<(), crate::device::usb::UsbError> {
        let fail = match self.state.lock() {
            Ok(mut state) => {
                state.disable_calls += 1;
                if !state.fail_disable {
                    state.enabled = Some(false);
                }
                state.fail_disable
            }
            Err(_) => false,
        };
        if fail {
            return Err(crate::device::usb::UsbError::GadgetConfig(
                "injected disable failure".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
struct TestPowerState {
    suspend_calls: u32,
    resume_calls: u32,
    arm_deep_idle_calls: u32,
    disarm_deep_idle_calls: u32,
    suspend_fails: bool,
    wake_lock_probe: Option<PathBuf>,
    wake_lock_at_suspend: Option<String>,
    on_prepare_suspend: Option<Box<dyn FnOnce() + Send + 'static>>,
    on_suspend: Option<Box<dyn FnOnce() + Send + 'static>>,
    resume_gate: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}

/// Assertable power manager test double.
#[derive(Clone)]
pub struct TestPowerManager {
    state: Arc<Mutex<TestPowerState>>,
}

impl TestPowerManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestPowerState::default())),
        }
    }

    pub fn suspend_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.suspend_calls).unwrap_or(0)
    }

    pub fn resume_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.resume_calls).unwrap_or(0)
    }

    pub fn arm_deep_idle_call_count(&self) -> u32 {
        self.state
            .lock()
            .map(|s| s.arm_deep_idle_calls)
            .unwrap_or(0)
    }

    pub fn disarm_deep_idle_call_count(&self) -> u32 {
        self.state
            .lock()
            .map(|s| s.disarm_deep_idle_calls)
            .unwrap_or(0)
    }

    pub fn was_suspend_called(&self) -> bool {
        self.suspend_call_count() > 0
    }

    /// Makes subsequent [`suspend`](crate::device::power::PowerManager::suspend)
    /// calls fail with `EBUSY`, like a kernel autosleep refusal.
    pub fn set_suspend_fails(&self, fails: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.suspend_fails = fails;
        }
    }

    pub fn was_resume_called(&self) -> bool {
        self.resume_call_count() > 0
    }

    /// Installs a one-shot callback run inside `prepare_suspend`, letting tests
    /// arbitrate a Full acquisition during Classic preparation deterministically.
    pub fn set_on_prepare_suspend(&self, hook: impl FnOnce() + Send + 'static) {
        if let Ok(mut state) = self.state.lock() {
            state.on_prepare_suspend = Some(Box::new(hook));
        }
    }

    /// Installs a one-shot callback run at the start of the kernel sleep write,
    /// letting tests hold the commit in flight before releasing it.
    pub fn set_on_suspend(&self, hook: impl FnOnce() + Send + 'static) {
        if let Ok(mut state) = self.state.lock() {
            state.on_suspend = Some(Box::new(hook));
        }
    }

    /// Installs a one-shot asynchronous barrier inside `resume`, ahead of the
    /// disarm it delegates to. Returns a receiver that fires when `resume` has
    /// been entered and a sender that releases it.
    pub fn set_resume_barrier(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        if let Ok(mut state) = self.state.lock() {
            state.resume_gate = Some((started_tx, release_rx));
        }
        (started_rx, release_tx)
    }

    /// Captures a sysfs wake-lock node at the moment `suspend` is called.
    pub fn set_wake_lock_probe(&self, path: PathBuf) {
        if let Ok(mut state) = self.state.lock() {
            state.wake_lock_probe = Some(path);
        }
    }

    /// Wake-lock node contents observed by the last `suspend` probe.
    pub fn wake_lock_at_suspend(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.wake_lock_at_suspend.clone())
    }
}

impl Default for TestPowerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::device::power::PowerManager for TestPowerManager {
    async fn prepare_suspend(&self) -> Result<(), crate::device::power::PowerError> {
        self.arm_deep_idle().await?;
        let hook = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.on_prepare_suspend.take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    async fn suspend(&self) -> Result<(), crate::device::power::PowerError> {
        let (fails, probe, hook) = match self.state.lock() {
            Ok(mut state) => {
                state.suspend_calls += 1;
                (
                    state.suspend_fails,
                    state.wake_lock_probe.clone(),
                    state.on_suspend.take(),
                )
            }
            Err(_) => (false, None, None),
        };
        if let Some(hook) = hook {
            hook();
        }
        if let Some(path) = probe {
            let value = tokio::fs::read_to_string(path)
                .await
                .ok()
                .map(|value| value.trim().to_owned());
            if let Ok(mut state) = self.state.lock() {
                state.wake_lock_at_suspend = value;
            }
        }
        if fails {
            return Err(crate::device::power::PowerError::Io(
                std::io::Error::from_raw_os_error(16),
            ));
        }
        Ok(())
    }

    async fn resume(&self) -> Result<(), crate::device::power::PowerError> {
        let gate = match self.state.lock() {
            Ok(mut state) => {
                state.resume_calls += 1;
                state.resume_gate.take()
            }
            Err(_) => return Err(crate::device::power::PowerError::LockPoisoned),
        };
        if let Some((started, release)) = gate {
            started.send(()).map_err(|()| {
                crate::device::power::PowerError::Io(std::io::Error::other(
                    "resume barrier start receiver dropped",
                ))
            })?;
            release.await.map_err(|error| {
                crate::device::power::PowerError::Io(std::io::Error::other(error))
            })?;
        }
        self.disarm_deep_idle().await
    }

    async fn arm_deep_idle(&self) -> Result<(), crate::device::power::PowerError> {
        if let Ok(mut state) = self.state.lock() {
            state.arm_deep_idle_calls += 1;
        }
        Ok(())
    }

    async fn disarm_deep_idle(&self) -> Result<(), crate::device::power::PowerError> {
        if let Ok(mut state) = self.state.lock() {
            state.disarm_deep_idle_calls += 1;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct TestLedsState {
    on_calls: u32,
    off_calls: u32,
    is_on: bool,
}

/// Assertable LED controller test double.
#[derive(Clone)]
pub struct TestLeds {
    state: Arc<Mutex<TestLedsState>>,
}

impl TestLeds {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestLedsState::default())),
        }
    }

    pub fn on_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.on_calls).unwrap_or(0)
    }

    pub fn off_call_count(&self) -> u32 {
        self.state.lock().map(|s| s.off_calls).unwrap_or(0)
    }

    pub fn is_on(&self) -> bool {
        self.state.lock().map(|s| s.is_on).unwrap_or(false)
    }
}

impl Default for TestLeds {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::device::leds::DeviceLeds for TestLeds {
    fn on(&self) -> Result<(), crate::device::leds::LedsError> {
        if let Ok(mut state) = self.state.lock() {
            state.on_calls += 1;
            state.is_on = true;
        }
        Ok(())
    }

    fn off(&self) -> Result<(), crate::device::leds::LedsError> {
        if let Ok(mut state) = self.state.lock() {
            state.off_calls += 1;
            state.is_on = false;
        }
        Ok(())
    }
}

/// Stub input source for tests.
pub struct TestInputSource;

impl InputSource for TestInputSource {
    fn start(
        &mut self,
        _display: crate::framebuffer::Display,
        _button_scheme: crate::settings::ButtonScheme,
        _inhibitor: Arc<Inhibitor>,
    ) -> (Hub, crate::view::HubReceiver) {
        crate::view::hub_channel()
    }
}

/// Test device with mock hardware for unit tests.
///
/// Uses `FakeBattery`, `LightLevels` frontlight, and cooperative stub managers.
pub struct TestDevice {
    dims: (u32, u32),
    dpi: u16,
    gyroscope: bool,
    page_turn_buttons: bool,
    framebuffer: Pixmap,
    battery: Arc<FakeBattery>,
    frontlight: LightLevels,
    lightsensor: u16,
    wifi_manager: Arc<TestWifiManager>,
    usb_manager: Arc<TestUsbManager>,
    power_manager: Arc<TestPowerManager>,
    leds: Arc<TestLeds>,
    rtc: Arc<TestRtc>,
    time_manager: crate::time_manager::TimeManager<TestRtc>,
    input: TestInputSource,
}

impl TestDevice {
    pub fn new() -> Self {
        let rtc = Arc::new(TestRtc::new());
        let time_manager = crate::time_manager::TimeManager::new(rtc.clone(), |_| Ok(()));
        Self {
            dims: (600, 800),
            dpi: 300,
            gyroscope: false,
            page_turn_buttons: false,
            framebuffer: Pixmap::new(600, 800, 1),
            battery: Arc::new(FakeBattery::new()),
            frontlight: LightLevels::default(),
            lightsensor: 0,
            wifi_manager: Arc::new(TestWifiManager::new()),
            usb_manager: Arc::new(TestUsbManager::new()),
            power_manager: Arc::new(TestPowerManager::new()),
            leds: Arc::new(TestLeds::new()),
            rtc,
            time_manager,
            input: TestInputSource,
        }
    }

    /// Returns the WiFi test double for lifecycle assertion helpers.
    pub fn wifi_manager_for_test(&self) -> &TestWifiManager {
        self.wifi_manager.as_ref()
    }

    /// Returns the USB test double for lifecycle assertion helpers.
    pub fn usb_manager_for_test(&self) -> &TestUsbManager {
        self.usb_manager.as_ref()
    }

    /// Returns the power test double for lifecycle assertion helpers.
    pub fn power_manager_for_test(&self) -> &TestPowerManager {
        self.power_manager.as_ref()
    }

    /// Returns the LED test double for assertion helpers.
    pub fn leds_for_test(&self) -> &TestLeds {
        self.leds.as_ref()
    }

    /// Simulates the presence of a gyroscope.
    pub fn set_has_gyroscope(&mut self, present: bool) {
        self.gyroscope = present;
    }

    /// Simulates the presence of dedicated page-turn buttons.
    pub fn set_has_page_turn_buttons(&mut self, present: bool) {
        self.page_turn_buttons = present;
    }

    /// Builds the test device.
    pub async fn from_environment() -> anyhow::Result<Self> {
        Ok(Self::default())
    }
}

impl Default for TestDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceIdentity for TestDevice {
    fn model(&self) -> Model {
        Model::TestDevice
    }

    fn proto(&self) -> TouchProto {
        TouchProto::Single
    }

    fn dims(&self) -> (u32, u32) {
        self.dims
    }

    fn dpi(&self) -> u16 {
        self.dpi
    }

    fn mark(&self) -> u8 {
        3
    }
}

impl DeviceCapabilities for TestDevice {
    fn frontlight_kind(&self) -> FrontlightKind {
        FrontlightKind::Standard
    }

    fn has_gyroscope(&self) -> bool {
        self.gyroscope
    }

    fn has_page_turn_buttons(&self) -> bool {
        self.page_turn_buttons
    }
}

impl DeviceRotation for TestDevice {
    fn startup_rotation(&self) -> i8 {
        3
    }

    fn mirroring_scheme(&self) -> (i8, i8) {
        (2, 1)
    }
}

impl DevicePaths for TestDevice {
    fn install_subdir(&self) -> &'static str {
        ".adds/cadmus-tst"
    }

    fn install_dir(&self) -> PathBuf {
        std::env::temp_dir()
            .join("test-kobo-installation")
            .join(self.install_subdir())
    }

    fn data_subdir(&self) -> &'static str {
        ".cadmus-tst"
    }

    fn data_dir(&self) -> PathBuf {
        self.install_dir()
    }

    fn peer_installs(&self) -> Vec<crate::device::PeerInstall> {
        let root = std::env::temp_dir().join("test-kobo-installation");
        let current = self.install_dir();
        [
            (".adds/cadmus", crate::version::BuildKind::Standard),
            (".adds/cadmus-tst", crate::version::BuildKind::Test),
        ]
        .into_iter()
        .filter_map(|(subdir, kind)| {
            let dir = root.join(subdir);
            if dir == current {
                return None;
            }
            let launcher = dir.join("cadmus.sh");
            launcher
                .is_file()
                .then_some(crate::device::PeerInstall { kind, launcher })
        })
        .collect()
    }
}

crate::impl_device_hardware!(
    TestDevice,
    Framebuffer = Pixmap,
    Battery = Arc<FakeBattery>,
    Frontlight = LightLevels,
    LightSensor = u16,
    WifiManager = TestWifiManager,
    UsbManager = TestUsbManager,
    PowerManager = TestPowerManager,
    Leds = TestLeds,
    Rtc = TestRtc;
    override inhibitor noop_battery,
);

impl DeviceInput for TestDevice {
    type Input = TestInputSource;

    fn input(&self) -> &Self::Input {
        &self.input
    }

    fn input_mut(&mut self) -> &mut Self::Input {
        &mut self.input
    }
}

impl DeviceLifecycle for TestDevice {
    async fn on_shutdown(
        _context: &mut ShutdownContext<'_, AppDevice>,
        _status: ExitStatus,
        _tasks: &[DeviceTask],
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn handle_event(
        _event: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut AppContext,
        _runtime: &mut DeviceRuntime<'_>,
    ) -> EventOutcome {
        EventOutcome::Unhandled
    }
}
