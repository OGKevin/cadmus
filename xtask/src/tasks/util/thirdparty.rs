//! Thirdparty library download and build helpers.
//!
//! Library source URLs are defined as constants so Renovate can track them.
//!
//! ## Download
//!
//! [`download_libraries`] fetches each library's source.  Most libraries are
//! downloaded as tarballs and extracted with the top-level directory stripped.
//! Libraries that use git submodules (currently freetype2) are cloned with
//! `--recurse-submodules` so submodule contents are always present.  If the
//! cloned source ships an `autogen.sh` script, it is run immediately after
//! cloning to generate the `configure` script that `build-kobo.sh` expects.
//!
//! ## Build
//!
//! [`build_libraries`] iterates over the packages in dependency order, applies
//! `kobo.patch` if present, then invokes each library's own `build-kobo.sh`
//! script.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{cmd, fs, http};

/// Base names of all thirdparty shared libraries.
///
/// SONAMEs are discovered at runtime via `readelf -d` because upstream
/// libraries do not follow a consistent ABI versioning scheme.
pub const SONAMES: &[&str] = &[
    "libz.so",
    "libbz2.so",
    "libpng16.so",
    "libjpeg.so",
    "libopenjp2.so",
    "libjbig2dec.so",
    "libfreetype.so",
    "libharfbuzz.so",
    "libgumbo.so",
    "libdjvulibre.so",
    "libmupdf.so",
];

/// Returns the SONAME of `lib` in `libs_dir`.
///
/// When the library file exists, `readelf -d` is used to extract the SONAME
/// from the binary. When only a versioned file exists (e.g. `libz.so.1.2.13`
/// without `libz.so`), the versioned filename is returned directly.
///
/// # Errors
///
/// Returns an error if `readelf` fails or the SONAME cannot be determined.
pub fn soname(libs_dir: &Path, lib: &str) -> Result<String> {
    let so_path = libs_dir.join(lib);
    if so_path.exists() {
        let so_path_str = so_path
            .to_str()
            .with_context(|| format!("shared library path is not valid UTF-8: {so_path:?}"))?;
        let output = cmd::output("readelf", &["-d", so_path_str], libs_dir, &[])?;
        let soname = output
            .lines()
            .find(|line| line.contains("SONAME"))
            .and_then(|line| line.split_whitespace().last())
            .map(|token| {
                token
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            })
            .with_context(|| format!("failed to find SONAME in readelf output for {lib}"))?;
        Ok(soname)
    } else {
        let prefix = format!("{}.", lib);
        let matching: Vec<_> = std::fs::read_dir(libs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .collect();

        match matching.len() {
            1 => Ok(matching[0].file_name().to_string_lossy().into_owned()),
            0 => bail!(
                "no versioned file found for {} in {}",
                lib,
                libs_dir.display()
            ),
            _ => bail!(
                "multiple versioned files found for {} in {}",
                lib,
                libs_dir.display()
            ),
        }
    }
}

/// Version strings for thirdparty libraries tracked by Renovate.
///
/// Each version constant is the single source of truth — the download URL is
/// derived from it at call time in [`library_source`].  A Renovate regex manager
/// in `renovate.json` matches these constants and opens PRs when new releases
/// are available.
///
/// # TODO
///
/// Add Renovate regex managers for the remaining URL constants below so that
/// all thirdparty dependency updates are tracked automatically.  The
/// following are now tracked via VERSION constants: openjpeg, harfbuzz, gumbo.
/// Their SONAMEs are discovered at build time via `readelf -d` rather than
/// hardcoded.  Remaining: bzip2, jbig2dec, mupdf.
pub const ZLIB_VERSION: &str = "1.3.2";
pub const LIBPNG_VERSION: &str = "1.6.53";
pub const DJVULIBRE_VERSION: &str = "3.5.29";
/// IJG libjpeg version tracked via the libjpeg-turbo `jpeg-<version>` tag mirror.
pub const LIBJPEG_VERSION: &str = "10";

pub const BZIP2_URL: &str = "https://sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz";
/// OpenJPEG version, derived from the archive URL.
pub const OPENJPEG_VERSION: &str = "2.5.4";
pub const JBIG2DEC_URL: &str =
    "https://github.com/ArtifexSoftware/jbig2dec/releases/download/0.20/jbig2dec-0.20.tar.gz";
/// FreeType version, cloned from `freetype/freetype` at tag `VER-X-Y-Z`.
///
/// Tracked by Renovate via the `github-tags` datasource with
/// `extractVersionTemplate: "^VER-(?<version>.+)$"`.  freetype2 is cloned
/// rather than downloaded as a tarball because its build system requires the
/// `nyorain/dlg` git submodule, which is absent from archive tarballs.
pub const FREETYPE2_VERSION: &str = "2.14.1";
/// HarfBuzz version, derived from the archive URL.
pub const HARFBUZZ_VERSION: &str = "12.3.0";
/// Gumbo version, derived from the archive URL.
pub const GUMBO_VERSION: &str = "0.10.1";

pub const MUPDF_URL: &str = "https://casper.mupdf.com/downloads/archive/mupdf-1.27.0-source.tar.gz";

/// All libraries in dependency order for building.
const LIBRARY_NAMES: &[&str] = &[
    "zlib",
    "bzip2",
    "libpng",
    "libjpeg",
    "openjpeg",
    "jbig2dec",
    "freetype2",
    "harfbuzz",
    "gumbo",
    "djvulibre",
    "mupdf",
];

/// Describes how a thirdparty library's source is obtained.
pub enum LibrarySource {
    /// Download a tarball and extract it with the top-level directory stripped.
    Tarball(String),
    /// Clone a git repository at a specific tag, recursing into submodules.
    Git { repo: String, tag: String },
}

/// Returns the source descriptor for a named library.
///
/// # Errors
///
/// Returns an error if `name` is not a known library.
pub fn library_source(name: &str) -> Result<LibrarySource> {
    match name {
        "zlib" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/madler/zlib/releases/download/v{v}/zlib-{v}.tar.gz",
            v = ZLIB_VERSION
        ))),
        "bzip2" => Ok(LibrarySource::Tarball(BZIP2_URL.to_owned())),
        "libpng" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/pnggroup/libpng/archive/refs/tags/v{v}.tar.gz",
            v = LIBPNG_VERSION
        ))),
        "libjpeg" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/jpeg-{v}.tar.gz",
            v = LIBJPEG_VERSION
        ))),
        "openjpeg" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/uclouvain/openjpeg/archive/v{v}.tar.gz",
            v = OPENJPEG_VERSION
        ))),
        "jbig2dec" => Ok(LibrarySource::Tarball(JBIG2DEC_URL.to_owned())),
        "freetype2" => Ok(LibrarySource::Git {
            repo: "https://github.com/freetype/freetype".to_owned(),
            tag: format!("VER-{}", FREETYPE2_VERSION.replace('.', "-")),
        }),
        "harfbuzz" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/harfbuzz/harfbuzz/archive/{v}.tar.gz",
            v = HARFBUZZ_VERSION
        ))),
        "gumbo" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/google/gumbo-parser/archive/v{v}.tar.gz",
            v = GUMBO_VERSION
        ))),
        "djvulibre" => Ok(LibrarySource::Tarball(format!(
            "https://github.com/barak/djvulibre/archive/refs/tags/release.{v}.tar.gz",
            v = DJVULIBRE_VERSION
        ))),
        "mupdf" => Ok(LibrarySource::Tarball(MUPDF_URL.to_owned())),
        _ => bail!("unknown thirdparty library: {name}"),
    }
}

/// Downloads source for the given libraries into `thirdparty/`.
///
/// When `names` is empty all libraries are downloaded.  Tarballs are extracted
/// with the top-level directory stripped.  Libraries with a [`LibrarySource::Git`]
/// source are cloned with `--recurse-submodules` so submodule contents are
/// always present.
///
/// Skips libraries with persisted marker files:
/// - source-ready marker ([`SOURCE_READY_MARKER`])
/// - built marker ([`BUILT_MARKER`])
///
/// This avoids fragile file-heuristic detection across heterogeneous upstream
/// source trees.
///
/// # Errors
///
/// Returns an error if any download, extraction, or clone fails.
pub fn download_libraries(thirdparty_dir: &Path, names: &[&str]) -> Result<()> {
    let targets: Vec<&str> = if names.is_empty() {
        LIBRARY_NAMES.to_vec()
    } else {
        names.to_vec()
    };

    for name in targets {
        let dest_dir = thirdparty_dir.join(name);

        if is_source_ready(&dest_dir) || is_built(&dest_dir) {
            println!("Skipping {name} (source ready)…");
            continue;
        }

        println!("Downloading {name}…");

        match library_source(name)? {
            LibrarySource::Tarball(url) => {
                let tarball = thirdparty_dir.join(format!("{name}.tgz"));

                if dest_dir.exists() {
                    clean_untracked(&dest_dir)?;
                } else {
                    std::fs::create_dir_all(&dest_dir)
                        .with_context(|| format!("failed to create {}", dest_dir.display()))?;
                }

                http::download(&url, &tarball)
                    .with_context(|| format!("failed to download {name}"))?;

                fs::extract_tarball_strip_one(&tarball, &dest_dir)
                    .with_context(|| format!("failed to extract {name}"))?;

                std::fs::remove_file(&tarball).ok();

                write_marker(&dest_dir, SOURCE_READY_MARKER, name, "source")?;
            }
            LibrarySource::Git { repo, tag } => {
                if !dest_dir.exists() {
                    std::fs::create_dir_all(&dest_dir)
                        .with_context(|| format!("failed to create {}", dest_dir.display()))?;
                }

                git_clone_tag(&repo, &tag, &dest_dir)
                    .with_context(|| format!("failed to clone {name}"))?;

                let autogen = dest_dir.join("autogen.sh");
                if autogen.exists() {
                    cmd::run("./autogen.sh", &[], &dest_dir, &[])
                        .with_context(|| format!("failed to run autogen.sh for {name}"))?;
                }

                write_marker(&dest_dir, SOURCE_READY_MARKER, name, "source")?;
            }
        }
    }

    Ok(())
}

/// Clones `repo` at `tag` into `dest`, recursing into submodules.
///
/// Clones into a temporary sibling directory first, then moves the contents
/// into `dest`.  This preserves any files already in `dest` that are tracked
/// in the cadmus repository (e.g. `build-kobo.sh`), matching the behaviour of
/// tarball extraction with `strip_one`.
fn git_clone_tag(repo: &str, tag: &str, dest: &Path) -> Result<()> {
    let tmp = dest.with_extension("_clone_tmp");

    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)
            .with_context(|| format!("failed to remove {}", tmp.display()))?;
    }

    cmd::run(
        "git",
        &[
            "clone",
            "--depth=1",
            "--recurse-submodules",
            "--branch",
            tag,
            repo,
            tmp.to_str().context("tmp path is not valid UTF-8")?,
        ],
        std::path::Path::new("."),
        &[],
    )?;

    for entry in
        std::fs::read_dir(&tmp).with_context(|| format!("failed to read {}", tmp.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", tmp.display()))?;
        let target = dest.join(entry.file_name());
        if target.exists() {
            if target.is_dir() {
                std::fs::remove_dir_all(&target).ok();
            } else {
                std::fs::remove_file(&target).ok();
            }
        }
        std::fs::rename(entry.path(), &target).with_context(|| {
            format!(
                "failed to move {} to {}",
                entry.path().display(),
                target.display()
            )
        })?;
    }

    std::fs::remove_dir_all(&tmp).with_context(|| format!("failed to remove {}", tmp.display()))?;

    Ok(())
}

/// Sentinel file written inside a library directory after source extraction.
///
/// Its presence means the source tree was fetched and unpacked successfully.
pub const SOURCE_READY_MARKER: &str = ".source-ready";

/// Sentinel file written inside a library directory after a successful build.
///
/// Its presence means the library was already compiled and cached — both the
/// patch and the build step can be skipped on the next run.
pub const BUILT_MARKER: &str = ".built-kobo";

/// Returns `true` if `dir` already has a completed source download marker.
fn is_source_ready(dir: &Path) -> bool {
    dir.join(SOURCE_READY_MARKER).exists()
}

/// Returns `true` if the library in `dir` was already built and the sentinel
/// file is present.
fn is_built(dir: &Path) -> bool {
    dir.join(BUILT_MARKER).exists()
}

/// Writes a marker file inside `dir` to persist task completion state.
fn write_marker(dir: &Path, marker: &str, name: &str, state: &str) -> Result<()> {
    std::fs::write(dir.join(marker), "")
        .with_context(|| format!("failed to write {state} marker for {name}"))
}

/// Builds the given libraries for the Kobo ARM target.
///
/// When `names` is empty all libraries are built in dependency order.  For
/// each library, `kobo.patch` is applied if present, then `./build-kobo.sh`
/// is invoked.  A sentinel file ([`BUILT_MARKER`]) is written on success so
/// that a warm CI cache can skip already-built libraries without re-applying
/// the patch or re-running the build script.
///
/// # Errors
///
/// Returns an error if patching or building any library fails.
pub fn build_libraries(thirdparty_dir: &Path, names: &[&str]) -> Result<()> {
    let targets: Vec<&str> = if names.is_empty() {
        LIBRARY_NAMES.to_vec()
    } else {
        names.to_vec()
    };

    for name in targets {
        let lib_dir = thirdparty_dir.join(name);

        if !lib_dir.exists() {
            bail!(
                "thirdparty/{name} not found — run `cargo xtask build-kobo --download-only` first"
            );
        }

        if is_built(&lib_dir) {
            println!("Skipping {name} (already built)…");
            continue;
        }

        println!("Building {name}…");

        let patch = lib_dir.join("kobo.patch");
        if patch.exists() {
            cmd::run("patch", &["-p", "1", "-i", "kobo.patch"], &lib_dir, &[])
                .with_context(|| format!("failed to apply kobo.patch for {name}"))?;
        }

        cmd::run("./build-kobo.sh", &[], &lib_dir, &[])
            .with_context(|| format!("failed to build {name}"))?;

        write_marker(&lib_dir, BUILT_MARKER, name, "build")?;
        write_marker(&lib_dir, SOURCE_READY_MARKER, name, "source")?;
    }

    Ok(())
}

/// Removes untracked files from a directory using `git ls-files`, falling back
/// to removing and recreating the directory when git is unavailable.
fn clean_untracked(dir: &Path) -> Result<()> {
    let result = std::process::Command::new("git")
        .args(["ls-files", "-o", "--directory", "-z"])
        .arg(dir.file_name().unwrap_or(dir.as_os_str()))
        .current_dir(dir.parent().unwrap_or(dir))
        .output();

    match result {
        Ok(output) if output.status.success() => {
            for entry in output.stdout.split(|&b| b == 0) {
                if entry.is_empty() {
                    continue;
                }

                let path = dir
                    .parent()
                    .unwrap_or(dir)
                    .join(std::str::from_utf8(entry).unwrap_or(""));

                if path.is_dir() {
                    std::fs::remove_dir_all(&path).ok();
                } else {
                    std::fs::remove_file(&path).ok();
                }
            }
        }
        _ => {
            std::fs::remove_dir_all(dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to recreate {}", dir.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_source_is_defined_for_all_known_libraries() {
        for name in LIBRARY_NAMES {
            let source = library_source(name).unwrap();
            match source {
                LibrarySource::Tarball(url) => {
                    assert!(
                        url.starts_with("http"),
                        "tarball URL for {name} should start with http"
                    );
                    assert!(
                        url.contains(".tar.gz"),
                        "tarball URL for {name} should contain .tar.gz"
                    );
                }
                LibrarySource::Git { repo, tag } => {
                    assert!(
                        repo.starts_with("https://"),
                        "git repo for {name} should use https"
                    );
                    assert!(!tag.is_empty(), "git tag for {name} should not be empty");
                }
            }
        }
    }

    #[test]
    fn library_source_errors_on_unknown_library() {
        assert!(library_source("nonexistent").is_err());
    }

    #[test]
    fn library_names_has_no_duplicates() {
        let mut names = LIBRARY_NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            LIBRARY_NAMES.len(),
            "duplicate library names found"
        );
    }
}
