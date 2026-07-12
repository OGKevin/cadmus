use std::path::Path;

use anyhow::{Context, Result};
use build_deps::markers;

use crate::tasks::util::{fs, github};

const REPO: &str = "nicoverbruggen/ebook-fonts";
/// Tracked by Renovate via a regex manager in `renovate.json`.
pub const EBOOK_FONTS_VERSION: &str = "v2026.07.02";
const CORE_ASSET: &str = "other-core-fonts.zip";
const EXTRA_ASSET: &str = "other-extra-fonts.zip";

const CORE_FILES: &[&str] = &[
    "Cartisse-Bold.ttf",
    "Cartisse-BoldItalic.ttf",
    "Cartisse-Italic.ttf",
    "Cartisse-Regular.ttf",
    "Libron-Bold.ttf",
    "Libron-BoldItalic.ttf",
    "Libron-Italic.ttf",
    "Libron-Regular.ttf",
    "NV_Bitter-Bold.ttf",
    "NV_Bitter-BoldItalic.ttf",
    "NV_Bitter-Italic.ttf",
    "NV_Bitter-Regular.ttf",
    "NV_Charis-Bold.ttf",
    "NV_Charis-BoldItalic.ttf",
    "NV_Charis-Italic.ttf",
    "NV_Charis-Regular.ttf",
    "NV_Garamond-Bold.ttf",
    "NV_Garamond-BoldItalic.ttf",
    "NV_Garamond-Italic.ttf",
    "NV_Garamond-Regular.ttf",
    "NV_Jost-Bold.ttf",
    "NV_Jost-BoldItalic.ttf",
    "NV_Jost-Italic.ttf",
    "NV_Jost-Regular.ttf",
    "NV_Legible_Next-Bold.ttf",
    "NV_Legible_Next-BoldItalic.ttf",
    "NV_Legible_Next-Italic.ttf",
    "NV_Legible_Next-Regular.ttf",
    "NV_Palatium-Bold.ttf",
    "NV_Palatium-BoldItalic.ttf",
    "NV_Palatium-Italic.ttf",
    "NV_Palatium-Regular.ttf",
    "Sourcerer-Bold.ttf",
    "Sourcerer-BoldItalic.ttf",
    "Sourcerer-Italic.ttf",
    "Sourcerer-Regular.ttf",
];

const EXTRA_FILES: &[&str] = &[
    "NV_Libertinus-Bold.ttf",
    "NV_Libertinus-BoldItalic.ttf",
    "NV_Libertinus-Italic.ttf",
    "NV_Libertinus-Regular.ttf",
];

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    let cache_dir = root.join(format!(".cache/ebook-fonts/{EBOOK_FONTS_VERSION}"));

    let core_archive = ensure_cached_archive(&cache_dir, CORE_ASSET)?;
    fs::extract_zip_matching_flat(&core_archive, fonts_dir, "", ".ttf")
        .context("failed to extract core fonts from ebook-fonts archive")?;

    let extra_archive = ensure_cached_archive(&cache_dir, EXTRA_ASSET)?;
    fs::extract_zip_matching_flat(&extra_archive, fonts_dir, "NV_Libertinus", ".ttf")
        .context("failed to extract extra fonts from ebook-fonts extra archive")?;

    markers::mark_version(fonts_dir, "ebook-fonts", EBOOK_FONTS_VERSION)?;
    Ok(())
}

pub fn is_complete(fonts_dir: &Path) -> bool {
    CORE_FILES
        .iter()
        .chain(EXTRA_FILES.iter())
        .all(|name| fonts_dir.join(name).exists())
        && markers::is_version_current(fonts_dir, EBOOK_FONTS_VERSION)
}

fn ensure_cached_archive(cache_dir: &Path, asset: &str) -> Result<std::path::PathBuf> {
    let archive = cache_dir.join(asset);

    if !archive.exists() {
        std::fs::create_dir_all(cache_dir).context("failed to create ebook-fonts cache dir")?;
        let release_asset = github::fetch_release_asset(REPO, EBOOK_FONTS_VERSION, asset)?;
        println!("Downloading {asset} from {REPO} {EBOOK_FONTS_VERSION}…");
        github::download_asset(&release_asset, &archive)
            .context("failed to download ebook-fonts archive")?;
    } else {
        println!("Using cached {asset}");
    }

    Ok(archive)
}
