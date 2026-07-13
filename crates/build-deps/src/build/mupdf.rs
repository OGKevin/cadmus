//! Shared MuPDF build configuration and source preparation.
//!
//! Both the native and Kobo build flows share the same `make libs`
//! feature flags and core `XCFLAGS`, defined in this module so each
//! target only supplies its platform-specific pieces (WebP include
//! path, `OS=kobo`, native-only output disables, …).
//!
//! Both flows also apply the Cadmus WebP support patch series to a
//! MuPDF source tree before it is compiled. The series is defined in
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

/// `make` variables passed to every MuPDF `libs` build (native and Kobo).
pub const MAKE_LIBS_ARGS: &[&str] = &[
    "verbose=yes",
    "mujs=no",
    "tesseract=no",
    "extract=no",
    "archive=no",
    "brotli=no",
    "barcode=no",
    "commercial=no",
    "USE_SYSTEM_LIBS=yes",
];

/// C flags appended to `XCFLAGS` for every MuPDF build.
pub const XCFLAGS_SHARED: &str = "-DHAVE_WEBP=1";

/// Build the argument list for `make ... libs`.
pub fn make_libs_invocation(xcflags: &str, extra: &[&str], xlibs: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = MAKE_LIBS_ARGS.iter().copied().map(str::to_owned).collect();
    args.extend(extra.iter().copied().map(str::to_owned));
    args.push(format!("XCFLAGS={xcflags}"));
    if let Some(xlibs) = xlibs {
        args.push(format!("XLIBS={xlibs}"));
    }
    args.push("libs".into());
    args
}

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
