//! Bundled font sources for [`super::run`].
//!
//! Each submodule corresponds to one upstream (ebook-fonts, Libertinus, Noto,
//! Source Code, Google Fonts) and exposes its own [`install`](ebook::install)
//! and [`is_complete`](ebook::is_complete).  [`install`] and [`is_complete`]
//! at this level orchestrate every source.

use std::path::Path;

use anyhow::{Context, Result};

use build_deps::ensure_submodules;

pub mod ebook;
pub mod google;
pub mod libertinus;
pub mod noto;
pub mod source_code;

/// Installs every bundled font source into `fonts_dir`.
///
/// Downloads are cached under `root/.cache/`.  Submodule-backed sources rely on
/// [`ensure_submodules`] from `build-deps`.  Existing destination files are left
/// untouched.
///
/// # Errors
///
/// Returns an error if any source fails to download or extract.
pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    ensure_submodules(root).context("failed to initialise git submodules")?;
    ebook::install(root, fonts_dir)?;
    libertinus::install(root, fonts_dir)?;
    noto::install(root, fonts_dir)?;
    source_code::install(root, fonts_dir)?;
    google::install(root, fonts_dir)?;
    Ok(())
}

/// Returns `true` when every bundled font source is already present in `fonts_dir`.
pub fn is_complete(root: &Path, fonts_dir: &Path) -> bool {
    ebook::is_complete(fonts_dir)
        && libertinus::is_complete(fonts_dir)
        && noto::is_complete(root, fonts_dir)
        && source_code::is_complete(fonts_dir)
        && google::is_complete(root, fonts_dir)
}
