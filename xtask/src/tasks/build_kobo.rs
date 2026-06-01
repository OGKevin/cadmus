//! `cargo xtask build-kobo` — cross-compile Cadmus for Kobo devices.
//!
//! This task is a thin wrapper around `cargo build --release
//! --target arm-unknown-linux-gnueabihf -p cadmus`. All dependency
//! building (thirdparty libs, MuPDF, libwebp, mupdf_wrapper) is
//! handled automatically by `build.rs` when cargo build runs.
//!
//! Pre-flight steps performed before invoking cargo:
//!
//! 1. Verify the Linaro ARM toolchain (`arm-linux-gnueabihf-gcc`)
//!    is on `PATH`.
//!
//! Git submodules are not initialised up-front here: the Rust build
//! script clones them lazily, only when the cached Kobo build
//! artefacts in `libs/` and `target/cadmus-build-deps/...` are
//! missing. This keeps warm-cache CI runs fast by avoiding the
//! recursive submodule clone done by `actions/checkout`.
//!
//! The Kobo build is only available on Linux and macOS hosts.

use anyhow::{Result, bail};
use clap::Args;

use build_deps::versions::CROSS_ENV;

use super::util::{cmd, workspace};

/// Arguments for `cargo xtask build-kobo`.
#[derive(Debug, Args)]
pub struct BuildKoboArgs {
    /// Cargo feature flags to pass to the Cadmus build (e.g. `test`).
    #[arg(long)]
    pub features: Option<String>,
}

/// Cross-compiles Cadmus for Kobo ARM devices.
///
/// # Errors
///
/// Returns an error if:
/// - The host platform is not Linux or macOS.
/// - The Linaro ARM toolchain is not on `PATH`.
/// - The underlying `cargo build` invocation fails.
///
/// Git submodules are initialised lazily by the Rust build script
/// when the cached Kobo artefacts are missing; this task no longer
/// triggers a recursive submodule clone unconditionally.
pub fn run(args: BuildKoboArgs) -> Result<()> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        bail!(
            "Kobo cross-compilation is only available on Linux and macOS.\n\
             On other platforms, please use Docker or a Linux VM instead."
        );
    }

    let root = workspace::root()?;

    ensure_linaro_toolchain()?;

    cargo_build_kobo(&root, args.features.as_deref())?;

    Ok(())
}

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
    #[test]
    fn symlink_list_has_no_duplicates() {
        let mut link_names: Vec<&str> = build_deps::versions::SONAMES.to_vec();
        link_names.sort_unstable();
        let original_len = link_names.len();
        link_names.dedup();
        assert_eq!(link_names.len(), original_len, "duplicate link names found");
    }
}
