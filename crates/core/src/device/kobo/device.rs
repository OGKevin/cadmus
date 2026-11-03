use super::input::InputSource;
use super::model::{DeviceTreeCompatible, Model, parse_device_tree_compatible_bytes};
use crate::device::linux::LinuxRtc;
use crate::device::metadata::DeviceMetadata;
use crate::device::{DeviceIdentity, DevicePaths};
use std::env;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn discover_peer_installs(root: &Path, current: &Path) -> Vec<crate::device::PeerInstall> {
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

pub struct Device {
    model: Model,
    metadata: DeviceMetadata,
    framebuffer: Box<dyn crate::framebuffer::Framebuffer + Send>,
    battery: Arc<dyn crate::device::battery::Battery>,
    frontlight: Box<dyn crate::frontlight::Frontlight>,
    lightsensor: Box<dyn crate::lightsensor::LightSensor>,
    wifi_manager: std::sync::Arc<crate::device::kobo::wifi::KoboWifiManager>,
    usb_manager: std::sync::Arc<crate::device::kobo::usb::KoboUsbManager>,
    power_manager: std::sync::Arc<crate::device::kobo::power::KoboPowerManager>,
    leds: std::sync::Arc<crate::device::kobo::leds::KoboLeds>,
    rtc: std::sync::Arc<LinuxRtc>,
    time_manager: crate::time_manager::TimeManager<LinuxRtc>,
    input: InputSource,
    boot_transformed_rotation: i8,
    data_dir: PathBuf,
}

/// Chooses the data root from the install directory and the storage facts.
///
/// Pure so the policy is testable without reading `/proc/mounts`. Runtime data
/// lives on the external card only when the device has removable storage and
/// that card is mounted; otherwise it stays in the install directory.
fn data_root(
    install_dir: PathBuf,
    data_subdir: &str,
    removable: bool,
    external_mounted: bool,
) -> PathBuf {
    if removable && external_mounted {
        Path::new(crate::settings::EXTERNAL_CARD_ROOT).join(data_subdir)
    } else {
        install_dir
    }
}

/// Reports whether the external card slot holds a mounted card.
///
/// Runs before logging is configured, so the fallback warnings go to stderr,
/// which `cadmus.sh` merges into `info.log`.
async fn external_card_mounted(install_dir: &Path) -> bool {
    let root = Path::new(crate::settings::EXTERNAL_CARD_ROOT);
    match crate::device::linux::mounts::is_mount_point(root).await {
        Ok(mounted) => {
            if !mounted {
                eprintln!(
                    "Warning: removable storage is present but {} is not a mount point; using {}",
                    root.display(),
                    install_dir.display()
                );
            }
            mounted
        }
        Err(error) => {
            eprintln!(
                "Warning: failed to check whether {} is a mount point: {error}; using {}",
                root.display(),
                install_dir.display()
            );
            false
        }
    }
}

/// Resolves the data root once at startup.
///
/// The result is stored on the device so every later lookup sees the same
/// directory, even if the card is mounted or removed while the app runs.
async fn resolve_data_dir(device: &Device) -> PathBuf {
    let install_dir = device.install_dir();
    let removable = !cfg!(test) && crate::device::DeviceCapabilities::has_removable_storage(device);
    let external_mounted = removable && external_card_mounted(&install_dir).await;
    data_root(
        install_dir,
        device.data_subdir(),
        removable,
        external_mounted,
    )
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
    pub(super) async fn new(
        model: Model,
        metadata: DeviceMetadata,
        framebuffer: Box<dyn crate::framebuffer::Framebuffer + Send>,
        battery: Arc<dyn crate::device::battery::Battery>,
        frontlight: Box<dyn crate::frontlight::Frontlight>,
        lightsensor: Box<dyn crate::lightsensor::LightSensor>,
        wifi_manager: std::sync::Arc<crate::device::kobo::wifi::KoboWifiManager>,
        usb_manager: std::sync::Arc<crate::device::kobo::usb::KoboUsbManager>,
        power_manager: std::sync::Arc<crate::device::kobo::power::KoboPowerManager>,
        leds: std::sync::Arc<crate::device::kobo::leds::KoboLeds>,
        rtc: std::sync::Arc<LinuxRtc>,
        time_manager: crate::time_manager::TimeManager<LinuxRtc>,
        boot_transformed_rotation: i8,
        input: InputSource,
    ) -> Self {
        let mut device = Self {
            model,
            metadata,
            framebuffer,
            battery,
            frontlight,
            lightsensor,
            wifi_manager,
            usb_manager,
            power_manager,
            leds,
            rtc,
            time_manager,
            input,
            boot_transformed_rotation,
            data_dir: PathBuf::new(),
        };
        device.data_dir = resolve_data_dir(&device).await;
        device
    }

    /// Creates a device from product and model number strings.
    async fn from_product(product: &str, model_number: &str) -> anyhow::Result<Self> {
        Model::new(product, model_number).device().await
    }

    fn model_number_from_environment() -> String {
        match env::var("MODEL_NUMBER") {
            Ok(number) => number,
            Err(_) => {
                eprintln!("Warning: MODEL_NUMBER is not set; device-tree model matching may fail");
                String::new()
            }
        }
    }

    /// Builds a device from the device tree, falling back to environment variables.
    ///
    /// Runs before [`crate::logging::init_logging`] because the log path depends on the
    /// device, so `tracing` has no subscriber yet. Diagnostics therefore go to stderr,
    /// which `cadmus.sh` redirects into `info.log`.
    pub async fn from_environment() -> anyhow::Result<Self> {
        let model_number = match DeviceMetadata::read().await {
            Ok(metadata) => metadata.model_number,
            Err(error) => {
                eprintln!(
                    "Warning: failed to read Kobo device metadata from .kobo/version: {error:#}; falling back to MODEL_NUMBER"
                );
                Self::model_number_from_environment()
            }
        };
        if let Some(model) = Self::detect_model(&model_number).await {
            return model.device().await;
        }
        let product = env::var("PRODUCT").unwrap_or_default();
        Self::from_product(&product, &model_number).await
    }

    async fn detect_model(model_number: &str) -> Option<Model> {
        let compatible = Self::read_device_tree_compatible().await;
        let model = Model::from_device_tree(&compatible, model_number);

        if model.is_none() {
            eprintln!(
                "Warning: unable to identify Kobo model from device tree; compatible={:?}, \
                 model_number={model_number:?}, PRODUCT={:?}; using environment variables",
                compatible
                    .iter()
                    .map(|entry| entry.as_str())
                    .collect::<Vec<_>>(),
                env::var("PRODUCT").ok(),
            );
        }

        model
    }

    async fn read_device_tree_compatible() -> Vec<DeviceTreeCompatible> {
        const PATHS: &[&str] = &[
            "/proc/device-tree/compatible",
            "/sys/firmware/devicetree/base/compatible",
        ];
        for path in PATHS {
            if let Ok(bytes) = tokio::fs::read(path).await {
                return parse_device_tree_compatible_bytes(&bytes);
            }
        }
        Vec::new()
    }
}

/// Kobo install and data directory layout.
///
/// Install root: `/mnt/onboard/.adds/cadmus` (or `cadmus-tst` for test builds).
/// Data root: `/mnt/sd/.cadmus` when removable storage is present and the card
/// slot holds an actual mount; otherwise the install root. The data root is
/// resolved once at startup, so an empty card slot keeps runtime data on the
/// internal card instead of creating `/mnt/sd/.cadmus` on the rootfs.
impl DevicePaths for Device {
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
        self.data_dir.clone()
    }

    fn peer_installs(&self) -> Vec<crate::device::PeerInstall> {
        let root = cfg_select! {
            test => { std::env::temp_dir().join("test-kobo-installation") }
            _ => { PathBuf::from(crate::settings::INTERNAL_CARD_ROOT) }
        };
        discover_peer_installs(&root, &self.install_dir())
    }
}

crate::impl_device_hardware!(
    Device,
    Framebuffer = Box<dyn crate::framebuffer::Framebuffer + Send>,
    Battery = Arc<dyn crate::device::battery::Battery>,
    Frontlight = Box<dyn crate::frontlight::Frontlight>,
    LightSensor = Box<dyn crate::lightsensor::LightSensor>,
    WifiManager = crate::device::kobo::wifi::KoboWifiManager,
    UsbManager = crate::device::kobo::usb::KoboUsbManager,
    PowerManager = crate::device::kobo::power::KoboPowerManager,
    Leds = crate::device::kobo::leds::KoboLeds,
    Rtc = LinuxRtc;
    override
        metadata_from metadata,
        set_system_timezone linux,
        refresh_framebuffer_from_kernel framebuffer,
        inhibitor from_system,
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

crate::forward_device_identity!(Device, model);
crate::forward_device_capabilities!(Device, model);
crate::forward_device_rotation!(Device, model, boot = boot_transformed_rotation);

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::DeviceRotation as _;

    mod paths {
        use super::*;

        #[tokio::test]
        async fn install_subdir() {
            let d = Model::Sage.device().await.unwrap();
            let subdir = d.install_subdir();
            assert!(
                subdir == ".adds/cadmus" || subdir == ".adds/cadmus-tst" || subdir.is_empty(),
                "install_subdir returned {subdir:?}"
            );
        }

        #[tokio::test]
        async fn data_subdir() {
            let d = Model::Sage.device().await.unwrap();
            let subdir = d.data_subdir();
            assert!(
                subdir == ".cadmus" || subdir == ".cadmus-tst",
                "data_subdir returned {subdir:?}"
            );
        }

        #[tokio::test]
        async fn install_dir_ends_with_install_subdir() {
            let d = Model::Sage.device().await.unwrap();
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

        #[tokio::test]
        async fn install_path_joins_install_dir() {
            let d = Model::Sage.device().await.unwrap();
            let relative = std::path::Path::new("tmp");
            let expected = d.install_dir().join(relative);
            assert_eq!(d.install_path(relative), expected);
        }

        #[tokio::test]
        async fn data_path_joins_data_dir() {
            let d = Model::Sage.device().await.unwrap();
            let relative = std::path::Path::new("cadmus.sqlite");
            let expected = d.data_dir().join(relative);
            assert_eq!(d.data_path(relative), expected);
        }

        #[tokio::test]
        async fn tmp_dir_is_data_path_tmp() {
            let d = Model::Sage.device().await.unwrap();
            let expected = d.data_path(std::path::Path::new("tmp"));
            assert_eq!(d.tmp_dir(), expected);
        }

        #[tokio::test]
        async fn data_dir_is_install_dir_under_test() {
            let d = Model::Sage.device().await.unwrap();
            assert_eq!(d.data_dir(), d.install_dir());
        }

        #[test]
        fn data_root_prefers_mounted_external_card() {
            let install = PathBuf::from("/mnt/onboard/.adds/cadmus");
            assert_eq!(
                data_root(install, ".cadmus", true, true),
                Path::new(crate::settings::EXTERNAL_CARD_ROOT).join(".cadmus")
            );
        }

        #[test]
        fn data_root_keeps_install_dir_without_mounted_card() {
            let install = PathBuf::from("/mnt/onboard/.adds/cadmus");
            assert_eq!(data_root(install.clone(), ".cadmus", true, false), install);
        }

        #[test]
        fn data_root_keeps_install_dir_without_removable_storage() {
            let install = PathBuf::from("/mnt/onboard/.adds/cadmus");
            assert_eq!(data_root(install.clone(), ".cadmus", false, true), install);
        }

        #[test]
        fn peer_installs_empty_without_peer_launcher() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join(".adds/cadmus");
            assert!(discover_peer_installs(root.path(), &current).is_empty());
        }

        #[test]
        fn peer_installs_finds_peer_cadmus_sh() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join(".adds/cadmus");
            let peer_dir = root.path().join(".adds/cadmus-tst");
            let launcher = peer_dir.join("cadmus.sh");
            std::fs::create_dir_all(&peer_dir).unwrap();
            std::fs::write(&launcher, "#!/bin/sh\n").unwrap();

            let peers = discover_peer_installs(root.path(), &current);
            assert_eq!(peers.len(), 1);
            assert_eq!(peers[0].launcher, launcher);
            assert_eq!(peers[0].kind, crate::version::BuildKind::Test);
        }
    }

    #[tokio::test]
    async fn boot_transformed_rotation_stored_at_init() {
        let device = Model::Glo.device().await.unwrap();
        assert!((0..4).contains(&device.boot_transformed_rotation()));
    }
}
