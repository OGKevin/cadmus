//! Shared MuPDF build configuration and source preparation.
//!
//! Both the native and Kobo build flows share the same `make libs`
//! feature flags and core `XCFLAGS`, defined in this module so each
//! target only supplies its platform-specific pieces (WebP include
//! path, `OS=kobo`, native-only output disables, …).
//!
//! Both flows apply Cadmus patches from `build-scripts/mupdf/{generic,kobo}/`
//! before compile. Patch application is centralized in [`crate::patches`];
//! the native flow uses [`crate::patches::PatchProfile::Native`].
//!
//! A [`.patches-applied`](crate::markers::PATCHES_APPLIED_MARKER) marker
//! file is written under the patched tree on success. Re-application is
//! skipped while the marker is present, which keeps re-runs cheap when
//! the build tree is reused (the native flow) and stays correct when the
//! build tree is recreated from scratch (the Kobo flow).

use std::path::Path;

use anyhow::{Context, Result};

use crate::markers;
use crate::patches;

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

/// Apply sorted MuPDF patches when not already applied.
///
/// Returns `Ok(true)` when patches were applied during this call,
/// `Ok(false)` when they were already in place.
pub fn apply_mupdf_patches_if_needed(
    mupdf_dir: &Path,
    root: &Path,
    profile: patches::PatchProfile,
) -> Result<bool> {
    if markers::is_patches_applied(mupdf_dir) {
        println!("MuPDF patches already applied.");
        return Ok(false);
    }

    println!("Applying MuPDF patches...");
    let patches_dir = root.join("build-scripts/mupdf");
    patches::apply_sorted_patches(mupdf_dir, &patches_dir, profile)
        .context("failed to apply MuPDF patches")?;

    markers::write_marker(mupdf_dir, markers::PATCHES_APPLIED_MARKER, "mupdf", "patch")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patches::PatchProfile;

    #[test]
    fn apply_mupdf_patches_if_needed_skips_when_marker_present() {
        let tmp = tempfile::tempdir().unwrap();
        let mupdf_dir = tmp.path().join("mupdf");
        std::fs::create_dir_all(&mupdf_dir).unwrap();
        markers::write_marker(
            &mupdf_dir,
            markers::PATCHES_APPLIED_MARKER,
            "mupdf",
            "patch",
        )
        .unwrap();

        let applied =
            apply_mupdf_patches_if_needed(&mupdf_dir, tmp.path(), PatchProfile::Native).unwrap();
        assert!(!applied);
    }

    #[test]
    fn apply_mupdf_patches_if_needed_applies_generic_patches_and_writes_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mupdf_dir = root.join("mupdf");
        let patches_dir = root.join("build-scripts/mupdf/generic");
        std::fs::create_dir_all(&mupdf_dir).unwrap();
        std::fs::create_dir_all(&patches_dir).unwrap();
        std::fs::write(mupdf_dir.join("note.txt"), "alpha\n").unwrap();

        let patch = "\
--- a/note.txt\n\
+++ b/note.txt\n\
@@ -1 +1 @@\n\
-alpha\n\
+beta\n\
";
        std::fs::write(patches_dir.join("010-note.patch"), patch).unwrap();

        let applied =
            apply_mupdf_patches_if_needed(&mupdf_dir, root, PatchProfile::Native).unwrap();
        assert!(applied);
        assert!(markers::is_patches_applied(&mupdf_dir));
        assert_eq!(
            std::fs::read_to_string(mupdf_dir.join("note.txt")).unwrap(),
            "beta\n"
        );

        let again = apply_mupdf_patches_if_needed(&mupdf_dir, root, PatchProfile::Native).unwrap();
        assert!(!again);
    }
}
