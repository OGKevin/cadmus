//! Build MuPDF into the final `libmupdf.so` shared object that the
//! Kobo runtime links against.
//!
//! After the per-library recipes have produced their shared objects in
//! sibling directories (zlib, freetype2, harfbuzz, …) this module
//! collects every `*.o` produced by `make libs` inside MuPDF and
//! relinks them into a single `libmupdf.so` that depends on the
//! sibling libraries.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cmd;

/// Build `libmupdf.so` from the patched MuPDF source tree under
/// `build_dir`, linking against the shared libraries produced by the
/// other Kobo recipes.
pub fn build_mupdf(build_dir: &Path) -> Result<()> {
    clean_git_metadata(build_dir)?;
    clean_old_build(build_dir)?;

    cmd::run("make", &["verbose=yes", "generate"], build_dir, &[])
        .context("failed to run make generate for mupdf")?;

    let xcflags = format!("-I../libwebp/src {}", crate::build::mupdf::XCFLAGS_SHARED);
    let make_args =
        crate::build::mupdf::make_libs_invocation(&xcflags, &["OS=kobo", "build=release"], None);
    let make_refs: Vec<&str> = make_args.iter().map(String::as_str).collect();
    cmd::run("make", &make_refs, build_dir, &[]).context("failed to build mupdf libs")?;

    let obj_files = collect_object_files(build_dir)?;
    let libmupdf_so = build_dir.join("build/release/libmupdf.so");
    let libmupdf_so_str = libmupdf_so
        .to_str()
        .context("libmupdf.so path is not valid UTF-8")?;

    let mut link_args = vec!["-Wl,--gc-sections", "-o", libmupdf_so_str];
    link_args.extend(obj_files.iter().map(|s| s.as_str()));
    link_args.extend_from_slice(&[
        "-lm",
        "-L../freetype2/objs/.libs",
        "-lfreetype",
        "-L../harfbuzz/build/src",
        "-lharfbuzz",
        "-L../gumbo/.libs",
        "-lgumbo",
        "-L../jbig2dec/.libs",
        "-ljbig2dec",
        "-L../libjpeg/.libs",
        "-ljpeg",
        "-L../openjpeg/build/bin",
        "-lopenjp2",
        "-L../zlib",
        "-lz",
        "-L../libwebp/src/.libs",
        "-lwebp",
        "-L../libwebp/src/demux/.libs",
        "-lwebpdemux",
        "-shared",
        "-Wl,-soname",
        "-Wl,libmupdf.so",
        "-Wl,--no-undefined",
    ]);

    cmd::run("arm-linux-gnueabihf-gcc", &link_args, build_dir, &[])
        .context("failed to link libmupdf.so")
}

/// Strip `.git*` entries from `build_dir`. MuPDF's makefiles
/// misinterpret git attributes when present, so they are removed
/// before the build starts.
fn clean_git_metadata(build_dir: &Path) -> Result<()> {
    let gitattributes = build_dir.join(".gitattributes");
    if !gitattributes.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(build_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".git") {
            if entry.path().is_dir() {
                std::fs::remove_dir_all(entry.path()).with_context(|| {
                    format!("failed to remove directory {}", entry.path().display())
                })?;
            } else {
                std::fs::remove_file(entry.path())
                    .with_context(|| format!("failed to remove file {}", entry.path().display()))?;
            }
        }
    }
    Ok(())
}

/// Remove any pre-existing `build/` directory created by a previous
/// `make` invocation.
fn clean_old_build(build_dir: &Path) -> Result<()> {
    let mupdf_build = build_dir.join("build");
    if mupdf_build.exists() {
        std::fs::remove_dir_all(&mupdf_build)?;
    }
    Ok(())
}

/// Walk `build_dir/build/release/` and return every `*.o` path except
/// the CJK font and lcms sub-archives that MuPDF bundles by default
/// but that Cadmus does not need at runtime.
fn collect_object_files(build_dir: &Path) -> Result<Vec<String>> {
    let release_dir = build_dir.join("build/release");
    let excluded_substrings = [
        "SourceHanSerif-Regular",
        "DroidSansFallbackFull",
        "NotoSerifTangut",
        "color-lcms",
    ];

    let mut objects = Vec::new();
    for entry in walk_release(&release_dir)? {
        let path = entry?;
        if path.extension().is_none_or(|e| e != "o") {
            continue;
        }
        let path_str = path.to_string_lossy();
        if excluded_substrings
            .iter()
            .any(|needle| path_str.contains(needle))
        {
            continue;
        }
        objects.push(path_str.into_owned());
    }
    Ok(objects)
}

fn walk_release(dir: &Path) -> Result<Vec<Result<PathBuf, std::io::Error>>> {
    let mut out = Vec::new();
    walk_recursive(dir, &mut out)?;
    Ok(out)
}

fn walk_recursive(dir: &Path, out: &mut Vec<Result<PathBuf, std::io::Error>>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_recursive(&path, out)?;
        } else {
            out.push(Ok(path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_object_files_skips_excluded_substrings() {
        let tmp = tempfile::tempdir().unwrap();
        let release = tmp.path().join("build/release");
        std::fs::create_dir_all(&release).unwrap();

        for name in [
            "extract.o",
            "SourceHanSerif-Regular.o",
            "DroidSansFallbackFull.o",
            "NotoSerifTangut.o",
            "color-lcms.o",
            "fz-image.o",
        ] {
            std::fs::write(release.join(name), b"").unwrap();
        }

        let objects = collect_object_files(tmp.path()).unwrap();
        let names: Vec<_> = objects
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert!(names.contains(&"extract.o".to_string()));
        assert!(names.contains(&"fz-image.o".to_string()));
        for excluded in [
            "SourceHanSerif-Regular.o",
            "DroidSansFallbackFull.o",
            "NotoSerifTangut.o",
            "color-lcms.o",
        ] {
            assert!(
                !names.iter().any(|n| n == excluded),
                "{excluded} should have been excluded"
            );
        }
    }
}
