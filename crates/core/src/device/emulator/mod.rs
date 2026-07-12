//! Emulator device implementation (cfg-gated behind `emulator` feature).
//!
//! Screen rotation is intentionally unsupported: [`FBCanvas::rotation`] always
//! reports [`DEFAULT_ROTATION`] and rotation APIs are no-ops. This avoids
//! inconsistent state until faithful framebuffer and input tracking exists.

mod power;
mod rtc;
mod usb;
mod wifi;

use crate::battery::FakeBattery;
use crate::color::Color;
use crate::device::DeviceHardware as _;
use crate::device::emulator::rtc::NoopRtc;
use crate::device::types::FrontlightKind;
use crate::device::{AppContext, Model};
use crate::device::{
    DeviceCapabilities, DeviceIdentity, DeviceInput, DeviceLifecycle, DevicePaths, DeviceRotation,
    DeviceRuntime, EventOutcome, ExitStatus, InputSource,
};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::frontlight::LightLevels;
use crate::geom::{Axis, Rectangle};
use crate::gesture::GestureEvent;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use crate::settings::IntermKind;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{Bus, EntryId, Event, Hub, RenderData, RenderQueue, View};
use anyhow::{Context as ResultExt, Error};
use sdl2::Sdl;
use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::pixels::{Color as SdlColor, PixelFormatEnum};
use sdl2::rect::Point as SdlPoint;
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, WindowCanvas};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_ROTATION: i8 = 1;
const EMULATOR_WIDTH: u32 = 600;
const EMULATOR_HEIGHT: u32 = 800;

struct SendableSdl(Sdl);

// SAFETY: SDL types contain raw pointers for thread-affinity enforcement, but
// we ensure each value is only ever accessed from the single thread it is moved
// into. The event pump is created and used exclusively on the spawned thread.
unsafe impl Send for SendableSdl {}

struct SendableEventPump(sdl2::EventPump);
unsafe impl Send for SendableEventPump {}

impl std::ops::Deref for SendableEventPump {
    type Target = sdl2::EventPump;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SendableEventPump {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct EmulatorInputSource {
    dpi: u16,
    sender: Option<Sender<DeviceEvent>>,
    sdl_context: Option<SendableSdl>,
}

impl EmulatorInputSource {
    fn new(dpi: u16) -> Self {
        Self {
            dpi,
            sender: None,
            sdl_context: None,
        }
    }

    fn new_with_sdl(dpi: u16, sdl_context: Sdl) -> Self {
        Self {
            dpi,
            sender: None,
            sdl_context: Some(SendableSdl(sdl_context)),
        }
    }
}

impl InputSource for EmulatorInputSource {
    /// No-op — emulator rotation is unsupported until touch mapping tracks rotation.
    fn set_rotation(&self, _n: i8) {}

    fn start(
        &mut self,
        _display: crate::framebuffer::Display,
        _button_scheme: crate::settings::ButtonScheme,
    ) -> (Hub, Receiver<Event>) {
        let (hub, rx) = mpsc::channel();
        let (device_tx, device_rx) = mpsc::channel();
        self.sender = Some(device_tx.clone());

        let gesture_rx = crate::gesture::gesture_events(device_rx, self.dpi);
        let hub_clone = hub.clone();

        std::thread::spawn(move || {
            while let Ok(event) = gesture_rx.recv() {
                hub_clone.send(event).ok();
            }
        });

        if let Some(sendable_sdl) = self.sdl_context.take() {
            let hub = hub.clone();
            let sender = device_tx;
            let mut event_pump = SendableEventPump(sendable_sdl.0.event_pump().unwrap());
            std::thread::spawn(move || {
                'outer: loop {
                    while let Some(sdl_evt) = event_pump.poll_event() {
                        #[cfg(feature = "tracing")]
                        let span = tracing::trace_span!("sdl-event-loop", event = ?sdl_evt);
                        #[cfg(feature = "tracing")]
                        let _enter = span.enter();
                        #[cfg(feature = "tracing")]
                        tracing::trace!(event = ?sdl_evt, "handling event");

                        match sdl_evt {
                            SdlEvent::Quit { .. }
                            | SdlEvent::KeyDown {
                                keycode: Some(Keycode::Escape),
                                keymod: Mod::NOMOD,
                                ..
                            } => {
                                hub.send(Event::Select(EntryId::Quit)).ok();
                                break 'outer;
                            }
                            SdlEvent::KeyUp {
                                scancode: Some(scancode),
                                keymod: Mod::NOMOD,
                                timestamp,
                                ..
                            } => {
                                if let Some(code) = code_from_key(scancode) {
                                    sender
                                        .send(DeviceEvent::Button {
                                            time: seconds(timestamp),
                                            code,
                                            status: ButtonStatus::Released,
                                        })
                                        .ok();
                                }
                            }
                            SdlEvent::KeyDown {
                                scancode: Some(scancode),
                                keymod,
                                timestamp,
                                repeat,
                                ..
                            } => match keymod {
                                Mod::NOMOD => match scancode {
                                    Scancode::S => {
                                        hub.send(Event::Select(EntryId::TakeScreenshot)).ok();
                                    }
                                    Scancode::B
                                    | Scancode::F
                                    | Scancode::P
                                    | Scancode::L
                                    | Scancode::H
                                    | Scancode::E
                                    | Scancode::G => {
                                        if let Some(code) = code_from_key(scancode) {
                                            let status = if repeat {
                                                ButtonStatus::Repeated
                                            } else {
                                                ButtonStatus::Pressed
                                            };
                                            sender
                                                .send(DeviceEvent::Button {
                                                    time: seconds(timestamp),
                                                    code,
                                                    status,
                                                })
                                                .ok();
                                        }
                                    }
                                    Scancode::I | Scancode::O => {
                                        let mouse_state = event_pump.mouse_state();
                                        let x = mouse_state.x();
                                        let y = mouse_state.y();
                                        let center = pt!(x, y);
                                        if scancode == Scancode::I {
                                            hub.send(Event::Gesture(GestureEvent::Spread {
                                                center,
                                                factor: 2.0,
                                                axis: Axis::Diagonal,
                                            }))
                                            .ok();
                                        } else {
                                            hub.send(Event::Gesture(GestureEvent::Pinch {
                                                center,
                                                factor: 0.5,
                                                axis: Axis::Diagonal,
                                            }))
                                            .ok();
                                        }
                                    }
                                    _ => (),
                                },
                                Mod::LSHIFTMOD | Mod::RSHIFTMOD => match scancode {
                                    Scancode::S => {
                                        hub.send(Event::Select(EntryId::ShowIntermission(
                                            IntermKind::Suspend,
                                        )))
                                        .ok();
                                    }
                                    Scancode::P => {
                                        hub.send(Event::Select(EntryId::ShowIntermission(
                                            IntermKind::PowerOff,
                                        )))
                                        .ok();
                                    }
                                    Scancode::C => {
                                        hub.send(Event::Select(EntryId::ShowIntermission(
                                            IntermKind::Share,
                                        )))
                                        .ok();
                                    }
                                    _ => (),
                                },
                                _ => (),
                            },
                            _ => {
                                if let Some(dev_evt) = device_event(sdl_evt) {
                                    sender.send(dev_evt).ok();
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
        }

        let hub_clone = hub.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(CLOCK_REFRESH_INTERVAL);
                hub_clone.send(Event::ClockTick).ok();
            }
        });

        (hub, rx)
    }

    /// Forwards the event to the SDL device-event channel started by [`Self::start`].
    fn send_device(&self, event: DeviceEvent) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send(event).ok();
        }
    }
}

/// SDL window-backed framebuffer for the emulator.
///
/// Rotation is unsupported: [`Framebuffer::rotation`] always returns
/// [`DEFAULT_ROTATION`] and [`Framebuffer::set_rotation`] is a no-op.
pub struct FBCanvas(pub WindowCanvas);

unsafe impl Send for FBCanvas {}

impl Framebuffer for FBCanvas {
    fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        let [red, green, blue] = color.rgb();
        self.0.set_draw_color(SdlColor::RGB(red, green, blue));
        self.0
            .draw_point(SdlPoint::new(x as i32, y as i32))
            .unwrap();
    }

    fn set_blended_pixel(&mut self, x: u32, y: u32, color: Color, alpha: f32) {
        let [red, green, blue] = color.rgb();
        self.0
            .set_draw_color(SdlColor::RGBA(red, green, blue, (alpha * 255.0) as u8));
        self.0
            .draw_point(SdlPoint::new(x as i32, y as i32))
            .unwrap();
    }

    fn invert_region(&mut self, rect: &Rectangle) {
        let width = rect.width();
        let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
        if let Ok(data) = self.0.read_pixels(s_rect, PixelFormatEnum::RGB24) {
            for y in rect.min.y..rect.max.y {
                let v = (y - rect.min.y) as u32;
                for x in rect.min.x..rect.max.x {
                    let u = (x - rect.min.x) as u32;
                    let addr = 3 * (v * width + u);
                    let red = data[addr as usize];
                    let green = data[(addr + 1) as usize];
                    let blue = data[(addr + 2) as usize];
                    let mut color = Color::Rgb(red, green, blue);
                    color.invert();
                    self.set_pixel(x as u32, y as u32, color);
                }
            }
        }
    }

    fn shift_region(&mut self, rect: &Rectangle, drift: u8) {
        let width = rect.width();
        let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
        if let Ok(data) = self.0.read_pixels(s_rect, PixelFormatEnum::RGB24) {
            for y in rect.min.y..rect.max.y {
                let v = (y - rect.min.y) as u32;
                for x in rect.min.x..rect.max.x {
                    let u = (x - rect.min.x) as u32;
                    let addr = 3 * (v * width + u);
                    let red = data[addr as usize];
                    let green = data[(addr + 1) as usize];
                    let blue = data[(addr + 2) as usize];
                    let mut color = Color::Rgb(red, green, blue);
                    color.shift(drift);
                    self.set_pixel(x as u32, y as u32, color);
                }
            }
        }
    }

    fn update(&mut self, _rect: &Rectangle, _mode: UpdateMode) -> Result<u32, Error> {
        self.0.present();
        Ok(crate::chrono::Local::now().timestamp_subsec_millis())
    }

    fn wait(&self, _tok: u32) -> Result<i32, Error> {
        Ok(1)
    }

    fn save(&self, path: &str) -> Result<(), Error> {
        let (width, height) = self.dims();
        let file =
            File::create(path).with_context(|| format!("can't create output file {}", path))?;
        let mut encoder = crate::png::Encoder::new(file, width, height);
        encoder.set_depth(crate::png::BitDepth::Eight);
        encoder.set_color(crate::png::ColorType::Rgb);
        let mut writer = encoder
            .write_header()
            .with_context(|| format!("can't write PNG header for {}", path))?;
        let data = self
            .0
            .read_pixels(self.0.viewport(), PixelFormatEnum::RGB24)
            .unwrap_or_default();
        writer
            .write_image_data(&data)
            .with_context(|| format!("can't write PNG data to {}", path))?;
        Ok(())
    }

    fn rotation(&self) -> i8 {
        DEFAULT_ROTATION
    }

    fn set_rotation(&mut self, _n: i8) -> Result<(u32, u32), Error> {
        Ok(self.dims())
    }

    fn set_monochrome(&mut self, _enable: bool) {}
    fn set_dithered(&mut self, _enable: bool) {}
    fn set_inverted(&mut self, _enable: bool) {}
    fn monochrome(&self) -> bool {
        false
    }
    fn dithered(&self) -> bool {
        false
    }
    fn inverted(&self) -> bool {
        false
    }
    fn width(&self) -> u32 {
        self.0.window().size().0
    }
    fn height(&self) -> u32 {
        self.0.window().size().1
    }
}

pub struct EmulatorDevice {
    dims: (u32, u32),
    dpi: u16,
    framebuffer: Box<dyn Framebuffer + Send>,
    battery: FakeBattery,
    frontlight: LightLevels,
    lightsensor: u16,
    wifi_manager: Arc<crate::device::emulator::wifi::EmulatorWifiManager>,
    usb_manager: Arc<crate::device::emulator::usb::EmulatorUsbManager>,
    power_manager: Arc<crate::device::emulator::power::EmulatorPowerManager>,
    rtc: Arc<NoopRtc>,
    time_manager: crate::time_manager::TimeManager<NoopRtc>,
    input: EmulatorInputSource,
}

impl Default for EmulatorDevice {
    fn default() -> Self {
        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();

        let window = video_subsystem
            .window("Cadmus Emulator", EMULATOR_WIDTH, EMULATOR_HEIGHT)
            .position_centered()
            .build()
            .unwrap();
        let canvas = window.into_canvas().software().build().unwrap();

        Self::from_sdl_canvas(canvas, sdl_context)
    }
}

impl EmulatorDevice {
    pub fn new(framebuffer: Box<dyn Framebuffer + Send>) -> Self {
        let dims = framebuffer.dims();
        let dpi = 167;
        let rtc = Arc::new(NoopRtc);
        let time_manager = crate::time_manager::TimeManager::new(rtc.clone(), |_| Ok(()));
        Self {
            dims,
            dpi,
            framebuffer,
            battery: FakeBattery::new(),
            frontlight: LightLevels::default(),
            lightsensor: 0,
            wifi_manager: Arc::new(crate::device::emulator::wifi::EmulatorWifiManager),
            usb_manager: Arc::new(crate::device::emulator::usb::EmulatorUsbManager),
            power_manager: Arc::new(crate::device::emulator::power::EmulatorPowerManager),
            rtc,
            time_manager,
            input: EmulatorInputSource::new(dpi),
        }
    }

    pub fn from_sdl_canvas(canvas: WindowCanvas, sdl_context: Sdl) -> Self {
        let mut canvas = canvas;
        canvas.set_blend_mode(BlendMode::Blend);
        let mut device = Self::new(Box::new(FBCanvas(canvas)));
        device.input = EmulatorInputSource::new_with_sdl(device.dpi, sdl_context);
        device
    }
}

impl DeviceIdentity for EmulatorDevice {
    fn model(&self) -> Model {
        Model::Emulator
    }
    fn proto(&self) -> crate::input::TouchProto {
        crate::input::TouchProto::MultiB
    }
    fn dims(&self) -> (u32, u32) {
        self.dims
    }
    fn dpi(&self) -> u16 {
        self.dpi
    }
    fn mark(&self) -> u8 {
        7
    }
}

impl DeviceCapabilities for EmulatorDevice {
    fn frontlight_kind(&self) -> FrontlightKind {
        FrontlightKind::Premixed
    }
}

impl DeviceRotation for EmulatorDevice {
    fn startup_rotation(&self) -> i8 {
        3
    }
    fn mirroring_scheme(&self) -> (i8, i8) {
        (2, 1)
    }
}

impl DevicePaths for EmulatorDevice {
    fn install_subdir(&self) -> &'static str {
        ""
    }
    fn install_dir(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
    fn data_subdir(&self) -> &'static str {
        ".cadmus"
    }
    fn data_dir(&self) -> PathBuf {
        self.install_dir().join(self.data_subdir())
    }
}

crate::impl_device_hardware!(
    EmulatorDevice,
    Framebuffer = Box<dyn Framebuffer + Send>,
    Battery = FakeBattery,
    Frontlight = LightLevels,
    LightSensor = u16,
    WifiManager = crate::device::emulator::wifi::EmulatorWifiManager,
    UsbManager = crate::device::emulator::usb::EmulatorUsbManager,
    PowerManager = crate::device::emulator::power::EmulatorPowerManager,
    Rtc = crate::device::emulator::rtc::NoopRtc,
);

impl DeviceInput for EmulatorDevice {
    type Input = EmulatorInputSource;

    fn input(&self) -> &Self::Input {
        &self.input
    }
    fn input_mut(&mut self) -> &mut Self::Input {
        &mut self.input
    }
}

fn handle_set_wifi(enable: bool, context: &mut AppContext, hub: &Hub) -> EventOutcome {
    if context.settings.wifi == enable {
        return EventOutcome::Handled;
    }

    context.settings.wifi = enable;
    if enable {
        let hub = hub.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            hub.send(Event::Device(DeviceEvent::NetUp)).ok();
        });
    } else {
        context.online = false;
    }

    EventOutcome::Handled
}

fn handle_net_up(context: &mut AppContext) -> EventOutcome {
    context.online = true;
    EventOutcome::Continue
}

fn handle_toggle_frontlight(context: &mut AppContext) {
    context.set_frontlight(!context.settings.frontlight);
}

fn show_suspend_intermission(
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    runtime
        .view
        .handle_event(&Event::Suspend, hub, bus, rq, context);
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        IntermKind::Suspend,
        context,
    );
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    runtime.view.children_mut().push(Box::new(interm));
}

impl DeviceLifecycle for EmulatorDevice {
    fn handle_event(
        event: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        runtime: &mut DeviceRuntime<'_>,
    ) -> EventOutcome {
        match event {
            Event::SetWifi(enable) => handle_set_wifi(*enable, context, hub),
            Event::Select(EntryId::ToggleWifi) => {
                handle_set_wifi(!context.settings.wifi, context, hub)
            }
            Event::Device(DeviceEvent::NetUp) => handle_net_up(context),
            Event::Device(DeviceEvent::RotateScreen(_)) => EventOutcome::Handled,
            Event::Select(EntryId::ShowIntermission(kind)) => {
                if let Some(index) = locate::<Intermission>(runtime.view.as_ref()) {
                    let rect = *runtime.view.child(index).rect();
                    runtime.view.children_mut().remove(index);
                    rq.add(RenderData::expose(rect, UpdateMode::Full));
                } else {
                    runtime
                        .view
                        .handle_event(&Event::Suspend, hub, bus, rq, context);
                    let interm =
                        Intermission::new(context.device.framebuffer().rect(), *kind, context);
                    rq.add(RenderData::new(
                        interm.id(),
                        *interm.rect(),
                        UpdateMode::Full,
                    ));
                    runtime.view.children_mut().push(Box::new(interm));
                }
                EventOutcome::Handled
            }
            Event::ToggleFrontlight => {
                handle_toggle_frontlight(context);
                EventOutcome::Continue
            }
            Event::Select(EntryId::Restart) => EventOutcome::Exit(ExitStatus::Restart),
            Event::Select(EntryId::PowerOff) => EventOutcome::Exit(ExitStatus::PowerOff),
            Event::Select(EntryId::Suspend) => {
                show_suspend_intermission(hub, bus, rq, context, runtime);
                EventOutcome::Handled
            }
            Event::Select(EntryId::Reboot) => {
                std::thread::sleep(std::time::Duration::from_secs(3));
                EventOutcome::Exit(crate::device::ExitStatus::Quit)
            }
            Event::Select(EntryId::SyncTime) => {
                tracing::info!("Sync Time requested (no-op in emulator)");
                EventOutcome::Handled
            }
            Event::Select(EntryId::Quit) => EventOutcome::Exit(crate::device::ExitStatus::Quit),
            _ => EventOutcome::Unhandled,
        }
    }
}

#[inline]
fn seconds(timestamp: u32) -> f64 {
    timestamp as f64 / 1000.0
}

pub fn device_event(event: SdlEvent) -> Option<DeviceEvent> {
    match event {
        SdlEvent::MouseButtonDown {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Down,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        SdlEvent::MouseButtonUp {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Up,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        SdlEvent::MouseMotion {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Motion,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        _ => None,
    }
}

pub fn code_from_key(key: Scancode) -> Option<ButtonCode> {
    match key {
        Scancode::B => Some(ButtonCode::Backward),
        Scancode::F => Some(ButtonCode::Forward),
        Scancode::P => Some(ButtonCode::Power),
        Scancode::L => Some(ButtonCode::Light),
        Scancode::H => Some(ButtonCode::Home),
        Scancode::E => Some(ButtonCode::Erase),
        Scancode::G => Some(ButtonCode::Highlight),
        _ => None,
    }
}

#[cfg(all(test, feature = "emulator"))]
mod wifi_tests {
    use super::{handle_net_up, handle_set_wifi};
    use crate::context::test_helpers::create_test_context;
    use crate::device::EventOutcome;
    use std::sync::mpsc;

    #[test]
    fn handle_set_wifi_enable_does_not_set_online_immediately() {
        let mut context = create_test_context();
        let (hub, _rx) = mpsc::channel();
        assert!(!context.settings.wifi);
        assert!(!context.online);

        let outcome = handle_set_wifi(true, &mut context, &hub);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(context.settings.wifi);
        assert!(!context.online);
    }

    #[test]
    fn handle_set_wifi_disable_clears_online() {
        let mut context = create_test_context();
        let (hub, _rx) = mpsc::channel();
        context.settings.wifi = true;
        context.online = true;

        let outcome = handle_set_wifi(false, &mut context, &hub);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!context.settings.wifi);
        assert!(!context.online);
    }

    #[test]
    fn handle_net_up_sets_online() {
        let mut context = create_test_context();
        assert!(!context.online);

        let outcome = handle_net_up(&mut context);
        assert_eq!(outcome, EventOutcome::Continue);
        assert!(context.online);
    }

    #[test]
    fn toggle_wifi_enables_wifi_without_setting_online() {
        let mut context = create_test_context();
        let (hub, _rx) = mpsc::channel();
        assert!(!context.settings.wifi);

        let outcome = handle_set_wifi(!context.settings.wifi, &mut context, &hub);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(context.settings.wifi);
        assert!(!context.online);
    }
}

#[cfg(all(test, feature = "emulator"))]
mod lifecycle {
    use super::{EmulatorDevice, handle_toggle_frontlight};
    use crate::color::WHITE;
    use crate::context::test_helpers::create_test_context;
    use crate::device::DeviceHardware as _;
    use crate::device::DeviceLifecycle as _;
    use crate::device::{DeviceRuntime, EventOutcome, ExitStatus, HistoryItem};
    use crate::framebuffer::Framebuffer as _;
    use crate::view::filler::Filler;
    use crate::view::{Bus, EntryId, Event, RenderQueue, View};
    use std::sync::mpsc;
    use std::time::Instant;

    fn with_runtime<R>(
        f: impl FnOnce(
            &mpsc::Sender<Event>,
            &mut Bus,
            &mut RenderQueue,
            &mut crate::device::AppContext,
            &mut DeviceRuntime<'_>,
        ) -> R,
    ) -> R {
        let (hub, _rx) = mpsc::channel();
        let mut context = create_test_context();
        let rect = context.device.framebuffer().rect();
        let mut view: Box<dyn View> = Box::new(Filler::new(rect, WHITE));
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut tasks = Vec::new();
        let mut history = Vec::<HistoryItem>::new();
        let mut updating = Vec::new();
        let mut inactive_since = Instant::now();
        let mut runtime = DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            inactive_since: &mut inactive_since,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        f(&hub, &mut bus, &mut rq, &mut context, &mut runtime)
    }

    #[test]
    fn handle_toggle_frontlight_updates_settings() {
        let mut context = create_test_context();
        context.settings.frontlight = false;
        handle_toggle_frontlight(&mut context);
        assert!(context.settings.frontlight);
    }

    #[test]
    fn handle_event_toggle_frontlight_continues() {
        let outcome = with_runtime(|hub, bus, rq, context, runtime| {
            context.settings.frontlight = false;
            EmulatorDevice::handle_event(&Event::ToggleFrontlight, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Continue);
    }

    #[test]
    fn handle_event_restart_exits() {
        let outcome = with_runtime(|hub, bus, rq, context, runtime| {
            EmulatorDevice::handle_event(
                &Event::Select(EntryId::Restart),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Restart));
    }

    #[test]
    fn handle_event_power_off_exits() {
        let outcome = with_runtime(|hub, bus, rq, context, runtime| {
            EmulatorDevice::handle_event(
                &Event::Select(EntryId::PowerOff),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }
}

#[cfg(test)]
mod paths {
    use super::EmulatorDevice;
    use crate::device::DevicePaths as _;
    use crate::framebuffer::Pixmap;
    use std::path::Path;

    fn test_device() -> EmulatorDevice {
        EmulatorDevice::new(Box::new(Pixmap::new(600, 800, 1)))
    }

    #[test]
    fn data_dir_is_under_install_dir() {
        let device = test_device();
        let install_dir = device.install_dir();
        let data_dir = device.data_dir();
        assert!(
            data_dir.starts_with(&install_dir),
            "data_dir {:?} should be under install_dir {:?}",
            data_dir,
            install_dir
        );
        assert_eq!(data_dir, install_dir.join(device.data_subdir()));
    }

    #[test]
    fn data_path_dictionaries_ends_with_cadmus_dictionaries() {
        let device = test_device();
        let dict_path = device.data_path("dictionaries");
        assert!(
            dict_path.ends_with(Path::new(".cadmus").join("dictionaries")),
            "data_path(dictionaries) {:?} should end with .cadmus/dictionaries",
            dict_path
        );
    }
}
