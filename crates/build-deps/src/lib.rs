//! Build helpers shared between the `cadmus` crate's `build.rs` and
//! the `xtask` binary.
//!
//! The crate is intentionally free of UI and runtime logic: it owns
//! the recipes and orchestration required to produce the third-party
//! C/C++ artifacts the rest of the workspace links against, namely
//! MuPDF, libwebp and the small set of compression and font libraries
//! that come with the Kobo build.
//!
//! Consumers should treat the public API in this crate as a stable
//! contract: a single public entry point per concern.
//!
//! # Native vs Kobo
//!
//! Two build flows are supported and they are intentionally kept
//! separate:
//!
//! * The **native** flow (Linux or macOS development hosts) uses
//!   system libraries through `pkg-config` and is exposed by
//!   [`build::native`]. The MuPDF source tree is copied into a
//!   per-target directory, the WebP support patches are applied, and
//!   that patched tree is the canonical source of truth used both for
//!   the compiled library and for the `mupdf_wrapper` C glue.
//! * The **Kobo** cross-build targets `arm-unknown-linux-gnueabihf`
//!   and is exposed by [`build::kobo`]. All libraries are built from
//!   source from the git submodules under `thirdparty/`, with
//!   per-library recipes in [`build::kobo::recipes`] and the final
//!   `libmupdf.so` produced by [`build::kobo::mupdf`].
//!
//! Both flows rely on the git submodules declared in `.gitmodules`.
//! [`ensure_submodules`] is the single entry point used to make sure
//! the working copy is ready before any other function is called.

pub mod build;
pub mod cargo_features;
pub mod cmd;
pub mod markers;
pub mod utils;
pub mod versions;

use std::path::Path;

use anyhow::{Context, Result};

/// Initialise git submodules. Required by both the native and Kobo
/// build flows because every thirdparty dependency lives in a
/// submodule under `thirdparty/`.
///
/// Returns an error when `.gitmodules` is missing (i.e. the working
/// copy was not produced by a recursive git clone) or when `git
/// submodule update --init --recursive` fails.
pub fn ensure_submodules(root: &Path) -> Result<()> {
    let gitmodules = root.join(".gitmodules");
    if !gitmodules.exists() {
        return Err(anyhow::anyhow!(
            ".gitmodules not found -- is this a git clone? Run: git clone --recursive"
        ));
    }

    println!("Initializing git submodules...");
    crate::cmd::run(
        "git",
        &["submodule", "update", "--init", "--recursive"],
        root,
        &[],
    )
    .context("failed to initialize git submodules")?;
    Ok(())
}
