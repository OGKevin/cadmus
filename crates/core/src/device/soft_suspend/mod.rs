//! Soft-suspend backend: a live Linux session or an inert no-op.
//!
//! [`SoftSuspend`] is a closed enum: [`SoftSuspend::Linux`] after a successful
//! sysfs probe on Linux, or [`SoftSuspend::NoOp`] on emulator, tests, and hosts
//! without complete writable autosleep / wake-lock nodes. Operations live on
//! [`SoftSuspendBackend`]; the enum implements that trait by matching on its
//! variants so `acquire` stays inlineable. Devices expose the backend via
//! [`crate::device::DeviceHardware::soft_suspend`].
//!
//! Linux sysfs session types live in
//! [`crate::device::linux::soft_suspend`].

pub(crate) mod backend;
pub mod lease;
pub mod mode;
pub mod noop;

pub use backend::SoftSuspendBackend;

use self::lease::SoftSuspendLease;
use self::mode::AutosleepMode;
use self::noop::NoOpSoftSuspend;
use crate::lease::LeaseName;
use std::sync::Arc;
use std::time::Duration;

#[cfg(any(target_os = "linux", docsrs))]
use crate::device::leds::DeviceLeds;
#[cfg(any(target_os = "linux", docsrs))]
use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
#[cfg(any(target_os = "linux", docsrs))]
use crate::device::linux::soft_suspend::session::SoftSuspendSession;

macro_rules! call_backend {
    ($self:expr, $method:ident $(, $arg:expr)* $(,)?) => {
        match $self {
            #[cfg(any(target_os = "linux", docsrs))]
            Self::Linux(backend) => backend.$method($($arg),*),
            Self::NoOp(backend) => backend.$method($($arg),*),
        }
    };
}

/// Soft-suspend backend: a live Linux session or an inert no-op.
///
/// Implements [`SoftSuspendBackend`].
pub enum SoftSuspend {
    /// Kernel autosleep / wake-lock session after a successful sysfs probe.
    #[cfg(any(target_os = "linux", docsrs))]
    Linux(SoftSuspendSession),
    /// Emulator, tests, or hosts without complete writable sysfs.
    NoOp(NoOpSoftSuspend),
}

impl SoftSuspend {
    /// Inert backend that never touches `/sys/power`.
    pub fn noop() -> Arc<Self> {
        Arc::new(Self::NoOp(NoOpSoftSuspend))
    }

    /// Probes system sysfs and returns [`Self::Linux`] or [`Self::NoOp`].
    #[cfg(any(target_os = "linux", docsrs))]
    pub fn from_system(leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        Self::from_paths(SoftSuspendPaths::system(), leds)
    }

    /// Probes `paths` and returns [`Self::Linux`] or [`Self::NoOp`].
    #[cfg(any(target_os = "linux", docsrs))]
    pub(crate) fn from_paths(
        paths: SoftSuspendPaths,
        leds: Option<Arc<dyn DeviceLeds>>,
    ) -> Arc<Self> {
        match paths.probe() {
            Ok(ok) => {
                tracing::debug!(
                    autosleep = %ok.autosleep().display(),
                    "soft-suspend supported"
                );
                Arc::new(Self::Linux(SoftSuspendSession::open(ok, leds)))
            }
            Err(paths) => {
                tracing::info!(
                    autosleep = %paths.autosleep.display(),
                    wake_lock = %paths.wake_lock.display(),
                    "soft-suspend unsupported; using no-op"
                );
                Self::noop()
            }
        }
    }

    /// Always constructs [`Self::Linux`] with injectable paths (tests).
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn with_paths(
        paths: SoftSuspendPaths,
        leds: Option<Arc<dyn DeviceLeds>>,
    ) -> Arc<Self> {
        Arc::new(Self::Linux(SoftSuspendSession::with_paths(paths, leds)))
    }
}

impl SoftSuspendBackend for SoftSuspend {
    fn is_supported(&self) -> bool {
        call_backend!(self, is_supported)
    }

    fn acquire(&self, name: impl Into<LeaseName>) -> SoftSuspendLease {
        call_backend!(self, acquire, name)
    }

    fn len(&self) -> usize {
        call_backend!(self, len)
    }

    fn holders(&self) -> Vec<LeaseName> {
        call_backend!(self, holders)
    }

    fn mode(&self) -> AutosleepMode {
        call_backend!(self, mode)
    }

    fn indicate_autosleep_led(&self) -> bool {
        call_backend!(self, indicate_autosleep_led)
    }

    fn autosleep_grace(&self) -> Duration {
        call_backend!(self, autosleep_grace)
    }

    fn available_modes(&self) -> Vec<AutosleepMode> {
        call_backend!(self, available_modes)
    }

    fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode {
        call_backend!(self, sanitize_mode, mode)
    }

    fn set_mode(&self, mode: AutosleepMode) {
        call_backend!(self, set_mode, mode)
    }

    fn set_indicate_autosleep_led(&self, enabled: bool) {
        call_backend!(self, set_indicate_autosleep_led, enabled)
    }

    fn set_autosleep_grace(&self, grace: Duration) {
        call_backend!(self, set_autosleep_grace, grace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn noop_is_unsupported() {
        let session = SoftSuspend::noop();
        let lease = session.acquire("main-loop");
        drop(lease);
        assert_eq!(session.with("main-loop", || 7), 7);
        session.apply_settings(AutosleepMode::Mem, true, Duration::from_secs(5));
        assert!(!session.is_supported());
        assert_eq!(session.mode(), AutosleepMode::Off);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn from_paths_complete_writable_is_linux() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        let session = SoftSuspend::from_paths(paths.clone(), None);
        assert!(session.is_supported());
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            "",
            "probe must not write cadmus"
        );
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "off",
            "probe must not write autosleep"
        );
    }

    #[test]
    fn from_paths_missing_autosleep_is_noop() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        fs::remove_file(&paths.autosleep).expect("remove autosleep");
        let session = SoftSuspend::from_paths(paths.clone(), None);
        assert!(!session.is_supported());
        let _lease = session.acquire("library-import");
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            ""
        );
    }

    #[test]
    fn from_paths_unwritable_wake_lock_is_noop() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        fs::remove_file(&paths.wake_lock).expect("remove");
        fs::create_dir(&paths.wake_lock).expect("create directory");
        let session = SoftSuspend::from_paths(paths.clone(), None);
        assert!(!session.is_supported());
        let _lease = session.acquire("input");
    }

    #[test]
    fn noop_acquire_does_not_write_sysfs() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        let session = SoftSuspend::noop();
        let lease = session.acquire("main-loop");
        drop(lease);
        session.apply_settings(AutosleepMode::Mem, true, Duration::from_secs(5));
        assert!(!session.is_supported());
        assert_eq!(session.mode(), AutosleepMode::Off);
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            ""
        );
        assert_eq!(
            fs::read_to_string(&paths.wake_unlock).expect("read").trim(),
            ""
        );
    }
}
