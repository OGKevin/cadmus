//! `cargo xtask build-kobo` — cross-compile Cadmus for Kobo devices.
//!
//! 1. Optionally downloads and builds all thirdparty libraries from source
//!    (`--slow` mode, required for CI).
//! 2. Builds the `mupdf_wrapper` C library for the Kobo ARM target.
//! 3. Runs `cargo build --release --target arm-unknown-linux-gnueabihf`.
//!
//! ## Platform requirement
//!
//! Cross-compilation requires the Linaro ARM toolchain
//! (`arm-linux-gnueabihf-gcc` and friends) which is only available on Linux.
//! The task exits with a clear error on macOS.
//!
//! ## Build modes
//!
//! | Mode | Description |
//! |------|-------------|
//! | fast (default) | Downloads pre-built `.so` files and MuPDF sources |
//! | slow | Builds all thirdparty libraries from source |
//! | slow + download-only | Downloads all thirdparty sources without building |
//! | skip | Assumes `libs/` already exists; skips download entirely |

use anyhow::{Context, Result, bail};
use clap::Args;

use super::util::{cmd, fs, github, http, mupdf_wrapper, thirdparty, workspace};

const SYMLINKS: &[(&str, &str)] = &[
    ("libz.so.1", "libz.so"),
    ("libbz2.so.1.0", "libbz2.so"),
    ("libpng16.so.16", "libpng16.so"),
    ("libjpeg.so.9", "libjpeg.so"),
    ("libopenjp2.so.7", "libopenjp2.so"),
    ("libjbig2dec.so.0", "libjbig2dec.so"),
    ("libfreetype.so.6", "libfreetype.so"),
    ("libharfbuzz.so.0", "libharfbuzz.so"),
    ("libgumbo.so.1", "libgumbo.so"),
    ("libdjvulibre.so.21", "libdjvulibre.so"),
];

const BUILT_LIBRARY_COPIES: &[(&str, &str)] = &[
    ("thirdparty/zlib/libz.so", "libz.so"),
    ("thirdparty/bzip2/libbz2.so", "libbz2.so"),
    ("thirdparty/libpng/.libs/libpng16.so", "libpng16.so"),
    ("thirdparty/libjpeg/.libs/libjpeg.so", "libjpeg.so"),
    (
        "thirdparty/openjpeg/build/bin/libopenjp2.so",
        "libopenjp2.so",
    ),
    ("thirdparty/jbig2dec/.libs/libjbig2dec.so", "libjbig2dec.so"),
    (
        "thirdparty/freetype2/objs/.libs/libfreetype.so",
        "libfreetype.so",
    ),
    (
        "thirdparty/harfbuzz/build/src/libharfbuzz.so",
        "libharfbuzz.so",
    ),
    ("thirdparty/gumbo/.libs/libgumbo.so", "libgumbo.so"),
    (
        "thirdparty/djvulibre/libdjvu/.libs/libdjvulibre.so",
        "libdjvulibre.so",
    ),
    ("thirdparty/mupdf/build/release/libmupdf.so", "libmupdf.so"),
];

const CROSS_ENV: &[(&str, &str)] = &[
    ("CC", "arm-linux-gnueabihf-gcc"),
    ("CXX", "arm-linux-gnueabihf-g++"),
    ("AR", "arm-linux-gnueabihf-ar"),
    ("LD", "arm-linux-gnueabihf-ld"),
    ("RANLIB", "arm-linux-gnueabihf-ranlib"),
    ("STRIP", "arm-linux-gnueabihf-strip"),
    ("PKG_CONFIG_ALLOW_CROSS", "1"),
    (
        "CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABIHF_LINKER",
        "arm-linux-gnueabihf-gcc",
    ),
    ("CC_arm_unknown_linux_gnueabihf", "arm-linux-gnueabihf-gcc"),
    ("AR_arm_unknown_linux_gnueabihf", "arm-linux-gnueabihf-ar"),
];

/// Arguments for `cargo xtask build-kobo`.
#[derive(Debug, Args)]
pub struct BuildKoboArgs {
    /// Build all thirdparty libraries from source instead of downloading
    /// pre-built binaries.
    ///
    /// Required for CI where pre-built binaries are not available.
    #[arg(long)]
    pub slow: bool,

    /// Skip the library download/build step entirely.
    ///
    /// Use this when `libs/` already contains the required `.so` files.
    #[arg(long)]
    pub skip: bool,

    /// Download thirdparty sources without building or cross-compiling.
    ///
    /// Useful for pre-populating the source cache in CI setup steps.
    #[arg(long)]
    pub download_only: bool,

    /// Cargo feature flags to pass to the Cadmus build (e.g. `test`).
    #[arg(long)]
    pub features: Option<String>,
}

/// Cross-compiles Cadmus for Kobo ARM devices.
///
/// # Errors
///
/// Returns an error if:
/// - The platform is not Linux.
/// - The Linaro toolchain is not on `PATH`.
/// - Any build step fails.
pub fn run(args: BuildKoboArgs) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!(
            "Kobo cross-compilation is only available on Linux.\n\
             The Linaro ARM toolchain consists of x86_64 Linux ELF binaries \
             that cannot run on macOS. Use Docker or a Linux VM instead."
        );
    }

    let root = workspace::root()?;

    ensure_linaro_toolchain()?;

    match (args.slow, args.skip, args.download_only) {
        (_, true, _) => {
            println!("Skipping library download (--skip).");
        }
        (true, false, true) => {
            println!("Downloading thirdparty sources (--slow --download-only)…");
            thirdparty::download_libraries(&root.join("thirdparty"), &[])?;
            return Ok(());
        }
        (true, false, false) => {
            println!("Building thirdparty libraries from source (--slow)…");
            build_thirdparty_slow(&root)?;
        }
        (false, false, true) => {
            println!("Downloading MuPDF sources (--download-only)…");
            thirdparty::download_libraries(&root.join("thirdparty"), &["mupdf"])?;
            return Ok(());
        }
        (false, false, false) => {
            println!("Downloading pre-built libraries (fast mode)…");
            build_thirdparty_fast(&root)?;
        }
    }

    build_mupdf_wrapper_kobo(&root)?;
    cargo_build_kobo(&root, args.features.as_deref())?;

    Ok(())
}

/// Verifies that the Linaro ARM cross-compiler is available on `PATH`.
fn ensure_linaro_toolchain() -> Result<()> {
    cmd::run(
        "arm-linux-gnueabihf-gcc",
        &["--version"],
        std::path::Path::new("."),
        &[],
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "arm-linux-gnueabihf-gcc not found on PATH.\n\
             Install the Linaro toolchain or run inside the devenv shell."
        )
    })
}

/// Downloads pre-built `.so` files and MuPDF sources (fast mode).
fn build_thirdparty_fast(root: &std::path::Path) -> Result<()> {
    download_release_libs(root)?;

    let libs_dir = root.join("libs");
    create_symlinks(&libs_dir)?;

    thirdparty::download_libraries(&root.join("thirdparty"), &["mupdf"])
}

/// Builds all thirdparty libraries from source (slow mode).
fn build_thirdparty_slow(root: &std::path::Path) -> Result<()> {
    let thirdparty_dir = root.join("thirdparty");

    thirdparty::download_libraries(&thirdparty_dir, &[])?;
    thirdparty::build_libraries(&thirdparty_dir, &[])?;

    let libs_dir = root.join("libs");
    std::fs::create_dir_all(&libs_dir)?;

    copy_built_libs(root, &libs_dir)
}

/// Creates the `.so` version symlinks expected by the Cadmus runtime.
fn create_symlinks(libs_dir: &std::path::Path) -> Result<()> {
    for (target, link_name) in SYMLINKS {
        let link_path = libs_dir.join(link_name);
        if !link_path.exists() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &link_path)?;
        }
    }

    Ok(())
}

/// Copies the `.so` files produced by the slow build into `libs/`.
fn copy_built_libs(root: &std::path::Path, libs_dir: &std::path::Path) -> Result<()> {
    for (src_rel, dest_name) in BUILT_LIBRARY_COPIES {
        let src = root.join(src_rel);
        let dest = libs_dir.join(dest_name);
        std::fs::copy(&src, &dest).map_err(|e| {
            anyhow::anyhow!("failed to copy {} → {}: {e}", src.display(), dest.display())
        })?;
    }

    Ok(())
}

/// Downloads pre-built `.so` release assets from the cadmus GitHub release
/// with checksum verification.
fn download_release_libs(root: &std::path::Path) -> Result<()> {
    let version = workspace::current_version()?;
    let tag = format!("v{version}");
    let archive_name = "cadmus-kobo.tar.gz";

    let libs_dir = root.join("libs");
    if libs_dir.exists() {
        println!("libs/ directory already exists; skipping download of pre-built libraries.");
        return Ok(());
    }

    std::fs::create_dir_all(&libs_dir)?;

    let asset = github::fetch_release_asset("ogkevin/cadmus", &tag, archive_name)?;
    let archive = root.join(archive_name);

    match asset.sha256() {
        Some(expected) => {
            http::download_verified(&asset.browser_download_url, &archive, expected)?;
        }
        None => {
            http::download(&asset.browser_download_url, &archive)
                .with_context(|| format!("failed to download {archive_name}"))?;
        }
    }

    fs::extract_tarball_paths(&archive, root, &["libs"])?;
    std::fs::remove_file(&archive).ok();

    Ok(())
}

/// Builds the `mupdf_wrapper` C library for the Kobo ARM target.
fn build_mupdf_wrapper_kobo(root: &std::path::Path) -> Result<()> {
    println!("Building mupdf_wrapper for Kobo…");
    mupdf_wrapper::build_kobo(root)
}

/// Runs `cargo build --release` for the ARM Kobo target.
fn cargo_build_kobo(root: &std::path::Path, features: Option<&str>) -> Result<()> {
    let mut cargo_args = vec![
        "build",
        "--release",
        "--target",
        "arm-unknown-linux-gnueabihf",
        "-p",
        "cadmus",
    ];

    if let Some(f) = features {
        cargo_args.push("--features");
        cargo_args.push(f);
    }

    cmd::run("cargo", &cargo_args, root, CROSS_ENV)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_list_has_no_duplicates() {
        let mut link_names: Vec<&str> = SYMLINKS.iter().map(|(_, link)| *link).collect();
        link_names.sort_unstable();
        let original_len = link_names.len();
        link_names.dedup();
        assert_eq!(link_names.len(), original_len, "duplicate link names found");
    }
}
