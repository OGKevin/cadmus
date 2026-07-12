use super::input::InputSource;
use super::model::Model;
use crate::device::DeviceIdentity;
use crate::device::linux::LinuxRtc;
use crate::device::metadata::DeviceMetadata;
use std::env;
use std::fmt::Debug;
use std::path::PathBuf;

pub struct Device {
    model: Model,
    metadata: DeviceMetadata,
    framebuffer: Box<dyn crate::framebuffer::Framebuffer + Send>,
    battery: Box<dyn crate::battery::Battery>,
    frontlight: Box<dyn crate::frontlight::Frontlight>,
    lightsensor: Box<dyn crate::lightsensor::LightSensor>,
    wifi_manager: std::sync::Arc<crate::device::kobo::wifi::KoboWifiManager>,
    usb_manager: std::sync::Arc<crate::device::kobo::usb::KoboUsbManager>,
    power_manager: std::sync::Arc<crate::device::kobo::power::KoboPowerManager>,
    rtc: std::sync::Arc<LinuxRtc>,
    time_manager: crate::time_manager::TimeManager<LinuxRtc>,
    input: InputSource,
    boot_transformed_rotation: i8,
}

impl Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KoboDevice")
            .field("model", &self.model)
            .field("proto", &self.proto())
            .field("dims", &self.dims())
            .field("dpi", &self.dpi())
            .finish()
    }
}

impl Device {
    #[expect(
        clippy::too_many_arguments,
        reason = "device bundles all hardware subsystems"
    )]
    pub(super) fn new(
        model: Model,
        metadata: DeviceMetadata,
        framebuffer: Box<dyn crate::framebuffer::Framebuffer + Send>,
        battery: Box<dyn crate::battery::Battery>,
        frontlight: Box<dyn crate::frontlight::Frontlight>,
        lightsensor: Box<dyn crate::lightsensor::LightSensor>,
        wifi_manager: std::sync::Arc<crate::device::kobo::wifi::KoboWifiManager>,
        usb_manager: std::sync::Arc<crate::device::kobo::usb::KoboUsbManager>,
        power_manager: std::sync::Arc<crate::device::kobo::power::KoboPowerManager>,
        rtc: std::sync::Arc<LinuxRtc>,
        time_manager: crate::time_manager::TimeManager<LinuxRtc>,
        boot_transformed_rotation: i8,
        input: InputSource,
    ) -> Self {
        Self {
            model,
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
            input,
            boot_transformed_rotation,
        }
    }

    /// Creates a device from product and model number strings.
    fn from_product(product: &str, model_number: &str) -> anyhow::Result<Self> {
        Model::new(product, model_number).device()
    }
}

/// Kobo install and data directory layout.
///
/// Install root: `/mnt/onboard/.adds/cadmus` (or `cadmus-tst` for test builds).
/// Data root: `/mnt/sd/.cadmus` when removable storage is mounted, else install root.
impl crate::device::DevicePaths for Device {
    fn install_subdir(&self) -> &'static str {
        cfg_select! {
            feature = "test" => { ".adds/cadmus-tst" }
            _ => { ".adds/cadmus" }
        }
    }

    fn install_dir(&self) -> PathBuf {
        cfg_select! {
            test => {
                std::env::temp_dir()
                    .join("test-kobo-installation")
                    .join(self.install_subdir())
            }
            _ => {
                PathBuf::from(crate::settings::INTERNAL_CARD_ROOT).join(self.install_subdir())
            }
        }
    }

    fn data_subdir(&self) -> &'static str {
        cfg_select! {
            feature = "test" => { ".cadmus-tst" }
            _ => { ".cadmus" }
        }
    }

    fn data_dir(&self) -> PathBuf {
        cfg_select! {
            test => { self.install_dir() }
            _ => {
                if crate::device::DeviceCapabilities::has_removable_storage(self)
                    && std::path::Path::new(crate::settings::EXTERNAL_CARD_ROOT).is_dir()
                {
                    PathBuf::from(crate::settings::EXTERNAL_CARD_ROOT)
                        .join(self.data_subdir())
                } else {
                    self.install_dir()
                }
            }
        }
    }
}

crate::impl_device_hardware!(
    Device,
    Framebuffer = Box<dyn crate::framebuffer::Framebuffer + Send>,
    Battery = Box<dyn crate::battery::Battery>,
    Frontlight = Box<dyn crate::frontlight::Frontlight>,
    LightSensor = Box<dyn crate::lightsensor::LightSensor>,
    WifiManager = crate::device::kobo::wifi::KoboWifiManager,
    UsbManager = crate::device::kobo::usb::KoboUsbManager,
    PowerManager = crate::device::kobo::power::KoboPowerManager,
    Rtc = LinuxRtc;
    override
        metadata_from metadata,
        set_system_timezone linux,
        refresh_framebuffer_from_kernel framebuffer,
);

impl crate::device::DeviceInput for Device {
    type Input = InputSource;

    fn input(&self) -> &Self::Input {
        &self.input
    }

    fn input_mut(&mut self) -> &mut Self::Input {
        &mut self.input
    }
}

impl Default for Device {
    /// Builds a device from the `PRODUCT` and `MODEL_NUMBER` environment variables.
    ///
    /// # Panics
    ///
    /// Panics if hardware initialization fails. Cadmus cannot run without a
    /// working device; callers should treat init failure as fatal.
    fn default() -> Self {
        let product = env::var("PRODUCT").unwrap_or_default();
        let model_number = env::var("MODEL_NUMBER").unwrap_or_default();
        Device::from_product(&product, &model_number).expect("failed to initialize device")
    }
}

crate::forward_device_identity!(Device, model);
crate::forward_device_capabilities!(Device, model);
crate::forward_device_rotation!(Device, model, boot = boot_transformed_rotation);

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::{DevicePaths as _, DeviceRotation as _};

    mod paths {
        use super::*;

        #[test]
        fn install_subdir() {
            let d = Model::Sage.device().unwrap();
            let subdir = d.install_subdir();
            assert!(
                subdir == ".adds/cadmus" || subdir == ".adds/cadmus-tst" || subdir.is_empty(),
                "install_subdir returned {subdir:?}"
            );
        }

        #[test]
        fn data_subdir() {
            let d = Model::Sage.device().unwrap();
            let subdir = d.data_subdir();
            assert!(
                subdir == ".cadmus" || subdir == ".cadmus-tst",
                "data_subdir returned {subdir:?}"
            );
        }

        #[test]
        fn install_dir_ends_with_install_subdir() {
            let d = Model::Sage.device().unwrap();
            let install_dir = d.install_dir();
            let subdir = d.install_subdir();
            if !subdir.is_empty() {
                assert!(
                    install_dir.ends_with(subdir),
                    "install_dir {:?} should end with {:?}",
                    install_dir,
                    subdir
                );
            }
        }

        #[test]
        fn install_path_joins_install_dir() {
            let d = Model::Sage.device().unwrap();
            let relative = std::path::Path::new("tmp");
            let expected = d.install_dir().join(relative);
            assert_eq!(d.install_path(relative), expected);
        }

        #[test]
        fn data_path_joins_data_dir() {
            let d = Model::Sage.device().unwrap();
            let relative = std::path::Path::new("cadmus.sqlite");
            let expected = d.data_dir().join(relative);
            assert_eq!(d.data_path(relative), expected);
        }

        #[test]
        fn tmp_dir_is_data_path_tmp() {
            let d = Model::Sage.device().unwrap();
            let expected = d.data_path(std::path::Path::new("tmp"));
            assert_eq!(d.tmp_dir(), expected);
        }

        #[test]
        fn resolve_db_path_prefers_data_dir() {
            let d = Model::Sage.device().unwrap();
            let db_path = d.resolve_db_path();
            let expected = d.data_path(crate::db::DB_FILENAME);
            let install_expected = d.install_path(crate::db::DB_FILENAME);
            assert!(
                db_path == expected || db_path == install_expected,
                "resolve_db_path {:?} should be either data_dir or install_dir location",
                db_path
            );
        }
    }

    #[test]
    fn boot_transformed_rotation_stored_at_init() {
        let device = Model::Glo.device().unwrap();
        assert!((0..4).contains(&device.boot_transformed_rotation()));
    }
}
