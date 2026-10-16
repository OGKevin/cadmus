//! Over-the-air updates as a Slint screen.
//!
//! The screen asks where to install from, then downloads on a view-owned job.
//! Pull requests are a list, not a number the reader has to type. The list
//! appears as soon as the open pull requests arrive. A row can be installed
//! only when a live build shows up.

mod auth;
mod auth_panel;
mod download;
mod list;
mod progress;
mod view;

pub use self::view::{Ota, OtaView};

use self::download::DownloadOutcome;
use crate::ota::CheckedPull;

/// Events the OTA jobs send back to the screen.
#[derive(Debug, Clone)]
pub enum OtaEvent {
    /// The open pull requests arrived. Install status is still unknown.
    PrListed {
        /// Bumped each time the screen starts a fetch, so a late job is ignored.
        generation: u64,
        /// Every open pull request, newest activity first. Each build is pending.
        rows: Vec<CheckedPull>,
    },
    /// A newer picture of which rows can be installed.
    PrStatus {
        /// See [`Self::PrListed::generation`].
        generation: u64,
        /// The full list. Ready rows can be tapped. The rest stay pending until
        /// `failed` is set or the check finishes.
        rows: Vec<CheckedPull>,
        /// The check stopped early. Ready rows stay ready.
        failed: bool,
    },
    /// The open-pull list itself failed. No rows were replaced.
    PrListFailed {
        /// See [`Self::PrListed::generation`].
        generation: u64,
    },
    /// The progress screen should close.
    DownloadFinished(DownloadOutcome),
}
