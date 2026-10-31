use crate::device::inhibitor::Inhibitor;
use crate::framebuffer::Display;
use crate::input::{TaggedInputEvent, device_events, raw_events, usb_events};
use crate::settings::ButtonScheme;
use crate::view::Event;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

pub(crate) const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const BATTERY_REFRESH_INTERVAL: Duration = Duration::from_secs(299);

/// Stub input source for Kobo devices.
///
/// The real implementation will wrap the evdev/USB input pipeline.
/// This stub exists so `KoboDevice` can declare `type Input = KoboInputSource`.
pub struct InputSource {
    pub(super) info: crate::input::DeviceInputInfo,
    pub(super) dpi: u16,
    pub(super) raw_sender: Option<UnboundedSender<TaggedInputEvent>>,
    pub(super) pipeline_cancel: Option<CancellationToken>,
    pub(super) gesture_job: Option<crate::runtime::Job<()>>,
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
            },
            dpi: 300,
            raw_sender: None,
            pipeline_cancel: None,
            gesture_job: None,
        }
    }
}

impl Drop for InputSource {
    fn drop(&mut self) {
        if let Some(cancel) = &self.pipeline_cancel {
            cancel.cancel();
        }
    }
}

impl crate::device::InputSource for InputSource {
    /// Starts the Kobo input pipeline and returns its event hub channel.
    ///
    /// Cancels any previously started pipeline (hotplug restart), then spawns:
    /// - `raw_events`: a feature scan over `/dev/input` that classifies every
    ///   evdev device (touch digitizer, gpio-keys, power key, and any
    ///   hotplugged keyboards/remotes) into a `DeviceProfile` that maps its
    ///   scancodes to semantic functions — no hard-coded device paths,
    /// - the gesture pipeline, which turns raw touch events into semantic
    ///   input events, and
    /// - `usb_events`, which watches for USB device (dis)connections that
    ///   trigger a pipeline restart.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, screen, button_scheme, inhibitor),
            level = tracing::Level::TRACE,
            fields(
                proto = ?self.info.proto,
                mark = self.info.mark,
                rotation = screen.rotation,
                dims = ?screen.dims,
            )
        )
    )]
    fn start(
        &mut self,
        screen: Display,
        button_scheme: ButtonScheme,
        inhibitor: Arc<Inhibitor>,
    ) -> (crate::view::Hub, crate::view::HubReceiver) {
        if let Some(cancel) = &self.pipeline_cancel {
            cancel.cancel();
        }
        let pipeline_cancel = CancellationToken::new();
        self.pipeline_cancel = Some(pipeline_cancel.clone());

        let (raw_sender, raw_receiver) =
            raw_events(screen, self.info.proto, pipeline_cancel.clone());
        self.raw_sender = Some(raw_sender.clone());
        if let Some(job) = self.gesture_job.take() {
            job.cancel();
        }
        let (touch_screen, gesture_job) = crate::gesture::GesturePipeline::start(
            device_events(raw_receiver, screen, button_scheme, self.info),
            self.dpi,
        )
        .into_parts();
        self.gesture_job = Some(gesture_job);
        let usb_port = usb_events();
        let (tx, rx) = crate::view::hub_channel();

        let tx2 = tx.clone();
        let inhibitor2 = Arc::clone(&inhibitor);
        let touch_cancel = pipeline_cancel.clone();
        crate::runtime::current_handle().spawn(async move {
            let mut touch_screen = touch_screen;
            loop {
                tokio::select! {
                    () = touch_cancel.cancelled() => break,
                    evt = touch_screen.recv() => match evt {
                        Some(evt) => {
                            crate::view::hub_message::send_input_hub_message(&tx2, &inhibitor2, evt);
                        }
                        None => break,
                    },
                }
            }
        });

        let tx3 = tx.clone();
        let inhibitor3 = Arc::clone(&inhibitor);
        let usb_cancel = pipeline_cancel.clone();
        crate::runtime::current_handle().spawn(async move {
            let mut usb_port = usb_port;
            loop {
                tokio::select! {
                    () = usb_cancel.cancelled() => break,
                    evt = usb_port.recv() => match evt {
                        Some(evt) => {
                            crate::view::hub_message::send_input_hub_message(
                                &tx3,
                                &inhibitor3,
                                Event::Device(evt),
                            );
                        }
                        None => break,
                    },
                }
            }
        });

        let tx4 = tx.clone();
        let clock_cancel = pipeline_cancel.clone();
        crate::runtime::current_handle().spawn(async move {
            loop {
                tokio::select! {
                    () = clock_cancel.cancelled() => break,
                    () = tokio::time::sleep(CLOCK_REFRESH_INTERVAL) => {
                        tx4.send(Event::ClockTick.into()).ok();
                    }
                }
            }
        });

        let tx5 = tx.clone();
        let battery_cancel = pipeline_cancel.clone();
        crate::runtime::current_handle().spawn(async move {
            loop {
                tokio::select! {
                    () = battery_cancel.cancelled() => break,
                    () = tokio::time::sleep(BATTERY_REFRESH_INTERVAL) => {
                        tx5.send(Event::BatteryTick.into()).ok();
                    }
                }
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
            kind = event.event.kind,
            code = event.event.code,
            value = event.event.value,
        ))
    )]
    fn send_raw(&self, event: TaggedInputEvent) {
        if event.event.kind == crate::input::EV_KEY
            && event.event.code == crate::input::KEY_ROTATE_DISPLAY
        {
            tracing::debug!(
                rotation = event.event.value,
                channel_ready = self.raw_sender.is_some(),
                "injecting display_rotate_event"
            );
        }
        if let Some(sender) = self.raw_sender.as_ref() {
            sender.send(event).ok();
        }
    }
}
