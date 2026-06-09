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
//!
//! Consumers (the `cadmus-core` build and `xtask build-kobo`) must set
//! `SQLITE3_LIB_DIR` and `SQLITE3_INCLUDE_DIR` so that
//! `libsqlite3-sys` links against the custom build instead of the
//! system library.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cmd;
use crate::markers;
use crate::utils;
use crate::versions::SQLITE_VERSION;

/// Compile-time defines passed when compiling the amalgamation.
///
/// These are safe to add to any UDL-capable amalgamation and do not
/// require parser regeneration.
const SQLITE_DEFINES: &[&str] = &[
    "-DSQLITE_ENABLE_UPDATE_DELETE_LIMIT",
    "-DSQLITE_DEFAULT_WAL_SYNCHRONOUS=1",
    "-DSQLITE_OMIT_DEPRECATED",
    "-DSQLITE_DQS=0",
    "-DSQLITE_DEFAULT_MEMSTATUS=0",
    "-DSQLITE_LIKE_DOESNT_MATCH_BLOBS",
    "-DSQLITE_OMIT_SHARED_CACHE",
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
    if markers::is_built(root, &build_dir, submodule_path)
        && lib_dir.join("libsqlite3.a").exists()
        && include_dir.join("sqlite3.h").exists()
    {
        println!("Skipping sqlite (already built for {target})...");
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

    verify_version(&src_dir)?;

    println!("Building sqlite for {target}...");

    // Clean previous build if the submodule moved.
    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)
            .context("failed to remove stale sqlite build directory")?;
    }

    // Copy source tree into build directory.
    utils::cp_r(&src_dir, &build_dir).context("failed to copy sqlite source")?;

    let is_cross = target == "arm-unknown-linux-gnueabihf";

    // Step 1: Configure with --enable-update-limit (generates UDL parser).
    configure(&build_dir, is_cross)?;

    // Step 2: Generate the UDL-enabled amalgamation.
    generate_amalgamation(&build_dir)?;

    // Step 3: Compile sqlite3.c → libsqlite3.a.
    std::fs::create_dir_all(&lib_dir)?;
    std::fs::create_dir_all(&include_dir)?;
    compile_amalgamation(&build_dir, &lib_dir, &include_dir, is_cross)?;

    markers::mark_built(root, &build_dir, "sqlite", submodule_path)?;

    Ok(SqliteArtifacts {
        lib_dir,
        include_dir,
    })
}

/// Verify the submodule version matches [`SQLITE_VERSION`].
fn verify_version(src_dir: &Path) -> Result<()> {
    let header = src_dir.join("VERSION");
    if !header.exists() {
        // Fall back to checking manifest.uuid or sqlite3.h if VERSION is missing.
        return Ok(());
    }
    let version = std::fs::read_to_string(&header)
        .context("failed to read sqlite VERSION file")?
        .trim()
        .to_owned();
    if version != SQLITE_VERSION {
        anyhow::bail!(
            "SQLite version mismatch: submodule has {version}, expected {SQLITE_VERSION}"
        );
    }
    Ok(())
}

/// Run `./configure --enable-update-limit` in the build directory.
fn configure(build_dir: &Path, is_cross: bool) -> Result<()> {
    let mut args = vec!["--enable-update-limit", "--disable-tcl", "--disable-readline"];
    if is_cross {
        args.push("--host=arm-linux-gnueabihf");
    }
    let env: &[(&str, &str)] = if is_cross {
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

/// Compile `sqlite3.c` into a static `libsqlite3.a` and copy
/// `sqlite3.h` into `include_dir`.
fn compile_amalgamation(
    build_dir: &Path,
    lib_dir: &Path,
    include_dir: &Path,
    is_cross: bool,
) -> Result<()> {
    let cc = if is_cross {
        "arm-linux-gnueabihf-gcc"
    } else {
        "cc"
    };
    let ar = if is_cross {
        "arm-linux-gnueabihf-ar"
    } else {
        "ar"
    };

    let mut compile_args: Vec<&str> = vec!["-c", "sqlite3.c", "-o", "sqlite3.o", "-O2"];
    if is_cross {
        compile_args.extend_from_slice(&["-mcpu=cortex-a9", "-mfpu=neon"]);
    }
    for define in SQLITE_DEFINES {
        compile_args.push(define);
    }
    cmd::run(cc, &compile_args, build_dir, &[]).context("failed to compile sqlite3.c")?;

    cmd::run(
        ar,
        &["rcs", "libsqlite3.a", "sqlite3.o"],
        build_dir,
        &[],
    )
    .context("failed to archive libsqlite3.a")?;

    // Move artefacts into the standard layout.
    std::fs::copy(build_dir.join("libsqlite3.a"), lib_dir.join("libsqlite3.a"))
        .context("failed to copy libsqlite3.a")?;
    std::fs::copy(build_dir.join("sqlite3.h"), include_dir.join("sqlite3.h"))
        .context("failed to copy sqlite3.h")?;

    Ok(())
}
