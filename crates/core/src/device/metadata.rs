//! Device metadata from the Kobo version file.
//!
//! # Version File
//!
//! The version file at `/mnt/onboard/.kobo/version` is a single
//! comma-separated line:
//!
//! ```text
//! NXXXXXXXXXXXXX,4.9.77,4.45.23640,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390
//! ```
//!
//! | Index | Field            | Example                                         |
//! |-------|------------------|-------------------------------------------------|
//! | 0     | Serial number    | `NXXXXXXXXXXXXX`                                |
//! | 1     | Kernel version   | `4.9.77`                                        |
//! | 2     | Firmware version | `4.45.23640`                                    |
//! | 3     | Kernel version   | `4.9.77`                                        |
//! | 4     | Kernel version   | `4.9.77`                                        |
//! | 5     | Model number     | `390` or `00000000-0000-0000-0000-000000000390` |
//!
//! # Model Number to Product ID Mapping
//!
//! The model number from field 5 maps to a USB Product ID and a
//! [`Model`](crate::device::Model) variant:
//!
//! | Model number(s) | Product ID | [`Model`](crate::device::Model)                                        |
//! |-----------------|------------|------------------------------------------------------------------------|
//! | 310, 320        | `0x4163`   | [`TouchAB`](crate::device::Model::TouchAB), [`TouchC`](crate::device::Model::TouchC) |
//! | 330             | `0x4173`   | [`Glo`](crate::device::Model::Glo)                                     |
//! | 340             | `0x4183`   | [`Mini`](crate::device::Model::Mini)                                   |
//! | 350             | `0x4193`   | [`AuraHD`](crate::device::Model::AuraHD)                               |
//! | 360             | `0x4203`   | [`Aura`](crate::device::Model::Aura)                                   |
//! | 370             | `0x4213`   | [`AuraH2O`](crate::device::Model::AuraH2O)                             |
//! | 371             | `0x4223`   | [`GloHD`](crate::device::Model::GloHD)                                 |
//! | 372             | `0x4224`   | [`Touch2`](crate::device::Model::Touch2)                               |
//! | 373, 381        | `0x4225`   | [`AuraONE`](crate::device::Model::AuraONE), [`AuraONELimEd`](crate::device::Model::AuraONELimEd) |
//! | 374             | `0x4227`   | [`AuraH2OEd2V1`](crate::device::Model::AuraH2OEd2V1), [`AuraH2OEd2V2`](crate::device::Model::AuraH2OEd2V2) |
//! | 375             | `0x4226`   | [`AuraEd2V1`](crate::device::Model::AuraEd2V1), [`AuraEd2V2`](crate::device::Model::AuraEd2V2) |
//! | 376             | `0x4228`   | [`ClaraHD`](crate::device::Model::ClaraHD)                             |
//! | 377, 380        | `0x4229`   | [`Forma`](crate::device::Model::Forma), [`Forma32GB`](crate::device::Model::Forma32GB) |
//! | 378             | `0x4227`   | [`AuraH2OEd2V2`](crate::device::Model::AuraH2OEd2V2) (V2 hardware)    |
//! | 379             | `0x4226`   | [`AuraEd2V2`](crate::device::Model::AuraEd2V2) (V2 hardware)          |
//! | 382             | `0x4230`   | [`Nia`](crate::device::Model::Nia)                                     |
//! | 383             | `0x4231`   | [`Sage`](crate::device::Model::Sage)                                   |
//! | 384             | `0x4232`   | [`LibraH2O`](crate::device::Model::LibraH2O)                           |
//! | 387             | `0x4233`   | [`Elipsa`](crate::device::Model::Elipsa)                               |
//! | 386             | `0x4235`   | [`Clara2E`](crate::device::Model::Clara2E)                             |
//! | 388             | `0x4234`   | [`Libra2`](crate::device::Model::Libra2)                               |
//! | 389             | `0x4236`   | [`Elipsa2E`](crate::device::Model::Elipsa2E)                           |
//! | 390             | `0x4237`   | [`LibraColour`](crate::device::Model::LibraColour)                     |
//! | 391, 395        | `0x4239`   | [`ClaraBW`](crate::device::Model::ClaraBW)                             |
//! | 393             | `0x4238`   | [`ClaraColour`](crate::device::Model::ClaraColour)                     |

use crate::device::error::DeviceError;
use std::env;
use std::fs;
use tracing::{debug, error, info, warn};

/// Represents the hardware platform of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Platform {
    /// MediaTek MT8113T NTX platform (e.g. Kobo Libra Colour).
    MT8113TNTX,
    /// Freescale i.MX 6SLL NTX platform.
    MX6SLLNTX,
    /// Freescale i.MX 6ULL NTX platform.
    MX6ULLNTX,
    /// Freescale i.MX 6UL NTX platform.
    MX6SULNTX,
    /// Freescale i.MX 6SL NTX platform.
    MX6SLNTX,
    /// Any other platform, with the raw identifier string preserved.
    Other(String),
}

impl From<String> for Platform {
    fn from(s: String) -> Self {
        match s.as_str() {
            "mt8113t-ntx" => Platform::MT8113TNTX,
            "mx6sll-ntx" => Platform::MX6SLLNTX,
            "mx6ull-ntx" => Platform::MX6ULLNTX,
            "mx6sul-ntx" => Platform::MX6SULNTX,
            "mx6sl-ntx" => Platform::MX6SLNTX,
            _ => Platform::Other(s),
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::MT8113TNTX => write!(f, "mt8113t-ntx"),
            Platform::MX6SLLNTX => write!(f, "mx6sll-ntx"),
            Platform::MX6ULLNTX => write!(f, "mx6ull-ntx"),
            Platform::MX6SULNTX => write!(f, "mx6sul-ntx"),
            Platform::MX6SLNTX => write!(f, "mx6sl-ntx"),
            Platform::Other(s) => write!(f, "{}", s),
        }
    }
}

const VERSION_PATH: &str = "/mnt/onboard/.kobo/version";
const VENDOR_ID: u16 = 0x2237;

/// Device metadata read from Kobo version file.
#[derive(Debug, Clone)]
pub struct DeviceMetadata {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: String,
    pub firmware_version: String,
    pub partition: String,
    pub manufacturer: String,
    pub product: String,
}

impl DeviceMetadata {
    /// Reads device metadata from `/mnt/onboard/.kobo/version`.
    ///
    /// The version file is a single comma-separated line with the following fields:
    ///
    /// | Index | Field            | Example                                  |
    /// |-------|------------------|------------------------------------------|
    /// | 0     | Serial number    | `NXXXXXXXXXXXXX`                         |
    /// | 1     | Kernel Version   | `4.9.77`                                 |
    /// | 2     | Firmware version | `4.45.23640`                             |
    /// | 3     | Kernel Version   | `4.9.77`                                 |
    /// | 4     | Kernel Version   | `4.9.77`                                 |
    /// | 5     | Model number     | `390` or `00000000-0000-0000-0000-000000000390` |
    ///
    /// Example file contents (Libra Colour):
    /// ```text
    /// NXXXXXXXXXXXXX,4.9.77,4.45.23640,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] if:
    /// - the version file cannot be read or has fewer than 6 fields, or
    /// - the `PLATFORM` environment variable is not set.
    pub fn read() -> Result<Self, DeviceError> {
        let content = fs::read_to_string(VERSION_PATH).map_err(|e| {
            error!(path = VERSION_PATH, error = %e, "Failed to read Kobo version file");
            DeviceError::Metadata(format!("Cannot read version file: {}", e))
        })?;

        let platform = detect_platform()?;
        Self::parse(&content, &platform)
    }

    fn parse(content: &str, platform: &Platform) -> Result<Self, DeviceError> {
        let fields: Vec<&str> = content.trim().split(',').collect();

        if fields.len() < 6 {
            error!(
                field_count = fields.len(),
                "Kobo version file has insufficient fields"
            );
            return Err(DeviceError::Metadata(
                "Version file format unexpected: insufficient fields".to_string(),
            ));
        }

        let serial_number = fields[0].to_string();
        let firmware_version = fields[2].to_string();

        let raw = fields[5].trim();
        let model_number = raw
            .rsplit('-')
            .next()
            .unwrap_or(raw)
            .trim_start_matches('0');

        let product_id = model_to_product_id(model_number);
        let partition = platform_to_partition(platform).to_string();
        let product = format!("eReader-{}", firmware_version);

        info!(
            serial_number = %serial_number,
            firmware_version = %firmware_version,
            model_number = %model_number,
            product_id,
            product_id_hex = %format_args!("{product_id:#06X}"),
            partition = %partition,
            "Device metadata read successfully"
        );

        Ok(Self {
            vendor_id: VENDOR_ID,
            product_id,
            serial_number,
            firmware_version,
            partition,
            manufacturer: "Kobo".to_string(),
            product,
        })
    }
}

/// Maps a Kobo model number to its USB Product ID.
fn model_to_product_id(model_number: &str) -> u16 {
    let product_id = match model_number {
        "320" | "310" => 0x4163,
        "330" => 0x4173,
        "340" => 0x4183,
        "350" => 0x4193,
        "360" => 0x4203,
        "370" => 0x4213,
        "371" => 0x4223,
        "372" => 0x4224,
        "373" | "381" => 0x4225,
        "374" => 0x4227,
        "375" => 0x4226,
        "376" => 0x4228,
        "377" | "380" => 0x4229,
        "378" => 0x4227,
        "379" => 0x4226,
        "384" => 0x4232,
        "382" => 0x4230,
        "387" => 0x4233,
        "383" => 0x4231,
        "388" => 0x4234,
        "386" => 0x4235,
        "389" => 0x4236,
        "390" => 0x4237,
        "393" => 0x4238,
        "391" | "395" => 0x4239,
        _ => 0x6666,
    };

    if product_id == 0x6666 {
        warn!(model_number = %model_number, "Unknown model number, using default Product ID");
    } else {
        debug!(model_number = %model_number, product_id, "Mapped model to Product ID");
    }

    product_id
}

/// Detects the platform type from the PLATFORM environment variable.
pub(crate) fn detect_platform() -> Result<Platform, DeviceError> {
    env::var("PLATFORM")
        .map(Platform::from)
        .map_err(|_| DeviceError::Metadata("PLATFORM environment variable not set".to_string()))
}

fn platform_to_partition(platform: &Platform) -> &'static str {
    match platform {
        Platform::MT8113TNTX => "/dev/mmcblk0p12",
        _ => "/dev/mmcblk0p3",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_to_product_id() {
        assert_eq!(model_to_product_id("376"), 0x4228);
        assert_eq!(model_to_product_id("377"), 0x4229);
        assert_eq!(model_to_product_id("378"), 0x4227);
        assert_eq!(model_to_product_id("379"), 0x4226);
        assert_eq!(model_to_product_id("380"), 0x4229);
        assert_eq!(model_to_product_id("390"), 0x4237);
        assert_eq!(model_to_product_id("999"), 0x6666);
    }

    #[test]
    fn test_platform_to_partition() {
        assert_eq!(
            platform_to_partition(&Platform::MT8113TNTX),
            "/dev/mmcblk0p12"
        );
        assert_eq!(
            platform_to_partition(&Platform::MX6SLLNTX),
            "/dev/mmcblk0p3"
        );
        assert_eq!(
            platform_to_partition(&Platform::MX6SULNTX),
            "/dev/mmcblk0p3"
        );
        assert_eq!(
            platform_to_partition(&Platform::Other("freescale".to_string())),
            "/dev/mmcblk0p3"
        );
    }

    #[test]
    fn test_parse_libra_colour() {
        let content = "SERIALPLACEHOLDER,4.9.77,4.45.23640,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390";
        let metadata = DeviceMetadata::parse(content, &Platform::MT8113TNTX).expect("parse failed");

        assert_eq!(metadata.serial_number, "SERIALPLACEHOLDER");
        assert_eq!(metadata.firmware_version, "4.45.23640");
        assert_eq!(metadata.product_id, 0x4237);
        assert_eq!(metadata.partition, "/dev/mmcblk0p12");
        assert_eq!(metadata.manufacturer, "Kobo");
        assert_eq!(metadata.product, "eReader-4.45.23640");
        assert_eq!(metadata.vendor_id, 0x2237);
    }

    #[test]
    fn test_parse_insufficient_fields() {
        let result = DeviceMetadata::parse("field1,field2", &Platform::MT8113TNTX);
        assert!(result.is_err());
    }
}
