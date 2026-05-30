//! Device detection and management.

use crate::device::error::DeviceError;
use crate::device::metadata::DeviceMetadata;
use crate::input::TouchProto;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use std::env;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

mod error;
mod metadata;
mod model;
mod power;
mod types;
mod usb;
mod wifi;

pub use model::Model;
pub use types::{FrontlightKind, Orientation};

pub struct Device {
    pub model: Model,
    pub proto: TouchProto,
    pub dims: (u32, u32),
    pub dpi: u16,
    metadata: OnceCell<DeviceMetadata>,
    wifi_manager: OnceCell<Box<dyn crate::device::wifi::WifiManager>>,
    power_manager: OnceCell<Box<dyn crate::device::power::PowerManager>>,
}

impl Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("model", &self.model)
            .field("proto", &self.proto)
            .field("dims", &self.dims)
            .field("dpi", &self.dpi)
            .finish()
    }
}
impl Device {
    /// Creates a new device from product and model number strings.
    fn new(product: &str, model_number: &str) -> Device {
        let (model, proto, dims, dpi) = match product {
            "kraken" => (Model::Glo, TouchProto::Single, (758, 1024), 212),
            "pixie" => (Model::Mini, TouchProto::Single, (600, 800), 200),
            "dragon" => (Model::AuraHD, TouchProto::Single, (1080, 1440), 265),
            "phoenix" => (Model::Aura, TouchProto::MultiA, (758, 1024), 212),
            "dahlia" => (Model::AuraH2O, TouchProto::MultiA, (1080, 1440), 265),
            "alyssum" => (Model::GloHD, TouchProto::MultiA, (1072, 1448), 300),
            "pika" => (Model::Touch2, TouchProto::MultiA, (600, 800), 167),
            "daylight" => {
                let model = if model_number == "381" {
                    Model::AuraONELimEd
                } else {
                    Model::AuraONE
                };
                (model, TouchProto::MultiA, (1404, 1872), 300)
            }
            "star" => {
                let model = if model_number == "379" {
                    Model::AuraEd2V2
                } else {
                    Model::AuraEd2V1
                };
                (model, TouchProto::MultiA, (758, 1024), 212)
            }
            "snow" => {
                let model = if model_number == "378" {
                    Model::AuraH2OEd2V2
                } else {
                    Model::AuraH2OEd2V1
                };
                (model, TouchProto::MultiB, (1080, 1440), 265)
            }
            "nova" => (Model::ClaraHD, TouchProto::MultiB, (1072, 1448), 300),
            "frost" => {
                let model = if model_number == "380" {
                    Model::Forma32GB
                } else {
                    Model::Forma
                };
                (model, TouchProto::MultiB, (1440, 1920), 300)
            }
            "storm" => (Model::LibraH2O, TouchProto::MultiB, (1264, 1680), 300),
            "luna" => (Model::Nia, TouchProto::MultiA, (758, 1024), 212),
            "europa" => (Model::Elipsa, TouchProto::MultiC, (1404, 1872), 227),
            "cadmus" => (Model::Sage, TouchProto::MultiC, (1440, 1920), 300),
            "io" => (Model::Libra2, TouchProto::MultiC, (1264, 1680), 300),
            "goldfinch" => (Model::Clara2E, TouchProto::MultiB, (1072, 1448), 300),
            "condor" => (Model::Elipsa2E, TouchProto::MultiC, (1404, 1872), 227),
            "spaBW" | "spaBWTPV" => (Model::ClaraBW, TouchProto::MultiB, (1072, 1448), 300),
            "spaColour" => (Model::ClaraColour, TouchProto::MultiB, (1072, 1448), 300),
            "monza" => (Model::LibraColour, TouchProto::MultiB, (1264, 1680), 300),
            _ => {
                let model = if model_number == "320" {
                    Model::TouchC
                } else {
                    Model::TouchAB
                };
                (model, TouchProto::Single, (600, 800), 167)
            }
        };

        Device {
            model,
            proto,
            dims,
            dpi,
            metadata: OnceCell::new(),
            wifi_manager: OnceCell::new(),
            power_manager: OnceCell::new(),
        }
    }

    /// Gets device metadata (lazy initialization).
    pub fn metadata(&self) -> Result<&DeviceMetadata, DeviceError> {
        self.metadata.get_or_try_init(DeviceMetadata::read)
    }

    /// Creates USB manager for this device.
    pub fn usb_manager(
        &self,
    ) -> Result<Box<dyn crate::device::usb::UsbManager>, crate::device::usb::UsbError> {
        cfg_select! {
            feature = "kobo" => {
               let metadata = self
                   .metadata()
                   .map_err(|e| crate::device::usb::UsbError::DeviceInfo(e.to_string()))?
                   .clone();
               crate::device::usb::create_usb_manager(metadata)
            }
            _ => {
               Ok(Box::new(crate::device::usb::StubUsbManager))
            }
        }
    }

    /// Returns the WiFi manager for this device.
    pub fn wifi_manager(
        &self,
    ) -> Result<&dyn crate::device::wifi::WifiManager, crate::device::wifi::WifiError> {
        self.wifi_manager
            .get_or_try_init(crate::device::wifi::create_wifi_manager)
            .map(|b| b.as_ref())
    }

    /// Returns the Power manager for this device.
    pub fn power_manager(
        &self,
    ) -> Result<&dyn crate::device::power::PowerManager, crate::device::power::PowerError> {
        self.power_manager
            .get_or_try_init(|| crate::device::power::create_power_manager(self.model))
            .map(|b| b.as_ref())
    }

    /// Returns the install subdirectory for this build.
    ///
    /// Kobo devices install Cadmus under `.adds/` on the user-visible storage.
    /// Test builds use a separate sibling directory so they can coexist with
    /// stable builds.
    pub fn install_subdir(&self) -> &'static str {
        cfg_select! {
            feature = "test" => { ".adds/cadmus-tst" }
            _ => { ".adds/cadmus" }
        }
    }

    /// Returns the absolute install directory for this device.
    ///
    /// The path is determined at compile time and does not depend on the
    /// process's current working directory, so it remains stable even when
    /// callers change `cwd`.
    ///
    /// - Normal device builds: `/mnt/onboard/.adds/cadmus`
    /// - Test device builds: `/mnt/onboard/.adds/cadmus-tst`
    /// - Emulator builds: `/tmp/.adds/cadmus` (or `cadmus-tst` with `test`)
    /// - Unit tests: `<temp_dir>/test-kobo-installation/.adds/cadmus-tst`
    pub fn install_dir(&self) -> PathBuf {
        cfg_select! {
            test => {
                std::env::temp_dir()
                    .join("test-kobo-installation")
                    .join(self.install_subdir())
            }
            feature = "emulator" => {
                PathBuf::from("/tmp").join(self.install_subdir())
            }
            _ => {
                PathBuf::from(crate::settings::INTERNAL_CARD_ROOT).join(self.install_subdir())
            }
        }
    }

    /// Returns a path inside the device install directory.
    ///
    /// Use this for files and directories that Cadmus owns under its install
    /// root, such as `tmp/` or `.github_token`.
    pub fn install_path(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.install_dir().join(relative_path)
    }

    /// Returns the path to the device-managed tmp directory.
    ///
    /// The returned path is rooted under [`Device::install_dir`], so it remains
    /// stable even when callers change `cwd` (for example during USB sharing).
    pub fn tmp_dir(&self) -> PathBuf {
        self.install_path("tmp")
    }

    /// Removes stale contents left by a previous run and recreates the tmp
    /// directory.
    ///
    /// `Device` owns the lifecycle of the tmp directory: callers may assume
    /// the directory exists after this runs and should not create it
    /// themselves. Call this once at startup before any feature that writes
    /// to `tmp_dir()` to ensure a clean slate.
    pub fn clean_tmp_dir(&self) {
        let dir = self.tmp_dir();
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = ?dir, error = %e, "Failed to clean tmp dir");
            }
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = ?dir, error = %e, "Failed to create tmp dir");
        }
    }

    /// Returns the number of color samples for the device screen.
    pub fn color_samples(&self) -> usize {
        match self.model {
            Model::ClaraColour | Model::LibraColour => 3,
            _ => 1,
        }
    }

    /// Returns the frontlight kind for this device.
    pub fn frontlight_kind(&self) -> FrontlightKind {
        match self.model {
            Model::ClaraHD
            | Model::Forma
            | Model::Forma32GB
            | Model::LibraH2O
            | Model::Sage
            | Model::Libra2
            | Model::Clara2E
            | Model::Elipsa2E
            | Model::ClaraBW
            | Model::ClaraColour
            | Model::LibraColour => FrontlightKind::Premixed,
            Model::AuraONE | Model::AuraONELimEd | Model::AuraH2OEd2V1 | Model::AuraH2OEd2V2 => {
                FrontlightKind::Natural
            }
            _ => FrontlightKind::Standard,
        }
    }

    /// Returns true if the device has natural light capability.
    pub fn has_natural_light(&self) -> bool {
        self.frontlight_kind() != FrontlightKind::Standard
    }

    /// Returns true if the device has a light sensor.
    pub fn has_lightsensor(&self) -> bool {
        matches!(self.model, Model::AuraONE | Model::AuraONELimEd)
    }

    /// Returns true if the device has a gyroscope.
    pub fn has_gyroscope(&self) -> bool {
        matches!(
            self.model,
            Model::Forma
                | Model::Forma32GB
                | Model::LibraH2O
                | Model::Elipsa
                | Model::Sage
                | Model::Libra2
                | Model::Elipsa2E
                | Model::LibraColour
        )
    }

    /// Returns true if the device has page turn buttons.
    pub fn has_page_turn_buttons(&self) -> bool {
        matches!(
            self.model,
            Model::Forma
                | Model::Forma32GB
                | Model::LibraH2O
                | Model::Sage
                | Model::Libra2
                | Model::LibraColour
        )
    }

    /// Returns true if the device supports a power cover.
    pub fn has_power_cover(&self) -> bool {
        matches!(self.model, Model::Sage)
    }

    /// Returns true if the device has removable storage.
    pub fn has_removable_storage(&self) -> bool {
        matches!(
            self.model,
            Model::AuraH2O
                | Model::Aura
                | Model::AuraHD
                | Model::Glo
                | Model::TouchAB
                | Model::TouchC
        )
    }

    /// Returns true if buttons should be inverted for the given rotation.
    pub fn should_invert_buttons(&self, rotation: i8) -> bool {
        let sr = self.startup_rotation();
        let (_, dir) = self.mirroring_scheme();

        rotation == (4 + sr - dir) % 4 || rotation == (4 + sr - 2 * dir) % 4
    }

    /// Returns the orientation for the given rotation.
    pub fn orientation(&self, rotation: i8) -> Orientation {
        if self.should_swap_axes(rotation) {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }

    /// Returns the device mark value.
    pub fn mark(&self) -> u8 {
        match self.model {
            Model::LibraColour => 13,
            Model::ClaraBW | Model::ClaraColour => 12,
            Model::Elipsa2E => 11,
            Model::Clara2E => 10,
            Model::Libra2 => 9,
            Model::Sage | Model::Elipsa => 8,
            Model::Nia
            | Model::LibraH2O
            | Model::Forma32GB
            | Model::Forma
            | Model::ClaraHD
            | Model::AuraH2OEd2V2
            | Model::AuraEd2V2 => 7,
            Model::AuraH2OEd2V1
            | Model::AuraEd2V1
            | Model::AuraONELimEd
            | Model::AuraONE
            | Model::Touch2
            | Model::GloHD => 6,
            Model::AuraH2O | Model::Aura => 5,
            Model::AuraHD | Model::Mini | Model::Glo | Model::TouchC => 4,
            Model::TouchAB => 3,
        }
    }

    /// Returns whether axes should be mirrored for the given rotation.
    pub fn should_mirror_axes(&self, rotation: i8) -> (bool, bool) {
        let (mxy, dir) = self.mirroring_scheme();
        let mx = (4 + (mxy + dir)) % 4;
        let my = (4 + (mxy - dir)) % 4;
        let mirror_x = mxy == rotation || mx == rotation;
        let mirror_y = mxy == rotation || my == rotation;
        (mirror_x, mirror_y)
    }

    /// Returns the center and direction of the mirroring pattern.
    pub fn mirroring_scheme(&self) -> (i8, i8) {
        match self.model {
            Model::AuraH2OEd2V1 | Model::LibraH2O | Model::Libra2 => (3, 1),
            Model::Sage => (0, 1),
            Model::AuraH2OEd2V2 => (0, -1),
            Model::Forma | Model::Forma32GB => (2, -1),
            _ => (2, 1),
        }
    }

    /// Returns true if axes should be swapped for the given rotation.
    pub fn should_swap_axes(&self, rotation: i8) -> bool {
        rotation % 2 == self.swapping_scheme()
    }

    /// Returns the swapping scheme value.
    fn swapping_scheme(&self) -> i8 {
        match self.model {
            Model::LibraH2O => 0,
            _ => 1,
        }
    }

    /// Returns the startup rotation value.
    pub fn startup_rotation(&self) -> i8 {
        match self.model {
            Model::LibraH2O => 0,
            Model::AuraH2OEd2V1
            | Model::Forma
            | Model::Forma32GB
            | Model::Sage
            | Model::Libra2
            | Model::Elipsa2E
            | Model::LibraColour => 1,
            _ => 3,
        }
    }

    /// Returns a device independent rotation value.
    pub fn to_canonical(&self, n: i8) -> i8 {
        let (_, dir) = self.mirroring_scheme();
        (4 + dir * (n - self.startup_rotation())) % 4
    }

    /// Returns a device dependent rotation value from canonical.
    pub fn from_canonical(&self, n: i8) -> i8 {
        let (_, dir) = self.mirroring_scheme();
        (self.startup_rotation() + (4 + dir * n) % 4) % 4
    }

    /// Returns the transformed rotation value.
    pub fn transformed_rotation(&self, n: i8) -> i8 {
        match self.model {
            Model::AuraHD | Model::AuraH2O => n ^ 2,
            Model::AuraH2OEd2V2 | Model::Forma | Model::Forma32GB => (4 - n) % 4,
            _ => n,
        }
    }

    /// Returns the transformed gyroscope rotation value.
    pub fn transformed_gyroscope_rotation(&self, n: i8) -> i8 {
        match self.model {
            Model::LibraH2O => n ^ 1,
            Model::Libra2 | Model::Sage | Model::Elipsa2E | Model::LibraColour => (6 - n) % 4,
            Model::Elipsa => (4 - n) % 4,
            _ => n,
        }
    }
}

lazy_static! {
    // TODO(OGKevin): we shan't rely on these env variables to construct the device, and instead
    //                do discovery here instead of in the bash script.
    /// Global singleton for the current device.
    pub static ref CURRENT_DEVICE: Device = {
        let product = env::var("PRODUCT").unwrap_or_default();
        let model_number = env::var("MODEL_NUMBER").unwrap_or_default();

        Device::new(&product, &model_number)
    };
}

#[cfg(test)]
mod tests {
    use super::Device;

    #[test]
    fn test_device_canonical_rotation() {
        let forma = Device::new("frost", "377");
        let aura_one = Device::new("daylight", "373");
        for n in 0..4 {
            assert_eq!(forma.from_canonical(forma.to_canonical(n)), n);
        }
        assert_eq!(aura_one.from_canonical(0), aura_one.startup_rotation());
        assert_eq!(
            forma.from_canonical(1) - forma.from_canonical(0),
            aura_one.from_canonical(2) - aura_one.from_canonical(3)
        );
    }
}
