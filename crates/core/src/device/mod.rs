//! Device detection and management.
//!
//! Device traits, [`InputSource`], and [`EventOutcome`] live in this module.
//! The [`Device`] trait is split into focused sub-traits. A type that
//! implements all sub-traits automatically implements [`Device`].
//! Feature flags ensure exactly one implementation is compiled per binary,
//! selected by `kobo` or `emulator`.

mod error;
mod forward;
mod metadata;
pub mod migration;
mod model;
pub mod power;
pub mod rtc;
mod types;

#[cfg(unix)]
mod linux;
#[cfg(unix)]
pub use linux::LinuxRtc;
pub mod usb;
pub mod wifi;

#[cfg(any(feature = "emulator", docsrs))]
mod emulator;

#[cfg(any(
    test,
    docsrs,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
pub(crate) mod test_device;

#[cfg(any(all(feature = "kobo", not(feature = "emulator")), docsrs,))]
pub(crate) mod kobo;

pub use model::Model;
pub use types::{FrontlightKind, Orientation};

#[cfg(any(feature = "emulator", docsrs))]
pub use emulator::{EmulatorDevice, code_from_key, device_event};

#[cfg(any(
    test,
    docsrs,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
use crate::device::test_device::TestDevice;

#[cfg(not(docsrs))]
#[cfg(any(
    test,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
pub type AppDevice = TestDevice;

#[cfg(not(docsrs))]
#[cfg(all(not(test), feature = "emulator", not(feature = "kobo")))]
pub type AppDevice = EmulatorDevice;

#[cfg(not(docsrs))]
#[cfg(all(not(test), feature = "kobo", not(feature = "emulator")))]
pub type AppDevice = kobo::Device;

#[cfg(docsrs)]
pub use test_device::TestDevice as AppDevice;

/// The active context type for the current build.
pub type AppContext = crate::context::Context<AppDevice>;

use crate::device::metadata::DeviceMetadata;
use crate::input::TouchProto;
use crate::input::{DeviceEvent, InputEvent};
use crate::settings::ButtonScheme;
use crate::settings::versioned::SettingsManager;
use crate::task::TaskManager;
use crate::time_manager::TimeManager;
use crate::view::{Bus, Event, Hub, RenderQueue, UpdateData, View};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

pub struct HistoryItem {
    pub view: Box<dyn View>,
    pub rotation: i8,
    pub monochrome: bool,
    pub dithered: bool,
}

pub struct DeviceTask {
    pub id: DeviceTaskId,
    pub _chan: Receiver<()>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DeviceTaskId {
    CheckBattery,
    PrepareSuspend,
    Exit,
    Suspend,
}

pub struct DeviceRuntime<'a> {
    pub view: &'a mut Box<dyn View>,
    pub history: &'a mut Vec<HistoryItem>,
    pub tasks: &'a mut Vec<DeviceTask>,
    pub updating: &'a mut Vec<UpdateData>,
    pub inactive_since: &'a mut Instant,
    pub settings_manager: Option<&'a SettingsManager>,
    pub startup_cwd: Option<&'a Option<PathBuf>>,
    pub background_tasks: Option<&'a mut TaskManager>,
}

/// Outcome of [`DeviceLifecycle::handle_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    /// The lifecycle consumed the event; the main loop should skip view dispatch.
    Handled,
    /// The lifecycle did not handle the event; the main loop should pass it to the view.
    Unhandled,
    /// The lifecycle applied platform state but the main loop should still dispatch
    /// the event to views (for example after toggling frontlight hardware).
    Continue,
    /// Device lifecycle could not proceed; the main loop should log and skip view dispatch.
    Error,
    /// The lifecycle requests application termination with the given [`ExitStatus`].
    Exit(ExitStatus),
}

/// How the application should terminate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Quit,
    Restart,
    Reboot,
    PowerOff,
}

/// Trait for device input sources.
///
/// Each platform implements this to provide event channels to the main loop.
pub trait InputSource: Send {
    /// Starts the input pipeline and returns the event hub and receiver.
    ///
    /// The hub allows other parts of the system to send events into the loop,
    /// and the receiver yields all events for the main loop.
    fn start(
        &mut self,
        display: crate::framebuffer::Display,
        button_scheme: ButtonScheme,
    ) -> (Hub, Receiver<Event>);

    /// Injects a raw [`InputEvent`] into the input pipeline.
    ///
    /// Callers use this after [`Self::start`] to push synthetic kernel-style
    /// events without going through hardware.
    ///
    /// Default: no-op.
    fn send_raw(&self, _event: InputEvent) {}

    /// Injects a [`DeviceEvent`] into the device-event channel.
    ///
    /// Callers use this after [`Self::start`] to enqueue high-level device
    /// events that the gesture pipeline turns into [`Event`] values.
    ///
    /// Default: no-op.
    fn send_device(&self, _event: DeviceEvent) {}

    /// Updates the rotation used when mapping touch coordinates.
    ///
    /// Input sources that transform touch positions must stay in sync with the
    /// framebuffer rotation applied elsewhere in the application.
    ///
    /// Default: no-op.
    fn set_rotation(&self, _n: i8) {}
}

/// Device identity: model, touch protocol, screen dimensions, and mark.
///
/// Callers use these values to select assets, interpret touch coordinates, and
/// distinguish hardware variants. Values are fixed for the lifetime of the
/// device instance.
pub trait DeviceIdentity: Send {
    /// Returns the device model enum for this hardware.
    fn model(&self) -> crate::device::Model;

    /// Returns the touch input protocol the kernel reports for this device.
    fn proto(&self) -> TouchProto;

    /// Returns the native screen dimensions as `(width, height)` in pixels.
    ///
    /// This is the unrotated framebuffer size; callers that need the effective
    /// layout size must account for rotation separately.
    // TODO(OGKevin): this should be a newtype
    fn dims(&self) -> (u32, u32);

    /// Returns the screen DPI used for scaling icons and typography.
    // TODO(OGKevin): this should be a newtype
    fn dpi(&self) -> u16;

    /// Returns the device mark byte used by the input stack to select
    /// coordinate transforms and button maps.
    // TODO(OGKevin): this should be a newtype
    fn mark(&self) -> u8;
}

/// Device capabilities: frontlight kind, sensors, buttons, and color.
///
/// Defaults assume a minimal device (no sensors, single gray channel). Platform
/// implementations override only the capabilities they provide.
pub trait DeviceCapabilities: Send {
    /// Returns the kind of frontlight hardware on this device.
    fn frontlight_kind(&self) -> FrontlightKind;

    /// Returns whether the device supports adjustable color temperature.
    ///
    /// Default: derived from [`Self::frontlight_kind`] — anything other than
    /// [`FrontlightKind::Standard`] counts as natural light.
    fn has_natural_light(&self) -> bool {
        self.frontlight_kind() != FrontlightKind::Standard
    }

    /// Returns whether an ambient light sensor is present.
    ///
    /// Default: `false`.
    fn has_lightsensor(&self) -> bool {
        false
    }

    /// Returns whether a gyroscope is available for auto-rotation.
    ///
    /// Default: `false`.
    fn has_gyroscope(&self) -> bool {
        false
    }

    /// Returns whether dedicated page-turn buttons exist.
    ///
    /// Default: `false`.
    fn has_page_turn_buttons(&self) -> bool {
        false
    }

    /// Returns whether a magnetic power cover is supported.
    ///
    /// Default: `false`.
    fn has_power_cover(&self) -> bool {
        false
    }

    /// Returns whether the device can store user data on removable media.
    ///
    /// When `true`, [`DevicePaths::data_dir`] may resolve to external storage.
    /// Default: `false`.
    fn has_removable_storage(&self) -> bool {
        false
    }

    /// Returns the number of framebuffer color samples per pixel.
    ///
    /// Most devices use `1` (grayscale). Color panels may report `3`.
    /// Default: `1`.
    // TODO(OGKevin): this should be a newtype
    fn color_samples(&self) -> usize {
        1
    }
}

/// Rotation algebra: canonicalization, mirroring, and orientation.
///
/// Each device reports display rotation as a quarter-turn index `0..=3`. These
/// methods define how that native value maps to a shared canonical space and how
/// touch, button, and layout code should transform coordinates at a given
/// rotation. Implementations supply device-specific constants; default methods
/// derive the rest from [`Self::mirroring_scheme`] and [`Self::startup_rotation`].
pub trait DeviceRotation: Send {
    /// Returns the rotation index the device reports at startup.
    ///
    /// Callers treat this as the hardware's "home" orientation before any user
    /// rotation is applied.
    // TODO(OGKevin): these should be newtypes
    fn startup_rotation(&self) -> i8;

    /// Returns the mirroring pattern as `(center, direction)`.
    ///
    /// `center` is a rotation index that acts as the pivot for axis mirroring;
    /// `direction` is `1` or `-1` and controls how rotations step around that
    /// pivot when converting to and from canonical space.
    // TODO(OGKevin): these should be newtypes
    fn mirroring_scheme(&self) -> (i8, i8);

    /// Returns the parity used by [`Self::should_swap_axes`].
    ///
    /// When `rotation % 2 == swapping_scheme()`, width and height are swapped.
    /// Default: `1`.
    // TODO(OGKevin): these should be newtypes
    fn swapping_scheme(&self) -> i8 {
        1
    }

    /// Maps a native rotation index to the shared canonical space.
    ///
    /// Canonical values let UI and settings refer to orientation without
    /// device-specific offsets. [`Self::to_native`] inverts this map.
    /// Default: derived from [`Self::startup_rotation`] and
    /// [`Self::mirroring_scheme`].
    // TODO(OGKevin): these should be newtypes
    fn to_canonical(&self, n: i8) -> i8 {
        let (_, dir) = self.mirroring_scheme();
        (4 + dir * (n - self.startup_rotation())) % 4
    }

    /// Maps a canonical rotation index back to the device's native space.
    ///
    /// Default: inverse of [`Self::to_canonical`].
    // TODO(OGKevin): these should be newtypes
    fn to_native(&self, n: i8) -> i8 {
        let (_, dir) = self.mirroring_scheme();
        (self.startup_rotation() + (4 + dir * n) % 4) % 4
    }

    /// Maps the kernel framebuffer rotation to the initial display rotation at boot.
    ///
    /// Some panels report rotation differently from the logical display index.
    /// [`Context::new`](crate::context::Context::new) applies this once when reading
    /// the boot framebuffer state. Runtime rotation writes the same index to both
    /// framebuffer and display via [`Context::set_rotation`](crate::context::Context::set_rotation).
    ///
    /// Implementations must be **self-inverse** on `{0, 1, 2, 3}` so that
    /// `f(f(n)) == n`. Default: identity (`n`).
    // TODO(OGKevin): these should be newtypes
    fn transformed_rotation(&self, n: i8) -> i8 {
        n
    }

    /// Returns the transformed rotation read from the framebuffer at device init,
    /// before any startup adjustment is applied.
    ///
    /// On quit, non-gyro devices restore this orientation so the panel returns to
    /// the state Cadmus observed at launch. Default: [`Self::transformed_rotation`]
    /// of [`Self::startup_rotation`].
    fn boot_transformed_rotation(&self) -> i8 {
        self.transformed_rotation(self.startup_rotation())
    }

    /// Applies a gyroscope-specific rotation remap.
    ///
    /// Auto-rotation from the accelerometer may need a different transform than
    /// the value used for rendering. Default: identity (`n`).
    // TODO(OGKevin): these should be newtypes
    fn transformed_gyroscope_rotation(&self, n: i8) -> i8 {
        n
    }

    /// Returns whether width and height should be swapped at `rotation`.
    ///
    /// Default: `rotation % 2 == self.swapping_scheme()`.
    fn should_swap_axes(&self, rotation: i8) -> bool {
        rotation % 2 == self.swapping_scheme()
    }

    /// Returns whether touch X and/or Y should be mirrored at `rotation`.
    ///
    /// The tuple is `(mirror_x, mirror_y)`. Default: derived from
    /// [`Self::mirroring_scheme`].
    fn should_mirror_axes(&self, rotation: i8) -> (bool, bool) {
        let (mxy, dir) = self.mirroring_scheme();
        let mx = (4 + (mxy + dir)) % 4;
        let my = (4 + (mxy - dir)) % 4;
        let mirror_x = mxy == rotation || mx == rotation;
        let mirror_y = mxy == rotation || my == rotation;
        (mirror_x, mirror_y)
    }

    /// Returns whether page-turn buttons should use inverted mapping at `rotation`.
    ///
    /// Default: `true` at the two rotations that are mirror-symmetric around
    /// the startup orientation.
    fn should_invert_buttons(&self, rotation: i8) -> bool {
        let sr = self.startup_rotation();
        let (_, dir) = self.mirroring_scheme();
        rotation == (4 + sr - dir) % 4 || rotation == (4 + sr - 2 * dir) % 4
    }

    /// Returns the layout orientation at `rotation`.
    ///
    /// Default: [`Orientation::Portrait`] when [`Self::should_swap_axes`] is
    /// `true`, otherwise [`Orientation::Landscape`].
    fn orientation(&self, rotation: i8) -> Orientation {
        if self.should_swap_axes(rotation) {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }

    /// Bundles rotation parameters for the input subsystem.
    ///
    /// Default: collects [`Self::mirroring_scheme`], [`Self::swapping_scheme`],
    /// and [`Self::startup_rotation`], and sets `swap_dims_on_rotation` when
    /// `mark == 8`.
    fn device_input_info(
        &self,
        proto: crate::input::TouchProto,
        mark: u8,
    ) -> crate::input::DeviceInputInfo {
        crate::input::DeviceInputInfo {
            proto,
            mark,
            mirroring_scheme: self.mirroring_scheme(),
            swapping_scheme: self.swapping_scheme(),
            startup_rotation: self.startup_rotation(),
            gyro_rotation_transform: crate::input::GyroRotationTransform::default(),
            swap_dims_on_rotation: mark == 8,
        }
    }
}

/// Filesystem paths: install directory, data directory, and tmp management.
///
/// Install paths hold static bundled assets; data paths hold mutable runtime
/// state (database, settings, logs). Callers should use [`Self::install_path`]
/// and [`Self::data_path`] rather than joining manually so platform-specific
/// roots stay consistent.
pub trait DevicePaths: Send {
    /// Returns the relative subdirectory name under the install root.
    fn install_subdir(&self) -> &'static str;

    /// Returns the absolute install directory for static assets.
    ///
    /// Must be stable regardless of process working directory.
    fn install_dir(&self) -> PathBuf;

    /// Returns the relative subdirectory name under the data root.
    fn data_subdir(&self) -> &'static str;

    /// Returns the absolute directory for mutable runtime data.
    ///
    /// May equal [`Self::install_dir`] when external storage is unavailable.
    fn data_dir(&self) -> PathBuf;

    /// Joins `relative` under [`Self::install_dir`].
    fn install_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.install_dir().join(relative)
    }

    /// Joins `relative` under [`Self::data_dir`].
    fn data_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.data_dir().join(relative)
    }

    /// Resolves the SQLite database path, preferring the data directory.
    ///
    /// Lookup order: existing file in data dir, then legacy install-dir copy
    /// (with a warning), then the data-dir path for a new install.
    fn resolve_db_path(&self) -> PathBuf {
        let data_path = self.data_path(crate::db::DB_FILENAME);
        if data_path.exists() {
            return data_path;
        }
        let install_path = self.install_path(crate::db::DB_FILENAME);
        if install_path.exists() {
            tracing::warn!(
                path = %install_path.display(),
                "sqlite db found in install dir, not data dir; \
                 copy it to data dir"
            );
            return install_path;
        }
        data_path
    }

    /// Returns the device-managed temporary directory under the data root.
    ///
    /// Default: `data_dir/tmp`.
    fn tmp_dir(&self) -> PathBuf {
        self.data_path(Path::new("tmp"))
    }

    /// Removes stale tmp contents and recreates the directory.
    ///
    /// Call once at startup before any writer uses [`Self::tmp_dir`]. Missing
    /// directories are not an error. Default: best-effort remove and recreate
    /// with warnings on failure.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn clean_tmp_dir(&self) {
        let dir = self.tmp_dir();
        if let Err(e) = std::fs::remove_dir_all(&dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = ?dir, error = %e, "Failed to clean tmp dir");
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = ?dir, error = %e, "Failed to create tmp dir");
        }
    }
}

/// Hardware subsystem accessors with associated types.
///
/// Associated types resolve to platform-specific implementations at compile time.
/// Manager accessors return shared handles so concurrent tasks can use Wi-Fi,
/// USB, and power subsystems without borrowing the whole device.
pub trait DeviceHardware: Send {
    type Framebuffer: crate::framebuffer::Framebuffer + Send;
    type Battery: crate::battery::Battery + Send;
    type Frontlight: crate::frontlight::Frontlight + Send;
    type LightSensor: crate::lightsensor::LightSensor + Send;
    type WifiManager: crate::device::wifi::WifiManager + Send + Sync;
    type UsbManager: crate::device::usb::UsbManager + Send + Sync;
    type PowerManager: crate::device::power::PowerManager + Send + Sync;
    type Rtc: crate::device::rtc::Rtc + Send + Sync + 'static;

    /// Returns a shared reference to the display framebuffer.
    fn framebuffer(&self) -> &Self::Framebuffer;
    /// Returns a mutable reference to the display framebuffer.
    fn framebuffer_mut(&mut self) -> &mut Self::Framebuffer;
    /// Returns a shared reference to the battery interface.
    fn battery(&self) -> &Self::Battery;
    /// Returns a mutable reference to the battery interface.
    fn battery_mut(&mut self) -> &mut Self::Battery;
    /// Returns a shared reference to the frontlight controller.
    fn frontlight(&self) -> &Self::Frontlight;
    /// Returns a mutable reference to the frontlight controller.
    fn frontlight_mut(&mut self) -> &mut Self::Frontlight;
    /// Returns a shared reference to the ambient light sensor.
    fn lightsensor(&self) -> &Self::LightSensor;
    /// Returns a mutable reference to the ambient light sensor.
    fn lightsensor_mut(&mut self) -> &mut Self::LightSensor;

    /// Returns a shared Wi-Fi manager handle.
    ///
    /// # Errors
    ///
    /// Returns [`crate::device::wifi::WifiError`] when Wi-Fi is unavailable.
    fn wifi_manager(
        &self,
    ) -> Result<std::sync::Arc<Self::WifiManager>, crate::device::wifi::WifiError>;

    /// Returns a shared USB manager handle.
    ///
    /// # Errors
    ///
    /// Returns [`crate::device::usb::UsbError`] when USB management is unavailable.
    fn usb_manager(&self)
    -> Result<std::sync::Arc<Self::UsbManager>, crate::device::usb::UsbError>;

    /// Returns a shared power manager handle.
    ///
    /// # Errors
    ///
    /// Returns [`crate::device::power::PowerError`] when power management is unavailable.
    fn power_manager(
        &self,
    ) -> Result<std::sync::Arc<Self::PowerManager>, crate::device::power::PowerError>;

    /// Returns a shared RTC handle.
    ///
    /// # Errors
    ///
    /// Returns an error when the RTC subsystem cannot be opened.
    fn rtc(&self) -> Result<std::sync::Arc<Self::Rtc>, anyhow::Error>;

    /// Returns the time manager that coordinates NTP sync and RTC writes.
    ///
    /// # Errors
    ///
    /// Returns an error when the time manager is not initialized.
    fn time_manager(&self) -> Result<&TimeManager<Self::Rtc>, anyhow::Error>;

    /// Applies `tz` as the system timezone.
    ///
    /// Default: no-op success. Platform implementations update OS state.
    fn set_system_timezone(&self, tz: chrono_tz::Tz) -> Result<(), anyhow::Error> {
        let _ = tz;
        Ok(())
    }

    /// Refreshes the framebuffer from kernel state after resume or rotation.
    ///
    /// Default: no-op. Platform implementations remap the mmap or reload the
    /// display buffer when the kernel reconfigures the panel.
    fn refresh_framebuffer_from_kernel(&mut self) {
        let _ = self;
    }

    /// Returns static device metadata (serial, firmware version, etc.).
    ///
    /// Default: [`crate::device::error::DeviceError::Metadata`] with
    /// `"no metadata available"`.
    fn metadata(&self) -> Result<&DeviceMetadata, crate::device::error::DeviceError> {
        Err(crate::device::error::DeviceError::Metadata(
            "no metadata available".to_string(),
        ))
    }
}

/// Input source ownership and access.
pub trait DeviceInput: Send {
    type Input: InputSource;

    /// Returns a shared reference to the input source.
    fn input(&self) -> &Self::Input;
    /// Returns a mutable reference to the input source.
    fn input_mut(&mut self) -> &mut Self::Input;
}

/// Event handling and application lifecycle.
pub trait DeviceLifecycle:
    DeviceIdentity
    + DeviceCapabilities
    + DeviceRotation
    + DevicePaths
    + DeviceHardware
    + DeviceInput
    + Send
    + Sized
{
    /// Dispatches a platform-specific device event from the main event loop.
    ///
    /// Called after background tasks see the event and before the root view
    /// when the outcome is [`EventOutcome::Unhandled`] or
    /// [`EventOutcome::Continue`]. Return [`EventOutcome::Handled`] or
    /// [`EventOutcome::Error`] to skip view dispatch, [`EventOutcome::Continue`]
    /// when platform state was updated but views should still see the event, or
    /// [`EventOutcome::Exit`] to terminate the loop with the given
    /// [`ExitStatus`].
    fn handle_event(
        event: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        runtime: &mut DeviceRuntime<'_>,
    ) -> EventOutcome;

    /// Runs once after the UI is initialized and before the main event loop.
    ///
    /// Platform implementations typically configure hardware (Wi-Fi, power,
    /// frontlight), schedule periodic device tasks, and emit startup events.
    ///
    /// Default: no-op success.
    fn on_startup(
        context: &mut AppContext,
        hub: &Hub,
        runtime: &mut DeviceRuntime<'_>,
    ) -> Result<(), anyhow::Error> {
        let _ = (context, hub, runtime);
        Ok(())
    }

    /// Runs once when the main event loop exits, before settings are persisted.
    ///
    /// `status` reflects how the application is terminating (quit, restart,
    /// reboot, or power off). Platform implementations tear down hardware and
    /// may write marker files consumed by the device init system.
    ///
    /// Default: no-op success.
    fn on_shutdown(
        context: &mut AppContext,
        status: ExitStatus,
        runtime: &mut DeviceRuntime<'_>,
    ) -> Result<(), anyhow::Error> {
        let _ = (context, status, runtime);
        Ok(())
    }
}

/// Super-trait combining all device sub-traits.
///
/// A type that implements all sub-traits automatically implements `Device`.
/// Feature flags ensure exactly one implementation is compiled per binary,
/// so associated types resolve to concrete types with no vtable overhead.
pub trait Device:
    DeviceIdentity
    + DeviceCapabilities
    + DeviceRotation
    + DevicePaths
    + DeviceHardware
    + DeviceInput
    + DeviceLifecycle
    + Sized
{
}

impl<T> Device for T where
    T: DeviceIdentity
        + DeviceCapabilities
        + DeviceRotation
        + DevicePaths
        + DeviceHardware
        + DeviceInput
        + DeviceLifecycle
        + Sized
{
}
