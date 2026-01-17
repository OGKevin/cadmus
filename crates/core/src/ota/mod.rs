//! Over-the-Air (OTA) update functionality for downloading and installing builds from GitHub.
//!
//! This module provides capabilities to:
//! - Download build artifacts from GitHub Actions workflows
//! - Extract and deploy KoboRoot.tgz packages
//! - Track download progress with callbacks
//!
//! The OTA client requires a GitHub personal access token with permissions to
//! read workflow artifacts from the ogkevin/cadmus repository.

mod client;

pub use client::{OtaClient, OtaError, OtaProgress};
