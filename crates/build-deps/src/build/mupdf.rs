//! Shared MuPDF source preparation.
//!
//! Both the native and Kobo build flows need to apply the Cadmus
//! WebP support patch series to a MuPDF source tree before it is
//! compiled. The series is defined in
//! [`crate::versions::MUPDF_WEBP_PATCHES`] and is identical for both
//! targets, so the application logic lives here in one place.
//!
//! A `.webp-patched` marker file is written under the patched tree on
//! success. Re-application is skipped while the marker is present,
//! which keeps re-runs cheap when the build tree is reused (the
//! native flow) and stays correct when the build tree is recreated
//! from scratch (the Kobo flow).

use std::path::Path;

use anyhow::{Context, Result};

use crate::cmd;
use crate::markers;

/// Apply the Cadmus WebP support patch series to a MuPDF source tree
/// if the patches have not been applied yet.
///
/// A `.webp-patched` marker file is written under `mupdf_dir` after a
/// successful application and re-applications are skipped while it
/// exists. Returns `Ok(true)` when patches were applied during this
/// call, `Ok(false)` when they were already in place.
///
/// # Errors
///
/// Returns an error if the patch list cannot be enumerated, any patch
/// fails to apply, or the marker file cannot be written.
pub fn apply_webp_patches_if_needed(mupdf_dir: &Path, root: &Path) -> Result<bool> {
    if markers::is_webp_patched(mupdf_dir) {
        println!("MuPDF WebP patches already applied.");
        return Ok(false);
    }

    println!("Applying MuPDF WebP patches...");
    let patches_dir = root.join("build-scripts/mupdf");
    for patch in crate::versions::MUPDF_WEBP_PATCHES {
        let patch_path = patches_dir.join(patch);
        let patch_str = patch_path
            .to_str()
            .context("patch path is not valid UTF-8")?;
        cmd::run("patch", &["-p", "1", "-i", patch_str], mupdf_dir, &[])
            .with_context(|| format!("failed to apply {patch}"))?;
    }

    markers::write_marker(
        mupdf_dir,
        markers::WEBP_PATCHED_MARKER,
        "mupdf",
        "WebP patch",
    )?;
    Ok(true)
}
