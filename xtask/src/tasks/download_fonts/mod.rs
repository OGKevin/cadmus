//! `cargo xtask download-fonts` — download bundled font files for Cadmus.
//!
//! Assembles the `fonts/` directory from reader sources and pinned UI
//! upstreams. Each source lives under [`font`] and exposes an `install`
//! function.
//!
//! ## Caching
//!
//! Release archives are cached locally under `.cache/` during download.
//! CI restores the assembled `fonts/` directory via the `cache-fonts` action.
//! Noto and Google UI fonts are copied from git submodules under `thirdparty/`.

mod font;
mod util;

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::tasks::util::workspace;

pub use font::ebook::EBOOK_FONTS_VERSION;
pub use font::libertinus::LIBERTINUS_VERSION;
pub use font::source_code::SOURCE_CODE_RELEASE;

/// Downloads and assembles the workspace `fonts/` directory.
///
/// Skips network I/O when every expected file is already present.
///
/// # Errors
///
/// Returns an error if any download or extraction step fails.
pub fn run() -> Result<()> {
    let root = workspace::root()?;
    let fonts_dir = root.join("fonts");

    if all_fonts_present(&root, &fonts_dir) {
        println!("All font files already present in fonts/, skipping download.");
        return Ok(());
    }

    std::fs::create_dir_all(&fonts_dir).context("failed to create fonts/ directory")?;

    font::install(&root, &fonts_dir)?;

    if !all_fonts_present(&root, &fonts_dir) {
        bail!("fonts/ is still incomplete after download-fonts");
    }

    let file_count = std::fs::read_dir(&fonts_dir)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    println!("fonts/ is ready ({file_count} files).");
    Ok(())
}

fn all_fonts_present(root: &Path, fonts_dir: &Path) -> bool {
    fonts_dir.exists() && font::is_complete(root, fonts_dir)
}
