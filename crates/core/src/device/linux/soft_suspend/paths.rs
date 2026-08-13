//! Sysfs paths and discovery helpers for soft suspend.

use crate::device::soft_suspend::AutosleepMode;
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

    /// Returns whether autosleep and wake_lock nodes exist.
    pub fn is_available(&self) -> bool {
        self.autosleep.exists() && self.wake_lock.exists() && self.wake_unlock.exists()
    }
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
}
