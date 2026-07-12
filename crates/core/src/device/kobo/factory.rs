use super::device::Device;
use super::input::InputSource;
use super::model::Model;
use crate::device::linux::LinuxRtc;
use crate::device::metadata::DeviceMetadata;
use crate::device::{DeviceCapabilities, DeviceIdentity, DeviceRotation, FrontlightKind};
use crate::framebuffer::Framebuffer;
#[cfg(not(test))]
use crate::framebuffer::{KoboFramebuffer1, KoboFramebuffer2};
use crate::frontlight::{Frontlight, NaturalFrontlight, PremixedFrontlight, StandardFrontlight};
use anyhow::Context;

cfg_select! {
    test => {

const RTC_DEVICE: &str = "/dev/null";
    }
    _ => {

const RTC_DEVICE: &str = "/dev/rtc0";
    }
}

impl Model {
    /// Creates a `KoboDevice` for this model with the correct hardware properties.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn device(self) -> anyhow::Result<Device> {
        let mark = self.mark();
        let startup_rotation = self.startup_rotation();
        let has_gyroscope = self.has_gyroscope();
        let has_power_cover = self.has_power_cover();
        let has_lightsensor = self.has_lightsensor();
        let frontlight_kind = self.frontlight_kind();

        let metadata = DeviceMetadata::read().context("failed to read device metadata")?;

        let mut framebuffer: Box<dyn Framebuffer + Send> = cfg_select! {
            test => {
                Box::new(crate::framebuffer::Pixmap::new(self.dims().0, self.dims().1, 1))
            }
            _ => {
                if mark != 8 {
                    Box::new(
                        KoboFramebuffer1::new("/dev/fb0", mark, self.color_samples(), self)
                            .context("failed to create framebuffer")?,
                    )
                } else {
                    Box::new(
                        KoboFramebuffer2::new("/dev/fb0", startup_rotation)
                            .context("failed to create framebuffer")?,
                    )
                }
            }
        };
        let fb_rotation = framebuffer.rotation();
        let initial_rotation = self.transformed_rotation(fb_rotation);
        if !has_gyroscope && initial_rotation != startup_rotation {
            framebuffer.set_rotation(startup_rotation).ok();
        }

        let battery = Box::new(crate::battery::KoboBattery::new(has_power_cover)?)
            as Box<dyn crate::battery::Battery>;
        let lightsensor = if has_lightsensor {
            Box::new(crate::lightsensor::KoboLightSensor::new()?)
                as Box<dyn crate::lightsensor::LightSensor>
        } else {
            Box::new(0u16) as Box<dyn crate::lightsensor::LightSensor>
        };
        let frontlight = match frontlight_kind {
            FrontlightKind::Standard => {
                Box::new(StandardFrontlight::new(Default::default())?) as Box<dyn Frontlight>
            }
            FrontlightKind::Natural => Box::new(NaturalFrontlight::new(
                Default::default(),
                Default::default(),
                crate::device::Model::Kobo(self),
            )?) as Box<dyn Frontlight>,
            FrontlightKind::Premixed => Box::new(PremixedFrontlight::new(
                Default::default(),
                Default::default(),
                mark,
            )?) as Box<dyn Frontlight>,
        };
        let wifi_manager = std::sync::Arc::new(
            crate::device::kobo::wifi::KoboWifiManager::from_env()
                .context("failed to create WiFi manager")?,
        );
        let usb_manager = std::sync::Arc::new(
            crate::device::kobo::usb::KoboUsbManager::new(metadata.clone())
                .context("failed to create USB manager")?,
        );
        let power_manager =
            std::sync::Arc::new(crate::device::kobo::power::KoboPowerManager::new(self));
        let rtc =
            std::sync::Arc::new(LinuxRtc::new(RTC_DEVICE).context("failed to initialize RTC")?);
        let time_manager = crate::time_manager::TimeManager::new(rtc.clone(), |tz| {
            crate::device::linux::set_system_timezone(tz)
        });

        Ok(Device::new(
            self,
            metadata,
            framebuffer,
            battery,
            frontlight,
            lightsensor,
            wifi_manager,
            usb_manager,
            power_manager,
            rtc,
            time_manager,
            initial_rotation,
            InputSource {
                info: crate::input::DeviceInputInfo {
                    proto: self.proto(),
                    mark,
                    mirroring_scheme: self.mirroring_scheme(),
                    swapping_scheme: self.swapping_scheme(),
                    startup_rotation,
                    gyro_rotation_transform: crate::input::GyroRotationTransform::new(
                        self.gyro_rotation_fn(),
                    ),
                    swap_dims_on_rotation: mark == 8,
                },
                dpi: self.dpi(),
                raw_sender: None,
            },
        ))
    }
}
