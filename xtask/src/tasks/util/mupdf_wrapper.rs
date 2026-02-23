//! MuPDF wrapper C library compilation helpers.
//!
//! Compiles `mupdf_wrapper.c` into a static library using the C compiler
//! directly.
//!
//! ## Output
//!
//! The compiled object and static library are placed in
//! `target/mupdf_wrapper/<target_os>/`:
//!
//! - `mupdf_wrapper.o`
//! - `libmupdf_wrapper.a`

use std::path::Path;

use anyhow::{Context, Result};

use super::cmd;

/// Compiles the mupdf_wrapper C library for the native platform only if the
/// output artifact does not already exist.
///
/// # Errors
///
/// Returns an error if compilation or archiving fails.
pub fn build_native_if_needed(root: &Path) -> Result<()> {
    let target_os = native_target_os();
    let lib = native_lib_path(root, target_os);

    if lib.exists() {
        println!("mupdf_wrapper already built for {target_os}.");
        return Ok(());
    }

    build(root, target_os, "cc", "ar", &[])
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

/// Compiles the mupdf_wrapper C library for the Kobo ARM target.
///
/// Uses the Linaro cross-compiler (`arm-linux-gnueabihf-gcc`) and
/// `arm-linux-gnueabihf-ar`.
///
/// # Errors
///
/// Returns an error if compilation or archiving fails.
pub fn build_kobo(root: &Path) -> Result<()> {
    build(
        root,
        "Kobo",
        "arm-linux-gnueabihf-gcc",
        "arm-linux-gnueabihf-ar",
        &[],
    )
}

/// Compiles `mupdf_wrapper.c` into `libmupdf_wrapper.a`.
///
/// Equivalent to the shell commands:
/// ```text
/// $CC -I../thirdparty/mupdf/include -c mupdf_wrapper.c -o $BUILD_DIR/mupdf_wrapper.o
/// $AR -rcs $BUILD_DIR/libmupdf_wrapper.a $BUILD_DIR/mupdf_wrapper.o
/// ```
///
/// # Errors
///
/// Returns an error if the compiler or archiver invocation fails.
fn build(root: &Path, target_os: &str, cc: &str, ar: &str, extra_cflags: &[&str]) -> Result<()> {
    let wrapper_dir = root.join("mupdf_wrapper");
    let build_dir = root.join(format!("target/mupdf_wrapper/{target_os}"));

    std::fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create {}", build_dir.display()))?;

    let include = root.join("thirdparty/mupdf/include");
    let obj = build_dir.join("mupdf_wrapper.o");
    let lib = build_dir.join("libmupdf_wrapper.a");

    let include_flag = format!("-I{}", include.display());
    let obj_str = obj.to_string_lossy().into_owned();

    let mut compile_args = vec![
        include_flag.as_str(),
        "-c",
        "mupdf_wrapper.c",
        "-o",
        &obj_str,
    ];

    for flag in extra_cflags {
        compile_args.push(flag);
    }

    cmd::run(cc, &compile_args, &wrapper_dir, &[])
        .with_context(|| format!("failed to compile mupdf_wrapper.c with {cc}"))?;

    let lib_str = lib.to_string_lossy().into_owned();
    let obj_str2 = obj.to_string_lossy().into_owned();

    cmd::run(ar, &["-rcs", &lib_str, &obj_str2], &wrapper_dir, &[])
        .with_context(|| format!("failed to archive libmupdf_wrapper.a with {ar}"))
}
