//! `cargo xtask setup-native` — build MuPDF and the C wrapper for native dev.
//!
//! 1. Downloads MuPDF sources if the required version is not already present.
//! 2. Builds the `mupdf_wrapper` C library.
//! 3. Compiles MuPDF using system libraries.
//! 4. Creates symlinks in `target/mupdf_wrapper/<platform>/` so the Rust
//!    build script can find the static libraries.
//!
//! ## Required MuPDF version
//!
//! The version is pinned to [`REQUIRED_MUPDF_VERSION`].  If the sources
//! already present on disk match this version the download is skipped.

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Args;

use super::util::{cmd, mupdf_wrapper, thirdparty, workspace};

/// The MuPDF source version that must be present for a successful build.
pub const REQUIRED_MUPDF_VERSION: &str = "1.27.0";

/// Marker file written after a successful native MuPDF build.
const NATIVE_BUILT_MARKER: &str = ".built-native";

/// Arguments for `cargo xtask setup-native`.
#[derive(Debug, Args)]
pub struct SetupNativeArgs {
    /// Force a re-download of MuPDF sources even if the correct version is
    /// already present.
    #[arg(long)]
    pub force: bool,
}

/// Builds MuPDF and the C wrapper for native (non-cross-compiled) development.
///
/// # Errors
///
/// Returns an error if any build step fails or if required tools (`make`,
/// `pkg-config`, `ar`) are not available.
pub fn run(args: SetupNativeArgs) -> Result<()> {
    let root = workspace::root()?;

    ensure_mupdf_sources(&root, args.force)?;
    build_mupdf_wrapper_native_if_needed(&root)?;

    if native_mupdf_ready(&root) {
        println!("Native MuPDF build already present.");
    } else {
        build_mupdf_native(&root)?;
        write_native_build_marker(&root)?;
    }

    link_mupdf_artifacts(&root)?;

    println!("\nNative setup complete!");
    println!("You can now run:");
    println!("  cargo test          - Run tests");
    println!("  cargo xtask build-kobo  - Build for Kobo (Linux & macOS)");

    Ok(())
}

/// Ensures MuPDF sources at the required version are present in
/// `thirdparty/mupdf/`.
///
/// If the version header is missing or reports a different version the
/// existing directory is removed and the sources are re-downloaded.
pub fn ensure_mupdf_sources(root: &Path, force: bool) -> Result<()> {
    let version_header = root.join("thirdparty/mupdf/include/mupdf/fitz/version.h");

    let current_version = read_mupdf_version(&version_header);

    if !force && current_version.as_deref() == Some(REQUIRED_MUPDF_VERSION) {
        println!("MuPDF {REQUIRED_MUPDF_VERSION} already present.");
        return Ok(());
    }

    if let Some(v) = &current_version {
        println!("MuPDF version mismatch: have '{v}', need '{REQUIRED_MUPDF_VERSION}'");
    }

    println!("Downloading MuPDF {REQUIRED_MUPDF_VERSION} sources…");

    let mupdf_dir = root.join("thirdparty/mupdf");
    if mupdf_dir.exists() {
        thirdparty::clean_untracked(&mupdf_dir)
            .context("failed to clean untracked files from thirdparty/mupdf")?;
    }

    thirdparty::download_libraries(&root.join("thirdparty"), &["mupdf"])
}

/// Reads the MuPDF version string from the version header file.
///
/// Returns `None` if the file does not exist or the version cannot be parsed.
fn read_mupdf_version(header: &Path) -> Option<String> {
    let content = std::fs::read_to_string(header).ok()?;

    // The header contains a line like: #define FZ_VERSION "1.27.0"
    for line in content.lines() {
        if line.contains("FZ_VERSION") && line.contains('"') {
            let start = line.find('"')? + 1;
            let end = line.rfind('"')?;
            if start < end {
                return Some(line[start..end].to_owned());
            }
        }
    }

    None
}

/// Returns `true` when the full native setup is complete.
///
/// Checks that the build marker, the compiled `libmupdf.a`, the C wrapper
/// library `libmupdf_wrapper.a`, and both symlinks in
/// `target/mupdf_wrapper/<platform>/` are all present.
pub fn native_setup_done(root: &Path) -> bool {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };

    let wrapper_dir = root.join(format!("target/mupdf_wrapper/{platform_dir}"));

    native_mupdf_ready(root)
        && wrapper_dir.join("libmupdf.a").exists()
        && wrapper_dir.join("libmupdf_wrapper.a").exists()
}

/// Returns `true` when native MuPDF libraries are present and marked as built.
fn native_mupdf_ready(root: &Path) -> bool {
    let marker = root.join("thirdparty/mupdf").join(NATIVE_BUILT_MARKER);
    if !marker.exists() {
        return false;
    }

    let libmupdf = root.join("thirdparty/mupdf/build/release/libmupdf.a");

    libmupdf.exists()
}

/// Writes the native build marker in the MuPDF source directory.
fn write_native_build_marker(root: &Path) -> Result<()> {
    let marker = root.join("thirdparty/mupdf").join(NATIVE_BUILT_MARKER);
    std::fs::write(&marker, "").with_context(|| {
        format!(
            "failed to write native build marker at {}",
            marker.display()
        )
    })
}

/// Builds the `mupdf_wrapper` C static library for the native platform.
fn build_mupdf_wrapper_native_if_needed(root: &Path) -> Result<()> {
    println!("Ensuring mupdf_wrapper is available…");
    mupdf_wrapper::build_native_if_needed(root)
}

/// Compiles MuPDF using system libraries for the native platform.
fn build_mupdf_native(root: &Path) -> Result<()> {
    println!("Building MuPDF for native development…");

    let mupdf_dir = root.join("thirdparty/mupdf");

    // Remove git metadata that interferes with the MuPDF build system.
    for entry in ["gitattributes", ".gitattributes"] {
        let path = mupdf_dir.join(entry);
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
    }

    cmd::run("make", &["clean"], &mupdf_dir, &[]).ok();
    cmd::run("make", &["verbose=yes", "generate"], &mupdf_dir, &[])?;

    let sys_cflags = collect_system_cflags()?;
    let xcflags = format!(
        "-DFZ_ENABLE_ICC=0 -DFZ_ENABLE_SPOT_RENDERING=0 \
         -DFZ_ENABLE_ODT_OUTPUT=0 -DFZ_ENABLE_OCR_OUTPUT=0 {sys_cflags}"
    );

    cmd::run(
        "make",
        &[
            "verbose=yes",
            "mujs=no",
            "tesseract=no",
            "extract=no",
            "archive=no",
            "brotli=no",
            "barcode=no",
            "commercial=no",
            "USE_SYSTEM_LIBS=yes",
            &format!("XCFLAGS={xcflags}"),
            "libs",
        ],
        &mupdf_dir,
        &[],
    )
}

/// Collects system library CFLAGS via `pkg-config` on macOS.
///
/// On Linux, MuPDF's build system detects system libraries automatically.
/// On macOS it needs explicit CFLAGS gathered from pkg-config.
fn collect_system_cflags() -> Result<String> {
    if !cfg!(target_os = "macos") {
        return Ok(String::new());
    }

    let libs = [
        "freetype2",
        "harfbuzz",
        "libopenjp2",
        "libjpeg",
        "zlib",
        "jbig2dec",
        "gumbo",
    ];

    let mut flags = String::new();
    for lib in libs {
        if let Ok(f) = cmd::output("pkg-config", &["--cflags", lib], Path::new("."), &[]) {
            if !f.is_empty() {
                flags.push(' ');
                flags.push_str(&f);
            }
        }
    }

    Ok(flags.trim().to_owned())
}

/// Creates symlinks in `target/mupdf_wrapper/<platform>/` pointing to the
/// compiled MuPDF static libraries.
fn link_mupdf_artifacts(root: &Path) -> Result<()> {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };

    let target_dir = root.join(format!("target/mupdf_wrapper/{platform_dir}"));
    std::fs::create_dir_all(&target_dir)
        .context("failed to create target/mupdf_wrapper directory")?;

    let release_dir = root.join("thirdparty/mupdf/build/release");

    let libmupdf = release_dir.join("libmupdf.a");
    if !libmupdf.exists() {
        bail!("libmupdf.a not found after build — check MuPDF build output");
    }

    symlink_force(&libmupdf, &target_dir.join("libmupdf.a"))?;
    println!("✓ Created libmupdf.a in target/mupdf_wrapper/{platform_dir}");

    let libmupdf_third = release_dir.join("libmupdf-third.a");
    if !libmupdf_third.exists() {
        println!("Creating empty libmupdf-third.a (system libs used instead)…");
        cmd::run("ar", &["cr", "libmupdf-third.a"], &release_dir, &[])?;
    }

    symlink_force(&libmupdf_third, &target_dir.join("libmupdf-third.a"))?;
    println!("✓ Created libmupdf-third.a");

    Ok(())
}

/// Creates a symlink at `link` pointing to `target`, removing any existing
/// file or symlink at `link` first.
fn symlink_force(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)
            .with_context(|| format!("failed to remove existing {}", link.display()))?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;

    #[cfg(not(unix))]
    std::fs::copy(target, link)
        .with_context(|| format!("failed to copy {} to {}", target.display(), link.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_mupdf_version_parses_define() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("version.h");
        fs::write(
            &header,
            r#"/* MuPDF version */
#define FZ_VERSION "1.27.0"
#define FZ_VERSION_MAJOR 1
"#,
        )
        .unwrap();

        let version = read_mupdf_version(&header);
        assert_eq!(version.as_deref(), Some("1.27.0"));
    }

    #[test]
    fn read_mupdf_version_returns_none_for_malformed_header() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("version.h");
        fs::write(&header, "/* no version define here */\n").unwrap();

        let version = read_mupdf_version(&header);
        assert!(version.is_none());
    }
}
