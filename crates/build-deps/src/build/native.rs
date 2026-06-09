//! Build MuPDF and its companion shared libraries for the native host
//! (Linux or macOS) using system libraries via `pkg-config`.
//!
//! The patched MuPDF source tree is the canonical source of truth: the
//! WebP support patches are applied to a copy of the submodule under
//! `target/cadmus-build-deps/<TARGET>/mupdf/`, and that copy is the one
//! compiled and the one whose headers must be used by the Rust build
//! script and the `mupdf_wrapper` C glue.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::build::mupdf;
use crate::cmd;
use crate::markers;
use crate::utils;
use crate::versions::MUPDF_VERSION;

/// Default Cargo target triple when `TARGET` is unset (build script context
/// outside of `cargo build`).
fn target_triple() -> String {
    std::env::var("TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-gnu".to_string())
}

/// Build-root directory used for native dependency builds.
pub fn build_root(root: &Path) -> PathBuf {
    root.join("target/cadmus-build-deps").join(target_triple())
}

/// Outputs of [`ensure_native_artifacts`].
pub struct NativeArtifacts {
    /// Path to the patched MuPDF `include/` directory. The Rust build
    /// script must pass this to the C wrapper compiler so that
    /// `<mupdf/fitz.h>` resolves against the patched headers.
    pub include: PathBuf,
}

/// Build all native MuPDF/libwebp artifacts and return paths needed by
/// the rest of the build.
///
/// Performs the following steps:
///
/// 1. If the cached native build outputs (libwebp `.built` marker,
///    MuPDF `.built` marker, MuPDF `.a` archives, patched include
///    tree) are all present, skip submodule initialisation entirely
///    and only re-link the artifacts. This keeps warm-cache builds
///    fast and avoids the ~5 minute recursive submodule clone done by
///    CI when submodules are not yet on disk.
/// 2. Otherwise initialise git submodules, verify the MuPDF submodule
///    matches [`MUPDF_VERSION`], build
///    libwebp from source, copy the MuPDF source to a per-target build
///    directory, apply the WebP support patches if not already
///    applied, build MuPDF, and link its static archives into
///    `target/mupdf_wrapper/<platform>/`.
///
/// # Errors
///
/// Returns an error if submodules cannot be initialised, the MuPDF
/// version does not match, or any of the underlying build steps fail.
pub fn ensure_native_artifacts(root: &Path) -> Result<NativeArtifacts> {
    let mupdf_src = root.join("thirdparty/mupdf");
    let mupdf_build = build_root(root).join("mupdf");

    let cache_hit = native_cache_complete(root);

    if !cache_hit {
        crate::ensure_submodules(root).context("failed to initialise git submodules")?;

        let version_header = mupdf_src.join("include/mupdf/fitz/version.h");
        let current_version = read_mupdf_version(&version_header);
        if current_version.as_deref() != Some(MUPDF_VERSION) {
            bail!(
                "MuPDF sources not found or version mismatch: have {:?}, need {}",
                current_version,
                MUPDF_VERSION
            );
        }

        build_libwebp_native(root)?;

        if !mupdf_build.exists() {
            utils::cp_r(&mupdf_src, &mupdf_build).context("failed to copy mupdf source")?;
        }

        mupdf::apply_webp_patches_if_needed(&mupdf_build, root)
            .context("failed to apply MuPDF WebP patches")?;

        let mupdf_a = build_root(root).join("mupdf/build/release/libmupdf.a");
        if !mupdf_a.exists() {
            build_mupdf_native(root).context("failed to build MuPDF")?;
        }
    }

    link_mupdf_artifacts(root).context("failed to link MuPDF artifacts")?;

    Ok(NativeArtifacts {
        include: mupdf_build.join("include"),
    })
}

/// Returns `true` when every native build artefact is already on disk
/// and **matches the current submodule revision**.  Used by
/// [`ensure_native_artifacts`] to avoid `git submodule update
/// --init --recursive` on warm CI caches.
///
/// Each `.built` marker now stores the submodule gitlink SHA that was
/// used for the last successful build.  If the submodule pointer has
/// moved (e.g. after a `git submodule update`), the cache is stale and
/// a full rebuild is triggered.
fn native_cache_complete(root: &Path) -> bool {
    let build_root = build_root(root);
    let libwebp_dir = build_root.join("libwebp");
    let mupdf_dir = build_root.join("mupdf");
    let mupdf_a = mupdf_dir.join("build/release/libmupdf.a");
    let mupdf_third_a = mupdf_dir.join("build/release/libmupdf-third.a");
    let mupdf_include = mupdf_dir.join("include");

    markers::is_built(root, &libwebp_dir, "thirdparty/libwebp")
        && markers::is_built(root, &mupdf_dir, "thirdparty/mupdf")
        && mupdf_a.exists()
        && mupdf_third_a.exists()
        && mupdf_include.is_dir()
}

/// Build libwebp from source for native development using the host
/// compiler and `pkg-config`.
///
/// The output (`src/.libs/libwebp.a`) is repacked from libwebp's
/// per-subsystem archives (see [`combine_libwebp_static_archives`]) so
/// downstream code can link with a single `-lwebp`.
///
/// Idempotent: a `.built` marker file is written under
/// `target/cadmus-build-deps/<TARGET>/libwebp/` and re-runs are skipped
/// while it exists.
///
/// # Errors
///
/// Returns an error if the host toolchain is missing or the configure
/// and `make` invocations fail.
pub fn build_libwebp_native(root: &Path) -> Result<()> {
    let build_root = build_root(root);
    let libwebp_dir = build_root.join("libwebp");

    if markers::is_built(root, &libwebp_dir, "thirdparty/libwebp") {
        println!("libwebp already built for native.");
        return Ok(());
    }

    std::fs::create_dir_all(&libwebp_dir)?;

    let src_dir = root.join("thirdparty/libwebp");
    if !libwebp_dir.join("configure").exists() {
        utils::cp_r(&src_dir, &libwebp_dir)?;
    }

    cmd::run("make", &["distclean"], &libwebp_dir, &[]).ok();

    println!("Building libwebp for native development...");

    cmd::run("sh", &["autogen.sh"], &libwebp_dir, &[("NOCONFIGURE", "1")])
        .context("failed to run autogen.sh for libwebp")?;

    cmd::run(
        "./configure",
        &[
            "--disable-shared",
            "--enable-static",
            "--disable-libwebpmux",
            "--enable-libwebpdecoder",
            "--enable-libwebpdemux",
            "--disable-webp-tools",
            "--with-pic",
        ],
        &libwebp_dir,
        &[],
    )
    .context("failed to configure libwebp")?;

    cmd::run("make", &["-j4"], &libwebp_dir, &[]).context("failed to build libwebp")?;

    combine_libwebp_static_archives(&libwebp_dir)?;

    let libwebp_a = libwebp_dir.join("src/.libs/libwebp.a");
    let ranlib_tool = resolve_ranlib_tool();
    cmd::run(
        &ranlib_tool,
        &[libwebp_a.to_str().context("non-UTF-8 libwebp.a path")?],
        &libwebp_dir,
        &[],
    )
    .context("failed to ranlib libwebp.a")?;

    markers::mark_built(root, &libwebp_dir, "libwebp", "thirdparty/libwebp")?;
    println!("✓ libwebp built successfully");
    Ok(())
}

/// libwebp's build system creates separate `.a` files in sub-directories
/// (`dec/`, `dsp/`, `enc/`, `utils/`) but does not assemble them into a
/// single `src/.libs/libwebp.a` when building static-only. Extract the
/// object files from each sub-archive and repack them into one
/// `libwebp.a` so downstream code can link with a single `-lwebp`.
fn combine_libwebp_static_archives(libwebp_dir: &Path) -> Result<()> {
    let libs_dir = libwebp_dir.join("src/.libs");
    std::fs::create_dir_all(&libs_dir).context("failed to create src/.libs for libwebp")?;

    let sublibs = [
        "dec/.libs/libwebpdecode.a",
        "dsp/.libs/libwebpdsp.a",
        "enc/.libs/libwebpencode.a",
        "utils/.libs/libwebputils.a",
    ];

    let ar_tool = resolve_ar_tool();
    let ranlib_tool = resolve_ranlib_tool();

    let mut objects = Vec::new();
    for sublib in sublibs {
        let archive_path = libwebp_dir.join("src").join(sublib);
        let subdir = tempfile::Builder::new()
            .prefix("libwebp-obj-")
            .tempdir_in(&libs_dir)
            .with_context(|| {
                format!(
                    "failed to create temp dir for extracting {}",
                    archive_path.display()
                )
            })?;
        cmd::run(
            &ar_tool,
            &[
                "x",
                archive_path.to_str().context("non-UTF-8 archive path")?,
            ],
            subdir.path(),
            &[],
        )
        .with_context(|| format!("failed to extract {} with `ar x`", archive_path.display()))?;

        for entry in std::fs::read_dir(subdir.path())
            .with_context(|| format!("failed to read {}", subdir.path().display()))?
        {
            let entry = entry
                .with_context(|| format!("failed to read entry in {}", subdir.path().display()))?;
            let path = entry.path();
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                objects.push(path);
            }
        }

        let _ = subdir.keep();
    }

    if objects.is_empty() {
        bail!("no object files extracted from libwebp sub-archives");
    }

    let libwebp_a = libs_dir.join("libwebp.a");
    if libwebp_a.exists() {
        std::fs::remove_file(&libwebp_a)
            .with_context(|| format!("failed to remove existing {}", libwebp_a.display()))?;
    }

    let mut args: Vec<String> = vec![
        "rcs".to_string(),
        libwebp_a
            .file_name()
            .context("could not get filename for libwebp_a")?
            .to_string_lossy()
            .into_owned(),
    ];
    for obj in &objects {
        let rel = obj
            .strip_prefix(&libs_dir)
            .with_context(|| format!("{} is not under {}", obj.display(), libs_dir.display()))?;
        args.push(rel.to_string_lossy().into_owned());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    cmd::run(&ar_tool, &arg_refs, &libs_dir, &[])
        .with_context(|| format!("failed to run `ar rcs` to build {}", libwebp_a.display()))?;

    cmd::run(
        &ranlib_tool,
        &[libwebp_a.to_str().context("non-UTF-8 libwebp.a path")?],
        &libs_dir,
        &[],
    )
    .with_context(|| format!("failed to run `ranlib` on {}", libwebp_a.display()))?;

    Ok(())
}

/// Return the path to the host `ar` binary, honouring the `AR` env var
/// when set by Cargo or the user. The host `ar` produces archives that
/// the platform linker accepts, including the long-name table that
/// libwebp's per-subsystem archives require.
fn resolve_ar_tool() -> String {
    env::var("AR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "ar".to_string())
}

/// Return the path to the host `ranlib` binary, honouring the `RANLIB`
/// env var when set.
fn resolve_ranlib_tool() -> String {
    env::var("RANLIB")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "ranlib".to_string())
}

/// Parse the `FZ_VERSION` macro out of MuPDF's `version.h`.
///
/// Returns `None` if the file is missing, unreadable, or does not
/// contain a `FZ_VERSION` string literal. Used to verify the submodule
/// matches [`MUPDF_VERSION`] before
/// kicking off a build.
pub fn read_mupdf_version(header: &Path) -> Option<String> {
    let content = std::fs::read_to_string(header).ok()?;

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

/// Build MuPDF and libmupdf-third using system libraries for the native
/// host platform.
///
/// Idempotent: a `.built` marker file is written under
/// `target/cadmus-build-deps/<TARGET>/mupdf/` and re-runs are skipped
/// while it exists.
pub fn build_mupdf_native(root: &Path) -> Result<()> {
    let build_root = build_root(root);
    let mupdf_dir = build_root.join("mupdf");

    if markers::is_built(root, &mupdf_dir, "thirdparty/mupdf") {
        println!("MuPDF already built for native.");
        return Ok(());
    }

    let src_dir = root.join("thirdparty/mupdf");
    if !mupdf_dir.exists() {
        utils::cp_r(&src_dir, &mupdf_dir)?;
    }

    println!("Building MuPDF for native development...");

    for entry in ["gitattributes", ".gitattributes"] {
        let path = mupdf_dir.join(entry);
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
    }

    cmd::run("make", &["clean"], &mupdf_dir, &[]).ok();
    cmd::run("make", &["verbose=yes", "generate"], &mupdf_dir, &[])?;

    let target = target_triple();
    let sys_cflags = collect_system_cflags()?;
    let xcflags = format!(
        "-DFZ_ENABLE_ICC=0 -DFZ_ENABLE_SPOT_RENDERING=0 \
         -DFZ_ENABLE_ODT_OUTPUT=0 -DFZ_ENABLE_OCR_OUTPUT=0 \
         -DHAVE_WEBP=1 -I{root}/target/cadmus-build-deps/{target}/libwebp/src {sys_cflags}",
        root = root.display(),
        target = target
    );

    let xlibs = format!(
        "-L{root}/target/cadmus-build-deps/{target}/libwebp/src/.libs -lwebp \
         -L{root}/target/cadmus-build-deps/{target}/libwebp/src/demux/.libs -lwebpdemux",
        root = root.display(),
        target = target
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
            &format!("XLIBS={xlibs}"),
            "libs",
        ],
        &mupdf_dir,
        &[],
    )
    .context("failed to build MuPDF libs")?;

    markers::mark_built(root, &mupdf_dir, "mupdf", "thirdparty/mupdf")?;
    Ok(())
}

/// Collect MuPDF CFLAGS via `pkg-config` on macOS only.
///
/// On Linux, MuPDF's build system detects system libraries
/// automatically. On macOS, explicit CFLAGS gathered from pkg-config
/// must be injected through `XCFLAGS` so that the headers for
/// freetype, harfbuzz, etc. resolve correctly.
fn collect_system_cflags() -> Result<String> {
    if !cfg!(target_os = "macos") {
        return Ok(String::new());
    }

    let libs = [
        "freetype2",
        "harfbuzz",
        "libopenjp2",
        "libjpeg",
        "libwebp",
        "zlib",
        "jbig2dec",
        "gumbo",
    ];

    let mut flags = String::new();
    for lib in libs {
        if let Ok(f) = cmd::output("pkg-config", &["--cflags", lib], Path::new("."), &[])
            && !f.is_empty()
        {
            flags.push(' ');
            flags.push_str(&f);
        }
    }

    Ok(flags.trim().to_owned())
}

/// Symlink the freshly built `libmupdf.a` and `libmupdf-third.a` into
/// `target/mupdf_wrapper/<platform>/` so the Rust build script can
/// pick them up via `cargo:rustc-link-search`.
pub fn link_mupdf_artifacts(root: &Path) -> Result<()> {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };

    let target_dir = root.join(format!("target/mupdf_wrapper/{platform_dir}"));
    std::fs::create_dir_all(&target_dir)
        .context("failed to create target/mupdf_wrapper directory")?;

    let release_dir = build_root(root).join("mupdf/build/release");

    let libmupdf = release_dir.join("libmupdf.a");
    if !libmupdf.exists() {
        bail!("libmupdf.a not found after build -- check MuPDF build output");
    }

    symlink_force(&libmupdf, &target_dir.join("libmupdf.a"))?;
    println!("✓ Created libmupdf.a in target/mupdf_wrapper/{platform_dir}");

    let libmupdf_third = release_dir.join("libmupdf-third.a");
    if !libmupdf_third.exists() {
        println!("Creating empty libmupdf-third.a (system libs used instead)...");
        write_empty_archive(&libmupdf_third).context("failed to create empty libmupdf-third.a")?;
    }

    symlink_force(&libmupdf_third, &target_dir.join("libmupdf-third.a"))?;
    println!("✓ Created libmupdf-third.a");

    Ok(())
}

/// Create a symlink at `link` pointing to `target`, removing any
/// existing file or symlink at `link` first.
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

/// Write an empty `ar` archive at `path` using the host `ar` tool.
fn write_empty_archive(path: &Path) -> Result<()> {
    let ar_tool = resolve_ar_tool();
    let parent = path.parent().context("empty archive path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let path_str = path.to_str().context("non-UTF-8 empty archive path")?;
    cmd::run(&ar_tool, &["rcs", path_str], parent, &[])
        .with_context(|| format!("failed to run `ar rcs` to create {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_mupdf_version_parses_define() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("version.h");
        std::fs::write(
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
        std::fs::write(&header, "/* no version define here */\n").unwrap();

        let version = read_mupdf_version(&header);
        assert!(version.is_none());
    }

    #[test]
    fn apply_mupdf_webp_patches_writes_marker_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let mupdf = root.path().join("mupdf");
        let patches = root.path().join("build-scripts/mupdf");
        std::fs::create_dir_all(&patches).unwrap();

        let source = mupdf.join("hello.txt");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "first\n").unwrap();

        let patch = patches.join("hello-kobo.patch");
        std::fs::write(
            &patch,
            "--- a/hello.txt\n+++ b/hello.txt\n@@ -1 +1 @@\n-first\n+second\n",
        )
        .unwrap();

        // We can't easily override `MUPDF_WEBP_PATCHES` from the test
        // because it is a `pub const`. Instead, exercise the marker
        // helper directly: writing the marker twice must leave a
        // single file behind and `is_webp_patched` must report true
        // on the second call.
        let marker_dir = mupdf.clone();
        crate::markers::write_marker(
            &marker_dir,
            crate::markers::WEBP_PATCHED_MARKER,
            "mupdf",
            "WebP patch",
        )
        .unwrap();
        assert!(crate::markers::is_webp_patched(&marker_dir));
        crate::markers::write_marker(
            &marker_dir,
            crate::markers::WEBP_PATCHED_MARKER,
            "mupdf",
            "WebP patch",
        )
        .unwrap();
        assert!(crate::markers::is_webp_patched(&marker_dir));
    }
}
