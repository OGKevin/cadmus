//! Device detection and management.

use crate::device::error::DeviceError;
use crate::device::metadata::DeviceMetadata;
use crate::input::TouchProto;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use std::env;
use std::fmt::Debug;

mod error;
mod metadata;
mod model;
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
        match product {
            "kraken" => Device {
                model: Model::Glo,
                proto: TouchProto::Single,
                dims: (758, 1024),
                dpi: 212,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "pixie" => Device {
                model: Model::Mini,
                proto: TouchProto::Single,
                dims: (600, 800),
                dpi: 200,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "dragon" => Device {
                model: Model::AuraHD,
                proto: TouchProto::Single,
                dims: (1080, 1440),
                dpi: 265,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "phoenix" => Device {
                model: Model::Aura,
                proto: TouchProto::MultiA,
                dims: (758, 1024),
                dpi: 212,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "dahlia" => Device {
                model: Model::AuraH2O,
                proto: TouchProto::MultiA,
                dims: (1080, 1440),
                dpi: 265,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "alyssum" => Device {
                model: Model::GloHD,
                proto: TouchProto::MultiA,
                dims: (1072, 1448),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "pika" => Device {
                model: Model::Touch2,
                proto: TouchProto::MultiA,
                dims: (600, 800),
                dpi: 167,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "daylight" => Device {
                model: if model_number == "381" {
                    Model::AuraONELimEd
                } else {
                    Model::AuraONE
                },
                proto: TouchProto::MultiA,
                dims: (1404, 1872),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "star" => Device {
                model: if model_number == "379" {
                    Model::AuraEd2V2
                } else {
                    Model::AuraEd2V1
                },
                proto: TouchProto::MultiA,
                dims: (758, 1024),
                dpi: 212,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "snow" => Device {
                model: if model_number == "378" {
                    Model::AuraH2OEd2V2
                } else {
                    Model::AuraH2OEd2V1
                },
                proto: TouchProto::MultiB,
                dims: (1080, 1440),
                dpi: 265,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "nova" => Device {
                model: Model::ClaraHD,
                proto: TouchProto::MultiB,
                dims: (1072, 1448),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "frost" => Device {
                model: if model_number == "380" {
                    Model::Forma32GB
                } else {
                    Model::Forma
                },
                proto: TouchProto::MultiB,
                dims: (1440, 1920),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "storm" => Device {
                model: Model::LibraH2O,
                proto: TouchProto::MultiB,
                dims: (1264, 1680),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "luna" => Device {
                model: Model::Nia,
                proto: TouchProto::MultiA,
                dims: (758, 1024),
                dpi: 212,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "europa" => Device {
                model: Model::Elipsa,
                proto: TouchProto::MultiC,
                dims: (1404, 1872),
                dpi: 227,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "cadmus" => Device {
                model: Model::Sage,
                proto: TouchProto::MultiC,
                dims: (1440, 1920),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "io" => Device {
                model: Model::Libra2,
                proto: TouchProto::MultiC,
                dims: (1264, 1680),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "goldfinch" => Device {
                model: Model::Clara2E,
                proto: TouchProto::MultiB,
                dims: (1072, 1448),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "condor" => Device {
                model: Model::Elipsa2E,
                proto: TouchProto::MultiC,
                dims: (1404, 1872),
                dpi: 227,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "spaBW" | "spaBWTPV" => Device {
                model: Model::ClaraBW,
                proto: TouchProto::MultiB,
                dims: (1072, 1448),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "spaColour" => Device {
                model: Model::ClaraColour,
                proto: TouchProto::MultiB,
                dims: (1072, 1448),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            "monza" => Device {
                model: Model::LibraColour,
                proto: TouchProto::MultiB,
                dims: (1264, 1680),
                dpi: 300,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
            _ => Device {
                model: if model_number == "320" {
                    Model::TouchC
                } else {
                    Model::TouchAB
                },
                proto: TouchProto::Single,
                dims: (600, 800),
                dpi: 167,
                metadata: OnceCell::new(),
                wifi_manager: OnceCell::new(),
            },
        }
    }

    /// Gets device metadata (lazy initialization).
    pub fn metadata(&self) -> Result<&DeviceMetadata, DeviceError> {
        self.metadata.get_or_try_init(DeviceMetadata::read)
    }

    /// Creates USB manager for this device.
    #[cfg(feature = "kobo")]
    pub fn usb_manager(
        &self,
    ) -> Result<Box<dyn crate::device::usb::UsbManager>, crate::device::usb::UsbError> {
        let metadata = self
            .metadata()
            .map_err(|e| crate::device::usb::UsbError::DeviceInfo(e.to_string()))?
            .clone();
        crate::device::usb::create_usb_manager(metadata)
    }

    /// Creates stub USB manager (non-kobo builds).
    #[cfg(not(feature = "kobo"))]
    pub fn usb_manager(
        &self,
    ) -> Result<Box<dyn crate::device::usb::UsbManager>, crate::device::usb::UsbError> {
        Ok(Box::new(crate::device::usb::StubUsbManager))
    }

    /// Returns the WiFi manager for this device.
    pub fn wifi_manager(
        &self,
    ) -> Result<&dyn crate::device::wifi::WifiManager, crate::device::wifi::WifiError> {
        self.wifi_manager
            .get_or_try_init(crate::device::wifi::create_wifi_manager)
            .map(|b| b.as_ref())
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
