//! `cargo xtask download-assets` — download static assets from the Plato release.
//!
//! The Cadmus distribution requires several directories of static assets
//! (`bin/`, `resources/`, `hyphenation-patterns/`) that are not stored in the
//! repository and are not included in the Cadmus release artifact.
//!
//! # Temporary workaround
//!
//! These assets are sourced from the upstream [Plato] release zip until
//! [issue #64] is resolved.  Once Cadmus builds these directories from source
//! as part of `cargo xtask build-kobo`, this entire task becomes unnecessary
//! and should be removed along with the CI step that calls it.
//!
//! [Plato]: https://github.com/baskerville/plato
//! [issue #64]: https://github.com/OGKevin/cadmus/issues/64
//!
//! ## Caching
//!
//! Extracted asset directories are cached under `.cache/plato-assets/<version>/`
//! so that CI can restore them with a version-keyed cache and avoid re-downloading
//! the zip on every run.  The workspace-level directories (`bin/`, `resources/`,
//! `hyphenation-patterns/`) are populated by copying from the cache directory.

use std::path::Path;

use anyhow::{Context, Result};

use super::util::{fs, github, workspace};

/// The Plato GitHub repository in `owner/name` format.
///
/// Asset directories are sourced from here until issue #64 is resolved.
const PLATO_REPO: &str = "baskerville/plato";

/// The pinned Plato release version.
///
/// Tracked by Renovate via a regex manager in `renovate.json`.  Update this
/// constant when a new Plato release is available.
pub const PLATO_VERSION: &str = "0.9.45";

/// Directories extracted from the Plato release zip into the workspace root.
const ASSET_DIRS: &[&str] = &["bin", "resources", "hyphenation-patterns"];

/// Downloads static asset directories from the pinned Plato release.
///
/// Checks `.cache/plato-assets/<version>/` first.  If the cache directory
/// already contains all required asset directories, they are copied from there
/// without hitting the network.  Otherwise the release zip is downloaded,
/// the directories are extracted into the cache, and then copied to the
/// workspace root.
///
/// These directories must exist before Kobo builds that generate compile-time
/// bundled asset metadata, otherwise the generated list will be incomplete.
///
/// Workspace-level directories that already exist are left untouched.
///
/// # Errors
///
/// Returns an error if the GitHub API request fails, the download fails, or
/// extraction fails.
pub fn run() -> Result<()> {
    let root = workspace::root()?;
    let cache_dir = root.join(format!(".cache/plato-assets/{PLATO_VERSION}"));

    let missing: Vec<&str> = ASSET_DIRS
        .iter()
        .copied()
        .filter(|dir| !root.join(dir).exists())
        .collect();

    if missing.is_empty() {
        println!("All asset directories already present, skipping download.");
        return Ok(());
    }

    if cache_dir.exists() && all_dirs_cached(&cache_dir) {
        println!("Restoring assets from cache (.cache/plato-assets/{PLATO_VERSION})…");
        copy_from_cache(&cache_dir, &root, &missing)?;
        return Ok(());
    }

    let asset_name = format!("plato-{PLATO_VERSION}.zip");
    let asset = github::fetch_release_asset(PLATO_REPO, PLATO_VERSION, &asset_name)?;

    println!("Downloading {asset_name} from Plato {PLATO_VERSION}…");

    let archive = root.join(&asset_name);
    github::download_asset(&asset, &archive).context("failed to download Plato release archive")?;

    std::fs::create_dir_all(&cache_dir).context("failed to create plato-assets cache directory")?;

    fs::extract_zip_paths(&archive, &cache_dir, ASSET_DIRS)
        .context("failed to extract asset directories from Plato release archive")?;

    std::fs::remove_file(&archive).ok();

    copy_from_cache(&cache_dir, &root, &missing)?;

    for dir in &missing {
        println!("Extracted {dir}/");
    }

    Ok(())
}

/// Returns `true` if every asset directory is present in `cache_dir`.
fn all_dirs_cached(cache_dir: &Path) -> bool {
    ASSET_DIRS.iter().all(|dir| cache_dir.join(dir).exists())
}

/// Copies asset directories from `cache_dir` into `dest` for each name in `dirs`.
fn copy_from_cache(cache_dir: &Path, dest: &Path, dirs: &[&str]) -> Result<()> {
    for dir in dirs {
        let src = cache_dir.join(dir);
        let dst = dest.join(dir);
        fs::copy_dir_all(&src, &dst)
            .with_context(|| format!("failed to copy {dir}/ from cache to workspace"))?;
        println!("Restored {dir}/ from cache");
    }
    Ok(())
}
