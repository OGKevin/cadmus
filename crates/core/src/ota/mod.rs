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

pub use crate::github::OtaProgress;
pub use cleanup::clean_bundled_files;
pub use client::{ArtifactSource, OtaClient, OtaError};
