//! Durable restart marker for a committed OTA deploy.
//!
//! Written after `KoboRoot.tgz` is renamed into place and before bundled files
//! are removed. Startup uses the marker plus whether the bundle is still at the
//! deploy path to finish or abandon the restart.
//!
//! | Marker | Bundle at deploy path | Action |
//! | ------ | --------------------- | ------ |
//! | absent | — | Start normally. |
//! | present | gone | Firmware applied the update. Clear the marker. |
//! | present | still there, under the attempt limit | Request reboot again. |
//! | present | still there, at the limit | Report failure; clear the marker after the notice is queued. |

use crate::device::ExitStatus;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Maximum restart attempts for a bundle the firmware will not consume.
pub(crate) const RESTART_ATTEMPT_LIMIT: u32 = 3;

const MARKER_NAME: &str = "ota-restart-required";

/// Startup decision from the marker and whether the deployed bundle is gone.
///
/// Absence of the marker is [`Self::Continue`] without inspecting the deploy
/// path. The firmware consuming the bundle is what distinguishes success from a
/// stranded install — not clearing the marker at the moment reboot is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "kobo", test))]
pub(crate) enum StartupAction {
    /// No restart work; start the application.
    Continue,
    /// Bundle is still at the deploy path; reboot so firmware can apply it.
    RequestRestart,
    /// Attempt limit reached with the bundle still present; start anyway.
    ReportUnapplied,
}

/// Marker file under `data_dir`; presence means a committed deploy needs reboot.
pub(crate) fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MARKER_NAME)
}

/// Writes the restart marker with attempt count zero.
///
/// Call after the bundle is at the deploy path and before bundled files are
/// removed. A crash between those steps is the stranded state this marker
/// exists to recover.
#[cfg_attr(feature = "tracing", tracing::instrument(fields(data_dir = %data_dir.display())))]
pub(crate) fn write_restart_marker(data_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    write_attempts(data_dir, 0)
}

/// Increments the stored attempt count, creating the marker at 1 if missing.
#[cfg_attr(feature = "tracing", tracing::instrument(fields(data_dir = %data_dir.display())))]
pub(crate) fn record_restart_attempt(data_dir: &Path) -> io::Result<u32> {
    let next = read_attempts(data_dir)?.unwrap_or(0).saturating_add(1);
    write_attempts(data_dir, next)?;
    tracing::info!(
        attempts = next,
        limit = RESTART_ATTEMPT_LIMIT,
        "recorded OTA restart attempt"
    );
    Ok(next)
}

/// Chooses a [`StartupAction`] from the marker and whether `deploy_path` still
/// exists.
///
/// The marker is cleared when the bundle is gone. At the attempt limit it stays
/// until the caller queues the failure notice, so a crash before that send can
/// report the failure again. Clearing it when reboot is *requested* would lose
/// both the marker and the reboot if the process dies before the event is
/// delivered.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(fields(data_dir = %data_dir.display(), deploy = %deploy_path.display()))
)]
#[cfg(any(feature = "kobo", test))]
pub(crate) fn reconcile_startup(data_dir: &Path, deploy_path: &Path) -> io::Result<StartupAction> {
    let Some(attempts) = read_attempts(data_dir)? else {
        tracing::debug!("no OTA restart marker");
        return Ok(StartupAction::Continue);
    };

    if !deploy_path.exists() {
        clear_marker(data_dir)?;
        tracing::info!("OTA bundle applied; cleared restart marker");
        return Ok(StartupAction::Continue);
    }

    if attempts >= RESTART_ATTEMPT_LIMIT {
        tracing::error!(
            attempts,
            limit = RESTART_ATTEMPT_LIMIT,
            "OTA restart attempt limit reached"
        );
        return Ok(StartupAction::ReportUnapplied);
    }

    record_restart_attempt(data_dir)?;
    tracing::warn!(attempts, "stranded OTA deploy; requesting restart");
    Ok(StartupAction::RequestRestart)
}

/// Whether shutdown should still reboot so firmware can apply a committed update.
///
/// False when the marker is absent, the bundle is gone, or the attempt limit
/// has already been reached. The limit case keeps the marker so startup can
/// report the failure, and must not reboot again.
pub(crate) fn committed_update_pending(data_dir: &Path, deploy_path: &Path) -> bool {
    if !deploy_path.exists() {
        return false;
    }
    match read_attempts(data_dir) {
        Ok(Some(attempts)) => attempts < RESTART_ATTEMPT_LIMIT,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "failed to read OTA restart marker while checking a pending update"
            );
            marker_path(data_dir).exists()
        }
    }
}

/// Promotes a non-reboot exit to [`ExitStatus::Reboot`] when a committed update
/// is still pending.
///
/// An already-requested reboot is left unchanged. Used at process shutdown so
/// quitting after a committed deploy still reaches the firmware applicator.
/// A failed attempt write leaves `status` unchanged, so a reboot is not
/// requested without a durable attempt count.
pub fn exit_status_for_shutdown(
    status: ExitStatus,
    data_dir: &Path,
    deploy_path: &Path,
) -> ExitStatus {
    if matches!(status, ExitStatus::Reboot) {
        return status;
    }
    if !committed_update_pending(data_dir, deploy_path) {
        return status;
    }
    if let Err(error) = record_restart_attempt(data_dir) {
        tracing::error!(
            error = %error,
            "failed to increment OTA restart attempts on shutdown"
        );
        return status;
    }
    tracing::info!("committed OTA deploy still pending; converting shutdown to reboot");
    ExitStatus::Reboot
}

#[cfg_attr(feature = "tracing", tracing::instrument(fields(data_dir = %data_dir.display())))]
fn read_attempts(data_dir: &Path) -> io::Result<Option<u32>> {
    let path = marker_path(data_dir);
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let attempts = contents.trim().parse().unwrap_or(0);
            Ok(Some(attempts))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
pub(crate) fn write_attempts_for_test(data_dir: &Path, attempts: u32) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    write_attempts(data_dir, attempts)
}

/// Durably records the attempt count so power loss cannot drop the marker
/// after the bundle is already at the deploy path.
fn write_attempts(data_dir: &Path, attempts: u32) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let dest = marker_path(data_dir);
    let staging = data_dir.join(format!("{MARKER_NAME}.tmp"));
    {
        let mut file = File::create(&staging)?;
        file.write_all(attempts.to_string().as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&staging, &dest)?;
    sync_dir(data_dir)
}

/// Flushes directory entries so a rename survives power loss.
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(path)?
            .sync_all()?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

/// Removes the restart marker after the unapplied-update notice has been queued.
#[cfg(any(feature = "kobo", test))]
pub(crate) fn clear_restart_marker(data_dir: &Path) -> io::Result<()> {
    clear_marker(data_dir)
}

#[cfg(any(feature = "kobo", test))]
fn clear_marker(data_dir: &Path) -> io::Result<()> {
    match fs::remove_file(marker_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ota::clean_bundled_files;

    fn data_and_deploy() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let deploy_path = tmp.path().join(".kobo").join("KoboRoot.tgz");
        if let Some(parent) = deploy_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        (tmp, data_dir, deploy_path)
    }

    #[test]
    fn test_marker_absent_starts_normally() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::Continue
        );
        assert!(!marker_path(&data_dir).exists());
    }

    #[test]
    fn test_marker_with_bundle_absent_clears_and_starts_normally() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        write_restart_marker(&data_dir).unwrap();
        assert!(!deploy_path.exists());

        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::Continue
        );
        assert!(!marker_path(&data_dir).exists());
    }

    #[test]
    fn test_marker_with_bundle_present_requests_restart_and_increments() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        write_restart_marker(&data_dir).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::RequestRestart
        );
        assert_eq!(read_attempts(&data_dir).unwrap(), Some(1));
        assert!(marker_path(&data_dir).exists());
        assert!(deploy_path.exists());
    }

    #[test]
    fn test_attempt_limit_stops_restarts_and_reports_failure() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        write_attempts(&data_dir, RESTART_ATTEMPT_LIMIT).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::ReportUnapplied
        );
        assert!(marker_path(&data_dir).exists());
        assert_eq!(
            read_attempts(&data_dir).unwrap(),
            Some(RESTART_ATTEMPT_LIMIT)
        );
        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::ReportUnapplied
        );

        clear_restart_marker(&data_dir).unwrap();
        assert_eq!(
            reconcile_startup(&data_dir, &deploy_path).unwrap(),
            StartupAction::Continue
        );
        assert!(deploy_path.exists());
    }

    #[test]
    fn test_attempt_limit_does_not_reboot_on_shutdown() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        write_attempts(&data_dir, RESTART_ATTEMPT_LIMIT).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        assert!(!committed_update_pending(&data_dir, &deploy_path));
        assert_eq!(
            exit_status_for_shutdown(ExitStatus::Quit, &data_dir, &deploy_path),
            ExitStatus::Quit
        );
        assert_eq!(
            read_attempts(&data_dir).unwrap(),
            Some(RESTART_ATTEMPT_LIMIT)
        );
    }

    #[test]
    fn test_marker_survives_clean_bundled_files_when_dirs_are_the_same() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let shared = tmp.path().join("install");
        fs::create_dir_all(shared.join("fonts")).unwrap();
        fs::create_dir_all(shared.join("icons")).unwrap();
        fs::write(shared.join("fonts/Libron-Regular.ttf"), b"owned").unwrap();
        fs::write(shared.join("icons/home.svg"), b"owned").unwrap();
        write_restart_marker(&shared).unwrap();

        clean_bundled_files(&shared).unwrap();

        assert!(marker_path(&shared).exists());
        assert_eq!(read_attempts(&shared).unwrap(), Some(0));
    }

    #[test]
    fn test_shutdown_keeps_status_when_attempt_write_fails() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        fs::create_dir(marker_path(&data_dir)).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        assert!(committed_update_pending(&data_dir, &deploy_path));
        assert_eq!(
            exit_status_for_shutdown(ExitStatus::Quit, &data_dir, &deploy_path),
            ExitStatus::Quit
        );
        assert!(marker_path(&data_dir).is_dir());
    }

    #[test]
    fn test_shutdown_after_commit_reaches_restart() {
        let (_tmp, data_dir, deploy_path) = data_and_deploy();
        write_restart_marker(&data_dir).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        assert_eq!(
            exit_status_for_shutdown(ExitStatus::Quit, &data_dir, &deploy_path),
            ExitStatus::Reboot
        );
        assert_eq!(read_attempts(&data_dir).unwrap(), Some(1));
        assert_eq!(
            exit_status_for_shutdown(ExitStatus::Reboot, &data_dir, &deploy_path),
            ExitStatus::Reboot
        );
        assert_eq!(read_attempts(&data_dir).unwrap(), Some(1));
    }
}
