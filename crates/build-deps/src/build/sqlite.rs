//! Build SQLite from the canonical source tree with
//! `SQLITE_ENABLE_UPDATE_DELETE_LIMIT` support.
//!
//! The standard SQLite amalgamation shipped by `libsqlite3-sys` does
//! not include a UDL-capable parser, so `DELETE … LIMIT` is rejected
//! at parse time regardless of compile flags. Building from the
//! canonical source with `--enable-update-limit` regenerates the
//! parser grammar via Lemon (requires TCL) and bakes in
//! `SQLITE_UDL_CAPABLE_PARSER`.
//!
//! # Why this must run before `cargo build`
//!
//! `libsqlite3-sys`'s build script runs before `cadmus-core`'s
//! `build.rs`, so the custom SQLite library must already be on disk
//! when Cargo resolves the dependency graph. There is no way to
//! trigger the build from `cadmus-core`'s own build script because
//! it executes too late in the chain. `cargo xtask setup` (or the
//! Kobo build flow) must be run first to place the artefacts where
//! `libsqlite3-sys` can find them via `SQLITE3_LIB_DIR` /
//! `SQLITE3_INCLUDE_DIR`.
//!
//! # Output layout
//!
//! ```text
//! target/cadmus-build-deps/<TARGET>/sqlite/
//! ├── .built          # submodule-SHA marker
//! ├── include/
//! │   └── sqlite3.h
//! └── lib/
//!     └── libsqlite3.a
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cmd;
use crate::markers;
use crate::utils;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets/"]
struct Assets;

/// Kobo ARM target triple.
pub const KOBO_TARGET: &str = "arm-unknown-linux-gnueabihf";

/// Compile-time defines passed when compiling the amalgamation.
///
/// These are safe to add to any UDL-capable amalgamation and do not
/// require parser regeneration.
const SQLITE_DEFINES: &[&str] = &[
    "-DSQLITE_ENABLE_UPDATE_DELETE_LIMIT",
    "-DSQLITE_ENABLE_COLUMN_METADATA",
    "-DSQLITE_ENABLE_UNLOCK_NOTIFY",
    "-DSQLITE_DEFAULT_WAL_SYNCHRONOUS=1",
    "-DSQLITE_OMIT_DEPRECATED",
    "-DSQLITE_DQS=0",
    "-DSQLITE_DEFAULT_MEMSTATUS=0",
    "-DSQLITE_LIKE_DOESNT_MATCH_BLOBS",
];

/// Artefact paths produced by [`ensure_sqlite`].
pub struct SqliteArtifacts {
    /// Directory containing `libsqlite3.a`.
    pub lib_dir: PathBuf,
    /// Directory containing `sqlite3.h`.
    pub include_dir: PathBuf,
}

/// Build SQLite from the canonical source for the given target,
/// placing artefacts under `target/cadmus-build-deps/<target>/sqlite/`.
///
/// The build is skipped when a `.built` marker matching the current
/// submodule SHA already exists.
///
/// Stale build directories are removed before starting so that
/// submodule updates always produce a clean build.
///
/// # Arguments
///
/// * `root`   — workspace root (parent of `thirdparty/`).
/// * `target` — Cargo target triple (e.g.
///   `x86_64-unknown-linux-gnu` or `arm-unknown-linux-gnueabihf`).
///
/// # Errors
///
/// Returns an error if TCL is not installed, `./configure` fails, or
/// any of the compilation steps fail.
pub fn ensure_sqlite(root: &Path, target: &str) -> Result<SqliteArtifacts> {
    let build_root = root.join("target/cadmus-build-deps").join(target);
    let build_dir = build_root.join("sqlite");
    let lib_dir = build_dir.join("lib");
    let include_dir = build_dir.join("include");

    let submodule_path = "thirdparty/sqlite";
    let sqlite_version = std::fs::read_to_string(root.join(submodule_path).join("VERSION"))
        .context("failed to read thirdparty/sqlite/VERSION")?;
    let sqlite_version = sqlite_version.trim();

    if markers::is_built(root, &build_dir, submodule_path)
        && lib_dir.join("libsqlite3.a").exists()
        && include_dir.join("sqlite3.h").exists()
    {
        println!("Skipping sqlite (already built for {target})...");
        write_pkgconfig(&lib_dir, &include_dir, sqlite_version)
            .context("failed to write sqlite3.pc for cached build")?;
        return Ok(SqliteArtifacts {
            lib_dir,
            include_dir,
        });
    }

    let src_dir = root.join(submodule_path);
    if !src_dir.exists() {
        anyhow::bail!(
            "{submodule_path} not found — run `git submodule update --init --recursive` first"
        );
    }

    println!("Building sqlite for {target}...");

    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)
            .context("failed to remove stale sqlite build directory")?;
    }

    utils::cp_r(&src_dir, &build_dir).context("failed to copy sqlite source")?;

    configure(&build_dir, target)?;
    generate_amalgamation(&build_dir)?;

    std::fs::create_dir_all(&lib_dir)?;
    std::fs::create_dir_all(&include_dir)?;
    compile_amalgamation(&build_dir, &lib_dir, &include_dir, target)?;

    write_pkgconfig(&lib_dir, &include_dir, sqlite_version)
        .context("failed to write sqlite3.pc")?;

    markers::mark_built(root, &build_dir, "sqlite", submodule_path)?;

    Ok(SqliteArtifacts {
        lib_dir,
        include_dir,
    })
}

/// Writes a `pkgconfig/sqlite3.pc` file describing the custom static build.
///
/// `libsqlite3-sys` (used via sqlx's `sqlite-unbundled` feature) supports
/// per-target library discovery only through pkg-config, since its
/// `SQLITE3_LIB_DIR` lookup is not target-aware. During cross-compilation the
/// host proc-macro build and the ARM target build each need a different
/// `libsqlite3.a`; pointing `PKG_CONFIG_PATH_<target>` at the matching `.pc`
/// resolves the conflict.
///
/// Only `libsqlite3.a` exists in `lib_dir`, so the linker resolves `-lsqlite3`
/// to the static archive automatically without needing an explicit
/// `static=` directive.
fn write_pkgconfig(lib_dir: &Path, include_dir: &Path, version: &str) -> Result<()> {
    let pkgconfig_dir = lib_dir.join("pkgconfig");
    std::fs::create_dir_all(&pkgconfig_dir).context("failed to create pkgconfig directory")?;

    let template = Assets::get("sqlite3.pc.template")
        .ok_or_else(|| anyhow::anyhow!("sqlite3.pc.template not embedded"))?;
    let contents = std::str::from_utf8(template.data.as_ref())
        .context("sqlite3.pc.template is not valid UTF-8")?
        .replace("{lib}", &lib_dir.display().to_string())
        .replace("{inc}", &include_dir.display().to_string())
        .replace("{version}", version);

    std::fs::write(pkgconfig_dir.join("sqlite3.pc"), contents).context("failed to write sqlite3.pc")
}

/// Run `./configure --enable-update-limit` in the build directory.
///
/// For cross-compilation targets the appropriate `--host`, `CC`, `AR`,
/// `RANLIB`, `STRIP`, and `CFLAGS` overrides are applied automatically.
fn configure(build_dir: &Path, target: &str) -> Result<()> {
    let mut args = vec![
        "--enable-update-limit",
        "--disable-tcl",
        "--disable-readline",
    ];
    if target == KOBO_TARGET {
        args.push("--host=arm-linux-gnueabihf");
    }
    let env: &[(&str, &str)] = if target == KOBO_TARGET {
        &[
            ("CC", "arm-linux-gnueabihf-gcc"),
            ("AR", "arm-linux-gnueabihf-ar"),
            ("RANLIB", "arm-linux-gnueabihf-ranlib"),
            ("STRIP", "arm-linux-gnueabihf-strip"),
            ("CFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ]
    } else {
        &[]
    };
    cmd::run("./configure", &args, build_dir, env).context("failed to configure sqlite")
}

/// Generate the UDL-enabled amalgamation (`sqlite3.c`, `sqlite3.h`).
fn generate_amalgamation(build_dir: &Path) -> Result<()> {
    cmd::run("make", &["sqlite3.c", "sqlite3.h"], build_dir, &[])
        .context("failed to generate sqlite amalgamation (is tclsh installed?)")
}

/// Compile `sqlite3.c` into a static `libsqlite3.a` and install
/// `sqlite3.h` into `include_dir` and the archive into `lib_dir`.
fn compile_amalgamation(
    build_dir: &Path,
    lib_dir: &Path,
    include_dir: &Path,
    target: &str,
) -> Result<()> {
    let cc = if target == KOBO_TARGET {
        "arm-linux-gnueabihf-gcc"
    } else {
        "cc"
    };
    let ar = if target == KOBO_TARGET {
        "arm-linux-gnueabihf-ar"
    } else {
        "ar"
    };

    // `-fPIC` is required because the final Cadmus binaries are linked as
    // position-independent executables (`-pie`); a non-PIC static archive
    // triggers `R_ARM_*` relocation errors at link time on the ARM target.
    let mut compile_args: Vec<&str> = vec!["-c", "sqlite3.c", "-o", "sqlite3.o", "-O2", "-fPIC"];
    if target == KOBO_TARGET {
        compile_args.extend_from_slice(&["-mcpu=cortex-a9", "-mfpu=neon"]);
    }
    for define in SQLITE_DEFINES {
        compile_args.push(define);
    }
    cmd::run(cc, &compile_args, build_dir, &[]).context("failed to compile sqlite3.c")?;

    cmd::run(ar, &["rcs", "libsqlite3.a", "sqlite3.o"], build_dir, &[])
        .context("failed to archive libsqlite3.a")?;

    std::fs::copy(build_dir.join("libsqlite3.a"), lib_dir.join("libsqlite3.a"))
        .context("failed to copy libsqlite3.a")?;
    std::fs::copy(build_dir.join("sqlite3.h"), include_dir.join("sqlite3.h"))
        .context("failed to copy sqlite3.h")?;

    Ok(())
}
