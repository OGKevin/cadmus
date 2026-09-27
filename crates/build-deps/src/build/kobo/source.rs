//! Prepare a clean, patched copy of each thirdparty library's source
//! tree inside the per-target build directory.
//!
//! The Kobo cross-build copies the submodule into
//! `target/cadmus-build-deps/<TARGET>/<lib>/`, then applies patches
//! from `build-scripts/<lib>/` (`generic/` + `kobo/` tiers, sorted
//! file-name order via [`crate::patches::apply_sorted_patches`]).
//!
//! Meson cross files (for example `kobo-options.txt`) stay in
//! `build-scripts/` and are referenced by absolute path from
//! [`super::recipes`].

use std::path::Path;

use anyhow::{Context, Result};

use crate::patches;
use crate::utils;

/// Copy a library's source tree into `build_dir`.
///
/// Skips git metadata, `build/`, `objs/` and `autom4te.cache/` via
/// [`utils::cp_r`].
pub fn copy_source(src_dir: &Path, build_dir: &Path, name: &str) -> Result<()> {
    println!("Copying {name} source...");
    utils::cp_r(src_dir, build_dir)?;
    Ok(())
}

/// Apply sorted patches from `build-scripts/<name>/` to a freshly
/// copied build tree.
///
/// `patch --forward` is used so already-applied patches are skipped.
/// When forward apply fails, a reverse `--dry-run` verifies the patch is
/// fully applied before treating it as a no-op.
pub fn apply_patches(build_dir: &Path, name: &str, root: &Path) -> Result<()> {
    let patches_dir = root.join("build-scripts").join(name);
    patches::apply_sorted_patches(build_dir, &patches_dir, patches::PatchProfile::Kobo)
        .with_context(|| format!("failed to apply patches for {name}"))?;
    Ok(())
}
