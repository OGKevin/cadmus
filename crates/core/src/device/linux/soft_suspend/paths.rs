//! Sysfs paths and discovery helpers for soft suspend.

use crate::device::soft_suspend::mode::AutosleepMode;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Sysfs paths used by soft suspend. Injectable for tests.
#[derive(Debug, Clone)]
pub struct SoftSuspendPaths {
    /// `/sys/power/state` — discover available sleep targets.
    pub state: PathBuf,
    /// `/sys/power/autosleep` — arm or disarm autosleep.
    pub autosleep: PathBuf,
    /// `/sys/power/wake_lock` — take a named wake lock.
    pub wake_lock: PathBuf,
    /// `/sys/power/wake_unlock` — release a named wake lock.
    pub wake_unlock: PathBuf,
}

impl SoftSuspendPaths {
    /// Default Kobo / Linux power sysfs layout.
    pub fn system() -> Self {
        Self {
            state: PathBuf::from("/sys/power/state"),
            autosleep: PathBuf::from("/sys/power/autosleep"),
            wake_lock: PathBuf::from("/sys/power/wake_lock"),
            wake_unlock: PathBuf::from("/sys/power/wake_unlock"),
        }
    }

    /// Returns whether the full sysfs set exists and is usable (writable locks, readable state).
    ///
    /// Opens for write without writing values.
    pub fn probe_supported(&self) -> bool {
        can_open_write(&self.autosleep)
            && can_open_write(&self.wake_lock)
            && can_open_write(&self.wake_unlock)
            && can_open_read(&self.state)
    }

    /// Probes sysfs and returns a capability token when the backend is usable.
    ///
    /// On failure, returns the original paths so callers can log them.
    pub(crate) fn probe(self) -> Result<SoftSuspendProbeOk, SoftSuspendPaths> {
        if self.probe_supported() {
            Ok(SoftSuspendProbeOk { paths: self })
        } else {
            Err(self)
        }
    }

    /// Temporary writable sysfs tree for tests (`freeze mem` in `state`).
    #[cfg(test)]
    pub fn test_fixture() -> (tempfile::TempDir, Self) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Self {
            state: dir.path().join("state"),
            autosleep: dir.path().join("autosleep"),
            wake_lock: dir.path().join("wake_lock"),
            wake_unlock: dir.path().join("wake_unlock"),
        };
        fs::write(&paths.state, "freeze mem\n").expect("state");
        fs::write(&paths.autosleep, "off\n").expect("autosleep");
        fs::write(&paths.wake_lock, "").expect("wake_lock");
        fs::write(&paths.wake_unlock, "").expect("wake_unlock");
        (dir, paths)
    }
}

/// Proof that soft-suspend sysfs was probed successfully.
///
/// Outside tests, [`SoftSuspendSession::open`](super::session::SoftSuspendSession::open)
/// is the only way to build a live session, and it requires this token from
/// [`SoftSuspendPaths::probe`]. Fields are private so the token cannot be forged
/// by struct literal; tests use [`SoftSuspendProbeOk::assume`].
#[derive(Debug)]
pub(crate) struct SoftSuspendProbeOk {
    paths: SoftSuspendPaths,
}

impl SoftSuspendProbeOk {
    /// Forges a probe token without checking sysfs (tests only).
    #[cfg(test)]
    pub(crate) fn assume(paths: SoftSuspendPaths) -> Self {
        Self { paths }
    }

    /// Path to the autosleep sysfs node.
    pub(crate) fn autosleep(&self) -> &Path {
        &self.paths.autosleep
    }

    pub(crate) fn into_paths(self) -> SoftSuspendPaths {
        self.paths
    }
}

fn can_open_write(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn can_open_read(path: &Path) -> bool {
    fs::File::open(path).is_ok()
}

/// Successful outcome of a soft-suspend sysfs write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SysfsWrite {
    /// Value was written to an existing sysfs node.
    Written,
    /// Path was absent — no-op on hosts without autosleep / wake locks.
    Missing,
}

/// Failure writing a soft-suspend sysfs node that exists.
#[derive(Debug, Error)]
#[error("failed to write soft-suspend sysfs {}: {source}", path.display())]
pub(super) struct SysfsWriteError {
    path: PathBuf,
    #[source]
    source: std::io::Error,
}

/// Reads `/sys/power/state` and returns supported soft-suspend modes (`Off` always included).
pub fn discover_available_modes(state_path: &Path) -> Vec<AutosleepMode> {
    let mut modes = vec![AutosleepMode::Off];
    let Ok(contents) = fs::read_to_string(state_path) else {
        return modes;
    };
    for token in contents.split_whitespace() {
        if let Some(mode) = AutosleepMode::from_state_token(token)
            && !modes.contains(&mode)
        {
            modes.push(mode);
        }
    }
    modes
}

/// Writes `value` to a soft-suspend sysfs node.
///
/// Does not log — callers choose how to handle [`SysfsWrite::Missing`] vs
/// [`SysfsWriteError`].
pub(super) fn write_sysfs(path: &Path, value: &str) -> Result<SysfsWrite, SysfsWriteError> {
    if !path.exists() {
        return Ok(SysfsWrite::Missing);
    }
    fs::write(path, value)
        .map(|()| SysfsWrite::Written)
        .map_err(|source| SysfsWriteError {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_parses_freeze_and_mem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state");
        fs::write(&path, "freeze mem\n").expect("write");

        let modes = discover_available_modes(&path);

        assert_eq!(
            modes,
            vec![
                AutosleepMode::Off,
                AutosleepMode::Freeze,
                AutosleepMode::Mem
            ]
        );
    }

    #[test]
    fn discover_missing_file_returns_off_only() {
        let modes = discover_available_modes(Path::new("/nonexistent/sys/power/state"));
        assert_eq!(modes, vec![AutosleepMode::Off]);
    }

    #[test]
    fn write_sysfs_missing_path_is_noop() {
        assert_eq!(
            write_sysfs(Path::new("/nonexistent/sys/power/autosleep"), "mem").unwrap(),
            SysfsWrite::Missing
        );
    }

    #[test]
    fn write_sysfs_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("autosleep");
        fs::create_dir(&path).expect("dir instead of file");
        let error = write_sysfs(&path, "mem").unwrap_err();
        assert_eq!(error.path, path);
    }

    #[test]
    fn write_sysfs_writes_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("autosleep");
        fs::write(&path, "off\n").expect("create");
        assert_eq!(write_sysfs(&path, "mem").unwrap(), SysfsWrite::Written);
        assert_eq!(fs::read_to_string(&path).expect("read").trim(), "mem");
    }

    #[test]
    fn probe_supported_when_all_nodes_writable() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        assert!(paths.probe_supported());
        assert!(paths.probe().is_ok());
    }

    #[test]
    fn probe_fails_when_autosleep_missing() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        fs::remove_file(&paths.autosleep).expect("remove");
        assert!(!paths.probe_supported());
        assert!(paths.probe().is_err());
    }

    #[test]
    fn probe_fails_when_wake_lock_is_not_a_writable_file() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        fs::remove_file(&paths.wake_lock).expect("remove");
        fs::create_dir(&paths.wake_lock).expect("create directory");
        assert!(!paths.probe_supported());
        assert!(paths.probe().is_err());
    }

    #[test]
    fn assume_forges_probe_token() {
        let (_dir, paths) = SoftSuspendPaths::test_fixture();
        fs::remove_file(&paths.autosleep).expect("remove");
        assert!(!paths.probe_supported());
        let ok = SoftSuspendProbeOk::assume(paths);
        assert!(!ok.autosleep().exists());
    }
}
