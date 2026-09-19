//! Over-the-Air (OTA) update functionality for downloading and installing builds from GitHub.
//!
//! This module provides capabilities to:
//! - Download build artifacts from GitHub Actions workflows
//! - Extract and deploy KoboRoot.tgz packages
//! - Track download progress with callbacks
//!
//! Authentication is handled via GitHub device auth flow — see [`crate::github`].

mod cleanup;
mod client;
mod restart;

pub use crate::github::OtaProgress;
pub use crate::http::{CancelFlag, CancelFunc};
pub use cleanup::{clean_bundled_files, cleanup_ota_artifacts, cleanup_ota_cancel};
pub use client::{ArtifactSource, DeployOutcome, OtaClient, OtaError};
pub use restart::exit_status_for_shutdown;
#[cfg(test)]
pub(crate) use restart::{RESTART_ATTEMPT_LIMIT, marker_path, write_attempts_for_test};
#[cfg(any(feature = "kobo", test))]
pub(crate) use restart::{StartupAction, clear_restart_marker, reconcile_startup};
pub(crate) use restart::{record_restart_attempt, write_restart_marker};
