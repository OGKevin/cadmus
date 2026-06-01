//! Build the `mupdf_wrapper` C glue library that exposes a small set
//! of MuPDF entry points to Rust.
//!
//! The wrapper has two variants: one for the native host (Linux or
//! macOS) and one cross-compiled for Kobo (ARM). Both are emitted into
//! `target/mupdf_wrapper/<platform>/` and statically linked into the
//! `cadmus` crate.
//!
//! The include path passed to the compiler must always point at the
//! MuPDF headers that match the source tree that was actually
//! compiled. For native builds that is the patched tree under
//! `target/cadmus-build-deps/<TARGET>/mupdf/include/`. For Kobo the
//! submodule headers at `thirdparty/mupdf/include/` are used as-is.
//!
//! The compilation step goes through the [`cc`] crate so the build
//! system handles dependency caching, parallel compilation, compiler
//! detection and archive creation in one place.

use std::path::Path;

use anyhow::{Context, Result};

/// Build `libmupdf_wrapper.a` for the native host if it does not
/// already exist.
///
/// `include` must point at the MuPDF `include/` directory whose
/// headers were used to build `libmupdf.a` (see
/// [`ensure_native_artifacts`][crate::build::native::ensure_native_artifacts]).
pub fn build_native_if_needed(root: &Path, include: &Path) -> Result<()> {
    let target_os = native_target_os();
    let lib = native_lib_path(root, target_os);

    if lib.exists() {
        println!("mupdf_wrapper already built for {target_os}.");
        return Ok(());
    }

    build(root, target_os, None, include, &[])
}

fn native_target_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    }
}

fn native_lib_path(root: &Path, target_os: &str) -> std::path::PathBuf {
    root.join(format!(
        "target/mupdf_wrapper/{target_os}/libmupdf_wrapper.a"
    ))
}

/// Build `libmupdf_wrapper.a` for the Kobo (ARM) target, reusing the
/// existing archive when present.
///
/// `include` must point at the patched MuPDF headers under
/// `target/cadmus-build-deps/<TARGET>/mupdf/include` so the wrapper
/// sees the WebP support patches applied by
/// [`crate::build::kobo::source::apply_patches`]. When the archive
/// is already on disk the function returns without recompiling, so
/// the caller only needs to make sure the patched headers are on
/// disk when a build is actually required.
pub fn build_kobo(root: &Path, include: &Path) -> Result<()> {
    let lib = kobo_lib_path(root);
    if lib.exists() {
        println!("mupdf_wrapper already built for Kobo.");
        return Ok(());
    }

    build(root, "Kobo", Some("arm-linux-gnueabihf-gcc"), include, &[])
}

fn kobo_lib_path(root: &Path) -> std::path::PathBuf {
    root.join("target/mupdf_wrapper/Kobo/libmupdf_wrapper.a")
}

fn build(
    root: &Path,
    target_os: &str,
    compiler: Option<&str>,
    include: &Path,
    extra_cflags: &[&str],
) -> Result<()> {
    let wrapper_dir = root.join("mupdf_wrapper");
    let build_dir = root.join(format!("target/mupdf_wrapper/{target_os}"));

    std::fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create {}", build_dir.display()))?;

    let mut build = cc::Build::new();
    build
        .file(wrapper_dir.join("mupdf_wrapper.c"))
        .include(include)
        .out_dir(&build_dir);

    for flag in extra_cflags {
        build.flag(flag);
    }

    if let Some(cc) = compiler {
        build.compiler(cc);
    }

    build
        .try_compile("mupdf_wrapper")
        .with_context(|| format!("failed to compile mupdf_wrapper.c for {target_os}"))?;

    // `cc::Build` leaves `mupdf_wrapper.o` next to the archive.
    // Remove it so the output directory matches the shape produced by
    // the previous shell-out implementation.
    let obj = build_dir.join("mupdf_wrapper.o");
    if obj.exists() {
        std::fs::remove_file(&obj).ok();
    }

    Ok(())
}
