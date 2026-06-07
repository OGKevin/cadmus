//! Cross-compilation helpers for the Kobo (ARM) target.
//!
//! The flow has three layers, each in its own submodule:
//!
//! * [`source`] prepares a clean, patched copy of each thirdparty
//!   library's source tree under
//!   `target/cadmus-build-deps/<TARGET>/<lib>/`.
//! * [`recipes`] builds the individual libraries (configure / make /
//!   meson / cmake invocations tailored to each upstream build
//!   system).
//! * [`mupdf`] links the per-library outputs into the final
//!   `libmupdf.so` shared object loaded by the Kobo runtime.
//!
//! The top-level entry points ([`build_libraries`], [`copy_built_libs`],
//! [`create_symlinks`]) compose these layers. Callers that just want
//! "the Kobo target is ready to link" should use the single
//! [`ensure_kobo_artifacts`] helper, which owns the full flow
//! (submodule init, library build/copy/symlink, and the
//! `mupdf_wrapper` archive).

pub(crate) mod mupdf;
pub(crate) mod recipes;
pub(crate) mod source;

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cmd;
use crate::markers;
use crate::versions::{self, SONAMES};

/// Default Cargo target triple when `TARGET` is unset.
fn target_triple() -> String {
    std::env::var("TARGET").unwrap_or_else(|_| "arm-unknown-linux-gnueabihf".to_string())
}

/// Build-root directory used for Kobo cross-builds.
pub fn build_root(root: &Path) -> std::path::PathBuf {
    root.join("target/cadmus-build-deps").join(target_triple())
}

/// Build every thirdparty library in [`LIBRARY_NAMES`][versions::LIBRARY_NAMES]
/// from source, in dependency order.
///
/// Each library is copied from its submodule into the per-target build
/// directory, patched (see [`source::copy_source`]) and built with the
/// toolchain-appropriate recipe from [`recipes`]. A `.built` marker is
/// written on success so re-runs are idempotent.
///
/// # Errors
///
/// Returns an error if a submodule is missing, the source cannot be
/// copied or patched, or any of the underlying build commands fail.
fn build_libraries(thirdparty_dir: &Path) -> Result<()> {
    let targets = versions::LIBRARY_NAMES.to_vec();

    let root = thirdparty_dir
        .parent()
        .context("thirdparty dir has no parent")?;
    let build_root = build_root(root);
    std::fs::create_dir_all(&build_root)?;

    for name in targets {
        let src_dir = thirdparty_dir.join(name);
        if !src_dir.exists() {
            anyhow::bail!(
                "thirdparty/{name} not found - run `git submodule update --init --recursive` first"
            );
        }

        let build_dir = build_root.join(name);
        let submodule_path = format!("thirdparty/{name}");
        if markers::is_built(root, &build_dir, &submodule_path) {
            println!("Skipping {name} (already built)...");
            continue;
        }

        if build_dir.exists() {
            std::fs::remove_dir_all(&build_dir)
                .with_context(|| format!("failed to remove old build dir for {name}"))?;
        }

        source::copy_source(&src_dir, &build_dir, name, root)?;
        source::apply_patches(&build_dir, name, root)?;
        recipes::build_library(name, &build_dir)?;

        markers::mark_built(root, &build_dir, name, &submodule_path)?;
    }

    Ok(())
}

/// Copy the per-library outputs produced by [`build_libraries`] into
/// `libs/`, naming each file according to
/// [`BUILT_LIBRARY_COPIES`][versions::BUILT_LIBRARY_COPIES].
///
/// Source paths in the constant use the `thirdparty/<lib>/...` form
/// and are rewritten against the per-target build root.
pub(crate) fn copy_built_libs(root: &Path, libs_dir: &Path) -> Result<()> {
    let build_root = build_root(root);

    for &(src_rel, dest_name) in versions::BUILT_LIBRARY_COPIES {
        let rel = src_rel.strip_prefix("thirdparty/").unwrap_or(src_rel);
        let src = build_root.join(rel);
        let dest = libs_dir.join(dest_name);
        std::fs::copy(&src, &dest).map_err(|e| {
            anyhow::anyhow!(
                "failed to copy {} to {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
    Ok(())
}

/// Build every artefact the Kobo (ARM) target needs — the thirdparty
/// shared libraries staged under `libs/` and the
/// `target/mupdf_wrapper/Kobo/libmupdf_wrapper.a` C glue archive — if
/// it's not already cached.
///
/// On a warm cache (every [`SONAMES`] entry present and the wrapper
/// archive on disk) this returns `Ok(())` without touching git or the
/// cross toolchain. Otherwise it initialises submodules, builds the
/// libraries, copies/symlinks them, and finally compiles the
/// `mupdf_wrapper` archive against the Kobo MuPDF headers.
///
/// # Errors
///
/// Returns an error if submodules cannot be initialised, `thirdparty/`
/// is missing, any of the library build steps fail, or the wrapper
/// compile fails.
pub fn ensure_kobo_artifacts(root: &Path) -> Result<()> {
    if kobo_artifacts_present(root) {
        return Ok(());
    }

    crate::ensure_submodules(root).context("failed to initialize git submodules")?;

    let thirdparty_dir = root.join("thirdparty");
    if !thirdparty_dir.exists() {
        bail!("thirdparty/ directory not found. Run: git submodule update --init --recursive");
    }

    let libs_dir = root.join("libs");
    build_libraries(&thirdparty_dir).context("failed to build thirdparty libraries")?;
    std::fs::create_dir_all(&libs_dir).context("failed to create libs/ directory")?;
    copy_built_libs(root, &libs_dir).context("failed to copy built libraries")?;
    create_symlinks(&libs_dir).context("failed to create symlinks")?;

    let include = build_root(root).join("mupdf/include");
    crate::build::mupdf_wrapper::build_kobo(root, &include)
        .context("failed to build mupdf_wrapper")?;

    Ok(())
}

/// Returns `true` when the Kobo artefact cache is complete:
///
/// * every [`SONAMES`] entry is present in `libs/`,
/// * the `mupdf_wrapper` archive has been built for the Kobo target, and
/// * every per-library `.built` marker under
///   `target/cadmus-build-deps/<TARGET>/` matches the current submodule
///   gitlink SHA.
fn kobo_artifacts_present(root: &Path) -> bool {
    let libs_dir = root.join("libs");
    if !libs_dir.exists() {
        return false;
    }
    if !SONAMES.iter().all(|lib| libs_dir.join(lib).exists()) {
        return false;
    }
    if !root
        .join("target/mupdf_wrapper/Kobo/libmupdf_wrapper.a")
        .exists()
    {
        return false;
    }

    let kobo_build_root = build_root(root);
    versions::LIBRARY_NAMES.iter().all(|name| {
        let build_dir = kobo_build_root.join(name);
        let submodule_path = format!("thirdparty/{name}");
        markers::is_built(root, &build_dir, &submodule_path)
    })
}

/// Create the `.so` → versioned-SONAME symlinks the Cadmus runtime
/// expects inside `libs/`. The actual SONAME of each `.so` is
/// discovered with `arm-linux-gnueabihf-readelf`; the symlink name is
/// the base name from [`SONAMES`].
fn create_symlinks(libs_dir: &Path) -> Result<()> {
    for &lib in SONAMES {
        let target = soname(libs_dir, lib)?;
        let link_path = libs_dir.join(lib);
        if !link_path.exists() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link_path)?;
        }
    }
    Ok(())
}

/// Resolve the SONAME of `lib` inside `libs_dir`.
///
/// If `lib` already exists at its base name, the SONAME is read out
/// of its ELF dynamic section with `arm-linux-gnueabihf-readelf`.
/// Otherwise, the directory is scanned for a single versioned file
/// starting with `<lib>.` and that filename is returned.
pub fn soname(libs_dir: &Path, lib: &str) -> Result<String> {
    if libs_dir.join(lib).exists() {
        read_elf_soname(libs_dir, lib)
    } else {
        find_versioned_soname(libs_dir, lib)
    }
}

/// Read the SONAME of `lib` from its ELF dynamic section using
/// `arm-linux-gnueabihf-readelf`. The SONAME is the last
/// bracketed token on the `SONAME` line of the dynamic section.
fn read_elf_soname(libs_dir: &Path, lib: &str) -> Result<String> {
    let so_path = libs_dir.join(lib);
    let so_path_str = so_path
        .to_str()
        .with_context(|| format!("shared library path is not valid UTF-8: {so_path:?}"))?;
    let output = cmd::output(
        "arm-linux-gnueabihf-readelf",
        &["-d", so_path_str],
        libs_dir,
        &[],
    )?;
    output
        .lines()
        .find(|line| line.contains("SONAME"))
        .and_then(|line| line.split_whitespace().last())
        .map(|token| {
            token
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string()
        })
        .with_context(|| format!("failed to find SONAME in readelf output for {lib}"))
}

/// Locate the SONAME of `lib` by scanning `libs_dir` for a single
/// versioned file whose name starts with `<lib>.`.
///
/// # Errors
///
/// Returns an error if zero or more than one matching file is found.
fn find_versioned_soname(libs_dir: &Path, lib: &str) -> Result<String> {
    let prefix = format!("{lib}.");
    let matching: Vec<_> = std::fs::read_dir(libs_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .collect();

    match matching.len() {
        1 => Ok(matching[0].file_name().to_string_lossy().into_owned()),
        0 => anyhow::bail!(
            "no versioned file found for {lib} in {}",
            libs_dir.display()
        ),
        _ => anyhow::bail!(
            "multiple versioned files found for {lib} in {}",
            libs_dir.display()
        ),
    }
}

/// Recursive copy that mirrors a source tree to `dst`, skipping git
/// metadata (`*.git`, `*.gitattributes`), build artefacts (`build/`,
/// `objs/`) and `autom4te.cache/`.
///
/// Symlinks are preserved as symlinks; regular files and directories
/// are copied recursively. Used by [`source::copy_source`] and by
/// the native build to snapshot the MuPDF source tree before
/// patching.
pub fn cp_r(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(".git")
            || name_str == "build"
            || name_str == "objs"
            || name_str == "autom4te.cache"
        {
            continue;
        }
        let ft = entry.file_type()?;
        let dst_child = dst.join(&name);
        if ft.is_dir() {
            cp_r(&entry.path(), &dst_child)?;
        } else if ft.is_symlink() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &dst_child)?;
            }
        } else {
            std::fs::copy(entry.path(), &dst_child)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_list_has_no_duplicates() {
        let mut link_names: Vec<&str> = SONAMES.to_vec();
        link_names.sort_unstable();
        let original_len = link_names.len();
        link_names.dedup();
        assert_eq!(link_names.len(), original_len, "duplicate link names found");
    }

    #[test]
    fn soname_finds_single_versioned_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("libfoo.so.1.2.3"), b"").unwrap();

        let resolved = soname(tmp.path(), "libfoo.so").unwrap();
        assert_eq!(resolved, "libfoo.so.1.2.3");
    }

    #[test]
    fn soname_errors_when_no_versioned_file_present() {
        let tmp = tempfile::tempdir().unwrap();

        let err = soname(tmp.path(), "libfoo.so").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("no versioned file found for libfoo.so"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn soname_errors_when_multiple_versioned_files_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("libfoo.so.1"), b"").unwrap();
        std::fs::write(tmp.path().join("libfoo.so.2"), b"").unwrap();

        let err = soname(tmp.path(), "libfoo.so").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("multiple versioned files found for libfoo.so"),
            "unexpected error: {msg}"
        );
    }
}
