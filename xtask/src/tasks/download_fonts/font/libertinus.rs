use std::path::Path;

use anyhow::{Context, Result};

use crate::tasks::download_fonts::util;
use crate::tasks::util::{fs, github};

const REPO: &str = "alerque/libertinus";
/// Tracked by Renovate via a regex manager in `renovate.json`.
///
/// GitHub release tags use a `v` prefix; archive names and in-zip paths omit it
/// (e.g. tag `v7.051` → asset `Libertinus-7.051.zip`).
pub const LIBERTINUS_VERSION: &str = "v7.051";

const FILES: &[&str] = &[
    "LibertinusSerif-Regular.otf",
    "LibertinusSerif-Italic.otf",
    "LibertinusSerif-Bold.otf",
    "LibertinusSerif-BoldItalic.otf",
    "LibertinusSans-Regular.otf",
    "LibertinusSans-Italic.otf",
    "LibertinusSans-Bold.otf",
    "LibertinusMono-Regular.otf",
];

pub fn is_complete(fonts_dir: &Path) -> bool {
    FILES.iter().all(|name| fonts_dir.join(name).exists())
}

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    if is_complete(fonts_dir) {
        return Ok(());
    }

    let cache_dir = root.join(format!(".cache/libertinus/{LIBERTINUS_VERSION}"));
    let archive_version = LIBERTINUS_VERSION
        .strip_prefix('v')
        .unwrap_or(LIBERTINUS_VERSION);
    let asset_name = format!("Libertinus-{archive_version}.zip");

    let archive = util::ensure_cached_release_asset(&cache_dir, &asset_name, || {
        println!("Fetching {asset_name} from {REPO} {LIBERTINUS_VERSION}…");
        github::fetch_release_asset(REPO, LIBERTINUS_VERSION, &asset_name)
    })?;

    let otf_prefix = format!("Libertinus-{archive_version}/static/OTF/");
    let entries: Vec<(String, String)> = FILES
        .iter()
        .map(|name| (format!("{otf_prefix}{name}"), (*name).to_string()))
        .collect();

    util::extract_cached_archive(
        &archive,
        || {
            util::ensure_cached_release_asset(&cache_dir, &asset_name, || {
                println!("Re-fetching {asset_name} from {REPO} {LIBERTINUS_VERSION}…");
                github::fetch_release_asset(REPO, LIBERTINUS_VERSION, &asset_name)
            })
            .map(|_| ())
        },
        |archive| {
            fs::extract_zip_entries_flat(archive, fonts_dir, &entries)
                .context("failed to extract Libertinus fonts")
        },
    )
}
