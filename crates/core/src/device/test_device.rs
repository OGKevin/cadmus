//! Test device stub for use in unit tests.
//!
//! Provides a `Device` implementation that uses mock hardware components,
//! replacing the `Box<dyn>` parameters in `create_test_context()`.

use crate::device::battery::FakeBattery;
use crate::device::inhibitor::Inhibitor;
use crate::device::rtc::TestRtc;
use crate::device::types::FrontlightKind;
use crate::device::{AppContext, Model};
use crate::device::{
    DeviceCapabilities, DeviceIdentity, DeviceInput, DeviceLifecycle, DevicePaths, DeviceRotation,
    DeviceRuntime, EventOutcome, InputSource,
};
use crate::framebuffer::Pixmap;
use crate::frontlight::LightLevels;
use crate::input::TouchProto;
use crate::view::{Bus, Event, Hub, RenderQueue};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct TestWifiState {
    enabled: Option<bool>,
    enable_calls: u32,
    disable_calls: u32,
    network_info: Result<Option<crate::device::wifi::NetworkInfo>, crate::device::wifi::WifiError>,
}

impl Default for TestWifiState {
    fn default() -> Self {
        Self {
            enabled: None,
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

    pub fn was_disable_called(&self) -> bool {
        self.disable_call_count() > 0
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

impl crate::device::wifi::WifiManager for TestWifiManager {
    fn enable(&self) -> Result<(), crate::device::wifi::WifiError> {
        if let Ok(mut state) = self.state.lock() {
            state.enabled = Some(true);
            state.enable_calls += 1;
        }
        Ok(())
    }

    fn disable(&self) -> Result<(), crate::device::wifi::WifiError> {
        if let Ok(mut state) = self.state.lock() {
            state.enabled = Some(false);
            state.disable_calls += 1;
        }
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.enabled)
            .unwrap_or(false)
    }

    fn network_info(
        &self,
    ) -> Result<Option<crate::device::wifi::NetworkInfo>, crate::device::wifi::WifiError> {
        if !self.is_enabled() {
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
}

impl Default for TestUsbManager {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::device::usb::UsbManager for TestUsbManager {
    fn enable(&self) -> Result<(), crate::device::usb::UsbError> {
        if let Ok(mut state) = self.state.lock() {
            state.enabled = Some(true);
            state.enable_calls += 1;
        }
        Ok(())
    }

    fn disable(&self) -> Result<(), crate::device::usb::UsbError> {
        if let Ok(mut state) = self.state.lock() {
            state.enabled = Some(false);
            state.disable_calls += 1;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct TestPowerState {
    suspend_calls: u32,
    resume_calls: u32,
    arm_deep_idle_calls: u32,
    disarm_deep_idle_calls: u32,
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

    pub fn was_resume_called(&self) -> bool {
        self.resume_call_count() > 0
    }
}

impl Default for TestPowerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::device::power::PowerManager for TestPowerManager {
    fn suspend(&self) -> Result<(), crate::device::power::PowerError> {
        if let Ok(mut state) = self.state.lock() {
            state.suspend_calls += 1;
        }
        Ok(())
    }

    fn resume(&self) -> Result<(), crate::device::power::PowerError> {
        if let Ok(mut state) = self.state.lock() {
            state.resume_calls += 1;
        }
        Ok(())
    }

    fn arm_deep_idle(&self) -> Result<(), crate::device::power::PowerError> {
        if let Ok(mut state) = self.state.lock() {
            state.arm_deep_idle_calls += 1;
        }
        Ok(())
    }

    fn disarm_deep_idle(&self) -> Result<(), crate::device::power::PowerError> {
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
    ) -> (Hub, Receiver<crate::view::HubMessage>) {
        std::sync::mpsc::channel()
    }
}

/// Test device with mock hardware for unit tests.
///
/// Uses `FakeBattery`, `LightLevels` frontlight, and cooperative stub managers.
pub struct TestDevice {
    dims: (u32, u32),
    dpi: u16,
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
    fn handle_event(
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
