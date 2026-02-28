//! `cargo xtask bundle` — package a `KoboRoot.tgz` for device installation.
//!
//! Creates one or more `.tgz` archives that can be placed in the `.kobo/`
//! directory on a Kobo device to install or update Cadmus.
//!
//! ## Output files
//!
//! | Flag | Output | Contents |
//! |------|--------|----------|
//! | *(default)* | `bundle/KoboRoot-nm.tgz` | Cadmus + NickelMenu |
//! | `--no-nickel` | `bundle/KoboRoot.tgz` | Cadmus only |
//! | `--test` | `bundle/KoboRoot-nm-test.tgz` | Test build + NickelMenu |
//! | `--test --no-nickel` | `bundle/KoboRoot-test.tgz` | Test build only |
//!
//! ## NickelMenu
//!
//! NickelMenu is downloaded from GitHub Releases and cached in
//! `.cache/nickelmenu/`.
//! The version is controlled by [`NICKEL_VERSION`].  Pass `--skip-download`
//! to use a previously cached archive.

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Args;

use super::util::{fs, github, http, workspace};
/// The NickelMenu release version to bundle.
pub const NICKEL_VERSION: &str = "0.6.0";

/// Arguments for `cargo xtask bundle`.
#[derive(Debug, Args)]
pub struct BundleArgs {
    /// Create a bundle without NickelMenu.
    #[arg(long)]
    pub no_nickel: bool,

    /// Bundle the test build (installs to `.adds/cadmus-tst`).
    #[arg(long)]
    pub test: bool,

    /// Use a cached NickelMenu archive instead of downloading.
    #[arg(long)]
    pub skip_download: bool,
}

/// Packages the distribution directory into a `KoboRoot.tgz`.
///
/// # Errors
///
/// Returns an error if `dist/` does not exist, the NickelMenu download fails,
/// or archive creation fails.
pub fn run(args: BundleArgs) -> Result<()> {
    let root = workspace::root()?;

    let dist_dir = root.join("dist");
    if !dist_dir.exists() {
        bail!("dist/ not found. Run `cargo xtask dist` first.");
    }

    let bundle_dir = root.join("bundle");
    if bundle_dir.exists() {
        std::fs::remove_dir_all(&bundle_dir).context("failed to remove existing bundle/")?;
    }

    if args.no_nickel {
        create_bundle_cadmus_only(&root, args.test)?;
    } else {
        let archive = ensure_nickel_menu(&root, args.skip_download)?;
        create_bundle_with_nickel(&root, &archive, args.test)?;
    }

    Ok(())
}

/// Returns the path to the NickelMenu archive, downloading it if necessary.
fn ensure_nickel_menu(root: &Path, skip_download: bool) -> Result<std::path::PathBuf> {
    let cache_dir = root.join(".cache/nickelmenu");
    let archive = cache_dir.join(format!("NickelMenu-{NICKEL_VERSION}-KoboRoot.tgz"));

    if archive.exists() {
        println!("Using cached NickelMenu v{NICKEL_VERSION}");
        return Ok(archive);
    }

    if skip_download {
        bail!(
            "NickelMenu archive not found at {}.\n\
             Remove --skip-download to auto-download.",
            archive.display()
        );
    }

    download_nickel_menu(&cache_dir, &archive)?;
    Ok(archive)
}

/// Downloads the NickelMenu release archive from GitHub with checksum verification.
fn download_nickel_menu(cache_dir: &Path, archive: &Path) -> Result<()> {
    std::fs::create_dir_all(cache_dir)?;

    println!("Downloading NickelMenu v{NICKEL_VERSION}…");

    let asset = github::fetch_release_asset(
        "pgaskin/NickelMenu",
        &format!("v{NICKEL_VERSION}"),
        "KoboRoot.tgz",
    )?;

    match asset.sha256() {
        Some(expected) => {
            http::download_verified(&asset.browser_download_url, archive, expected)?;
        }
        None => {
            http::download(&asset.browser_download_url, archive)
                .context("failed to download NickelMenu archive")?;
        }
    }

    println!("Downloaded NickelMenu to {}", archive.display());
    Ok(())
}

/// Creates a bundle containing only Cadmus (no NickelMenu).
fn create_bundle_cadmus_only(root: &Path, test: bool) -> Result<()> {
    let bundle_dir = root.join("bundle");
    let (adds_subdir, archive_name) = if test {
        ("cadmus-tst", "KoboRoot-test.tgz")
    } else {
        ("cadmus", "KoboRoot.tgz")
    };

    let install_dir = bundle_dir.join("mnt/onboard/.adds").join(adds_subdir);
    std::fs::create_dir_all(&install_dir)?;

    fs::copy_dir_all(&root.join("dist"), &install_dir)?;

    let archive = bundle_dir.join(archive_name);
    fs::create_tarball(&archive, &bundle_dir, &["mnt"])?;

    std::fs::remove_dir_all(bundle_dir.join("mnt"))?;

    println!("Bundle created: bundle/{archive_name}");
    println!("Place this file in the .kobo directory on your Kobo device");
    Ok(())
}

/// Creates a bundle that merges Cadmus with NickelMenu.
fn create_bundle_with_nickel(root: &Path, nickel_archive: &Path, test: bool) -> Result<()> {
    let bundle_dir = root.join("bundle");
    std::fs::create_dir_all(&bundle_dir)?;

    fs::extract_tarball(nickel_archive, &bundle_dir)?;

    let adds_src = bundle_dir.join("mnt/onboard/.adds");
    let adds_dst = bundle_dir.join(".adds");
    std::fs::rename(&adds_src, &adds_dst)?;
    std::fs::remove_dir_all(bundle_dir.join("mnt"))?;

    let (adds_subdir, nm_config, archive_name) = if test {
        ("cadmus-tst", "cadmus-tst", "KoboRoot-nm-test.tgz")
    } else {
        ("cadmus", "cadmus", "KoboRoot-nm.tgz")
    };

    fs::copy_dir_all(&root.join("dist"), &adds_dst.join(adds_subdir))?;

    std::fs::copy(
        root.join(format!("contrib/NickelMenu/{nm_config}")),
        adds_dst.join(format!("nm/{nm_config}")),
    )?;

    let final_adds = bundle_dir.join("mnt/onboard/.adds");
    std::fs::create_dir_all(bundle_dir.join("mnt/onboard"))?;
    std::fs::rename(&adds_dst, &final_adds)?;

    let archive = bundle_dir.join(archive_name);
    fs::create_tarball(&archive, &bundle_dir, &["usr", "mnt"])?;

    std::fs::remove_dir_all(bundle_dir.join("usr")).ok();
    std::fs::remove_dir_all(bundle_dir.join("mnt"))?;

    println!("Bundle created: bundle/{archive_name}");
    println!("Place this file in the .kobo directory on your Kobo device");
    Ok(())
}
