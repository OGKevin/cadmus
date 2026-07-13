use std::path::Path;

use anyhow::{Context, Result};

use crate::tasks::download_fonts::util;
use crate::tasks::util::{fs, github};

const REPO: &str = "adobe-fonts/source-code-pro";
/// Tracked by Renovate via a regex manager in `renovate.json`.
pub const SOURCE_CODE_RELEASE: &str = "2.042R-u/1.062R-i/1.026R-vf";
/// Asset name bundled in [`SOURCE_CODE_RELEASE`]; update manually when the
/// upstream release layout changes.
const VF_ASSET: &str = "VF-source-code-VF-1.026R.zip";

const FILES: &[&str] = &[
    "SourceCodeVariable-Roman.otf",
    "SourceCodeVariable-Italic.otf",
];

const ZIP_ENTRIES: &[(&str, &str)] = &[
    (
        "VF/SourceCodeVF-Upright.otf",
        "SourceCodeVariable-Roman.otf",
    ),
    (
        "VF/SourceCodeVF-Italic.otf",
        "SourceCodeVariable-Italic.otf",
    ),
];

pub fn is_complete(fonts_dir: &Path) -> bool {
    FILES.iter().all(|name| fonts_dir.join(name).exists())
}

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    if is_complete(fonts_dir) {
        return Ok(());
    }

    let cache_dir = root.join(format!(".cache/source-code-pro/{SOURCE_CODE_RELEASE}"));
    let archive = util::ensure_cached_release_asset(&cache_dir, VF_ASSET, || {
        println!("Fetching {VF_ASSET} from {REPO} {SOURCE_CODE_RELEASE}…");
        github::fetch_release_asset(REPO, SOURCE_CODE_RELEASE, VF_ASSET)
    })?;

    util::extract_cached_archive(
        &archive,
        || {
            util::ensure_cached_release_asset(&cache_dir, VF_ASSET, || {
                println!("Re-fetching {VF_ASSET} from {REPO} {SOURCE_CODE_RELEASE}…");
                github::fetch_release_asset(REPO, SOURCE_CODE_RELEASE, VF_ASSET)
            })
            .map(|_| ())
        },
        |archive| {
            fs::extract_zip_entries_flat(archive, fonts_dir, ZIP_ENTRIES)
                .context("failed to extract Source Code VF fonts")
        },
    )
}
