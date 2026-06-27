use crate::framebuffer::Display;
use crate::input::{InputEvent, device_events, raw_events, usb_events};
use crate::settings::ButtonScheme;
use crate::view::Event;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

pub(crate) const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const BATTERY_REFRESH_INTERVAL: Duration = Duration::from_secs(299);

fn touch_input_path() -> Option<String> {
    for path in [
        "/dev/input/by-path/platform-2-0010-event",
        "/dev/input/by-path/platform-1-0038-event",
        "/dev/input/by-path/platform-1-0010-event",
        "/dev/input/by-path/platform-0-0010-event",
        "/dev/input/event1",
    ] {
        if Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

/// Stub input source for Kobo devices.
///
/// The real implementation will wrap the evdev/USB input pipeline.
/// This stub exists so `KoboDevice` can declare `type Input = KoboInputSource`.
pub struct InputSource {
    pub(super) info: crate::input::DeviceInputInfo,
    pub(super) dpi: u16,
    pub(super) raw_sender: Option<Sender<InputEvent>>,
}

impl Default for InputSource {
    fn default() -> Self {
        Self {
            info: crate::input::DeviceInputInfo {
                proto: crate::input::TouchProto::Single,
                mark: 0,
                mirroring_scheme: (2, 1),
                swapping_scheme: 1,
                startup_rotation: 0,
                gyro_rotation_transform: crate::input::GyroRotationTransform::default(),
                swap_dims_on_rotation: false,
            },
            dpi: 300,
            raw_sender: None,
        }
    }
}

impl crate::device::InputSource for InputSource {
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, display, button_scheme),
            level = tracing::Level::TRACE,
            fields(proto = ?self.info.proto),
        )
    )]
    fn start(
        &mut self,
        display: Display,
        button_scheme: ButtonScheme,
    ) -> (
        crate::view::Hub,
        std::sync::mpsc::Receiver<crate::view::Event>,
    ) {
        let mut paths = Vec::new();
        let touch_path = touch_input_path();
        if let Some(path) = touch_path.as_ref() {
            paths.push(path.clone());
        }
        for bi in [
            "/dev/input/by-path/platform-gpio-keys-event",
            "/dev/input/by-path/platform-ntx_event0-event",
            "/dev/input/by-path/platform-mxckpd-event",
            "/dev/input/event0",
        ] {
            if Path::new(bi).exists() {
                paths.push(bi.to_string());
                break;
            }
        }
        for pi in [
            "/dev/input/by-path/platform-bd71828-pwrkey.6.auto-event",
            "/dev/input/by-path/platform-bd71828-pwrkey.4.auto-event",
            "/dev/input/by-path/platform-bd71828-pwrkey-event",
        ] {
            if Path::new(pi).exists() {
                paths.push(pi.to_string());
                break;
            }
        }

        if let Some(path) = touch_path.as_deref() {
            crate::input::trace_touch_device_geometry(path, self.info.proto, display);
        }

        tracing::trace!(
            touch_path = touch_path.as_deref(),
            input_paths = ?paths,
            mirroring_scheme = ?self.info.mirroring_scheme,
            swapping_scheme = self.info.swapping_scheme,
            startup_rotation = self.info.startup_rotation,
            mark = self.info.mark,
            "starting kobo input pipeline"
        );

        let (raw_sender, raw_receiver) = raw_events(paths);
        self.raw_sender = Some(raw_sender.clone());
        let touch_screen = crate::gesture::gesture_events(
            device_events(raw_receiver, display, button_scheme, self.info),
            self.dpi,
        );
        let usb_port = usb_events();
        let (tx, rx) = mpsc::channel();

        let tx2 = tx.clone();
        thread::spawn(move || {
            while let Ok(evt) = touch_screen.recv() {
                tx2.send(evt).ok();
            }
        });

        let tx3 = tx.clone();
        thread::spawn(move || {
            while let Ok(evt) = usb_port.recv() {
                tx3.send(Event::Device(evt)).ok();
            }
        });

        let tx4 = tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(CLOCK_REFRESH_INTERVAL);
                tx4.send(Event::ClockTick).ok();
            }
        });

        let tx5 = tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(BATTERY_REFRESH_INTERVAL);
                tx5.send(Event::BatteryTick).ok();
            }
        });

        (tx, rx)
    }

    /// Injects a synthetic evdev event into the raw input channel.
    ///
    /// Used when rotating the display (via [`crate::input::display_rotate_event`])
    /// or when restoring rotation after USB mass-storage export.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE, fields(
            kind = event.kind,
            code = event.code,
            value = event.value,
        ))
    )]
    fn send_raw(&self, event: InputEvent) {
        if event.kind == crate::input::EV_KEY && event.code == crate::input::KEY_ROTATE_DISPLAY {
            tracing::trace!(rotation = event.value, "injecting display_rotate_event");
        }
        if let Some(sender) = self.raw_sender.as_ref() {
            sender.send(event).ok();
        }
    }
}
