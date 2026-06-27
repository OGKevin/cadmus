//! Test device stub for use in unit tests.
//!
//! Provides a `Device` implementation that uses mock hardware components,
//! replacing the `Box<dyn>` parameters in `create_test_context()`.

use crate::battery::FakeBattery;
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

#[derive(Debug, Default)]
struct TestWifiState {
    enabled: Option<bool>,
    enable_calls: u32,
    disable_calls: u32,
}

/// Assertable WiFi manager test double.
///
/// Records enable/disable calls for lifecycle and settings tests. Default
/// behavior is a cooperative no-op that returns `Ok(())`.
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
}

/// Stub input source for tests.
pub struct TestInputSource;

impl InputSource for TestInputSource {
    fn start(
        &mut self,
        _display: crate::framebuffer::Display,
        _button_scheme: crate::settings::ButtonScheme,
    ) -> (Hub, Receiver<Event>) {
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
    battery: FakeBattery,
    frontlight: LightLevels,
    lightsensor: u16,
    wifi_manager: Arc<TestWifiManager>,
    usb_manager: Arc<TestUsbManager>,
    power_manager: Arc<TestPowerManager>,
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
            battery: FakeBattery::new(),
            frontlight: LightLevels::default(),
            lightsensor: 0,
            wifi_manager: Arc::new(TestWifiManager::new()),
            usb_manager: Arc::new(TestUsbManager::new()),
            power_manager: Arc::new(TestPowerManager::new()),
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
}

crate::impl_device_hardware!(
    TestDevice,
    Framebuffer = Pixmap,
    Battery = FakeBattery,
    Frontlight = LightLevels,
    LightSensor = u16,
    WifiManager = TestWifiManager,
    UsbManager = TestUsbManager,
    PowerManager = TestPowerManager,
    Rtc = TestRtc,
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
