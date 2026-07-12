//! Shared helpers for font sources.
//!
//! Font submodules under [`super::font`] copy files from `thirdparty/` via
//! [`install_from_submodule`].  Release archives are extracted with helpers in
//! [`crate::tasks::util::fs`].

use std::path::Path;

use anyhow::{Context, Result};

/// Copies `files` from a submodule into `fonts_dir`.
///
/// Each entry is `(dest_filename, path_relative_to_submodule_root)`.  Files
/// that already exist in `fonts_dir` are skipped.
///
/// Call [`build_deps::ensure_submodules`] before invoking this function.
///
/// # Errors
///
/// Returns an error if the submodule is missing a source file or a copy fails.
pub fn install_from_submodule(
    root: &Path,
    submodule: &str,
    fonts_dir: &Path,
    files: &[(&str, &str)],
) -> Result<()> {
    let submodule_root = root.join(submodule);
    if !submodule_root.is_dir() {
        anyhow::bail!(
            "{submodule} not found — run `git submodule update --init --recursive` first"
        );
    }

    for &(dest_name, rel_path) in files {
        let dest = fonts_dir.join(dest_name);
        if dest.exists() {
            continue;
        }
        let src = submodule_root.join(rel_path);
        if !src.is_file() {
            anyhow::bail!(
                "missing font file {} in submodule {submodule}",
                src.display()
            );
        }
        println!("Copying {dest_name} from {submodule}/{rel_path}…");
        std::fs::copy(&src, &dest)
            .with_context(|| format!("failed to copy {dest_name} from {submodule}"))?;
    }

    Ok(())
}
