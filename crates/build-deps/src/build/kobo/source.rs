//! Prepare a clean, patched copy of each thirdparty library's source
//! tree inside the per-target build directory.
//!
//! The Kobo cross-build copies the submodule into
//! `target/cadmus-build-deps/<TARGET>/<lib>/`, then layers in the
//! build scripts and patches kept under `build-scripts/<lib>/`.
//!
//! For MuPDF, the per-library `kobo.patch` is applied first, then the
//! shared WebP support patch series is applied via
//! [`crate::build::mupdf::apply_webp_patches_if_needed`].

use std::path::Path;

use anyhow::{Context, Result};

use crate::build::mupdf;
use crate::cmd;
use crate::utils;

/// Copy a library's source tree into `build_dir` and overlay any
/// build scripts kept under `build-scripts/<lib>/` (typically
/// `kobo.patch`, `kobo-options.txt`, etc.).
///
/// Skips git metadata, `build/`, `objs/` and `autom4te.cache/` via
/// [`utils::cp_r`].
pub fn copy_source(src_dir: &Path, build_dir: &Path, name: &str, root: &Path) -> Result<()> {
    println!("Copying {name} source...");

    utils::cp_r(src_dir, build_dir)?;

    let scripts_dir = root.join("build-scripts").join(name);
    if scripts_dir.exists() {
        for entry in std::fs::read_dir(&scripts_dir)
            .with_context(|| format!("failed to read build-scripts/{name}"))?
        {
            let entry = entry?;
            let src = entry.path();
            let dest = build_dir.join(entry.file_name());
            std::fs::copy(&src, &dest).with_context(|| {
                format!("failed to copy {} to {}", src.display(), dest.display())
            })?;
        }
    }

    Ok(())
}

/// Apply the `kobo.patch` (and, for MuPDF, the WebP support patch
/// series) to a freshly copied build tree.
///
/// `patch --forward` is used so already-applied patches are skipped.
/// A `--dry-run` fallback detects patches that were applied in
/// reverse and treats them as no-ops.
pub fn apply_patches(build_dir: &Path, name: &str, root: &Path) -> Result<()> {
    let patches_dir = root.join("build-scripts").join(name);

    let kobo_patch = patches_dir.join("kobo.patch");
    if kobo_patch.exists() {
        let kobo_patch_str = kobo_patch
            .to_str()
            .context("kobo.patch path is not valid UTF-8")?;
        match cmd::run(
            "patch",
            &["-p", "1", "-i", kobo_patch_str, "--forward"],
            build_dir,
            &[],
        ) {
            Ok(()) => {}
            Err(_) => {
                let dry_run = std::process::Command::new("patch")
                    .args(["-p", "1", "-i", kobo_patch_str, "--dry-run"])
                    .current_dir(build_dir)
                    .output();
                if let Ok(output) = dry_run {
                    let out = String::from_utf8_lossy(&output.stdout);
                    if out.contains("Reversed") || out.contains("Skipping") {
                        return Ok(());
                    }
                }
                return Err(anyhow::anyhow!("failed to apply kobo.patch for {name}"));
            }
        }
    }

    if name == "mupdf" {
        mupdf::apply_webp_patches_if_needed(build_dir, root)
            .with_context(|| format!("failed to apply MuPDF WebP patches for {name}"))?;
    }

    Ok(())
}
