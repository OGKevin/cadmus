//! Marker files written into build directories so subsequent builds can
//! skip work that is already done.
//!
//! Marker files live next to the artifacts they describe. Removing
//! them is the supported way to force a rebuild of just the affected
//! library without clearing the whole target directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// File name written into a MuPDF source tree after the WebP support
/// patches have been applied. Presence of this file indicates the
/// patches are already in place and re-application can be skipped.
pub const WEBP_PATCHED_MARKER: &str = ".webp-patched";

/// File name written into a per-library build directory after the
/// library's build recipe has completed successfully. Presence of
/// this file indicates the build is cached and can be skipped.
pub const BUILT_MARKER: &str = ".built";

/// Returns the absolute path of the [`BUILT_MARKER`] for `dir`.
pub fn built_marker_path(dir: &Path) -> PathBuf {
    dir.join(BUILT_MARKER)
}

/// Returns `true` if [`BUILT_MARKER`] is present in `dir`.
pub fn is_built(dir: &Path) -> bool {
    built_marker_path(dir).exists()
}

/// Write [`BUILT_MARKER`] in `dir`, recording that `name` has been
/// built successfully.
///
/// # Errors
///
/// Returns an error if the marker file cannot be written.
pub fn mark_built(dir: &Path, name: &str) -> Result<()> {
    write_marker(dir, BUILT_MARKER, name, "build")
}

/// Returns `true` if [`WEBP_PATCHED_MARKER`] is present in `mupdf_dir`.
pub fn is_webp_patched(mupdf_dir: &Path) -> bool {
    mupdf_dir.join(WEBP_PATCHED_MARKER).exists()
}

/// Write an empty marker file at `<dir>/<marker>`, recording that the
/// build step named `name` (described as `state`) has completed.
///
/// # Errors
///
/// Returns an error if the marker file cannot be written.
pub fn write_marker(dir: &Path, marker: &str, name: &str, state: &str) -> Result<()> {
    std::fs::write(dir.join(marker), "")
        .with_context(|| format!("failed to write {state} marker for {name}"))
}
