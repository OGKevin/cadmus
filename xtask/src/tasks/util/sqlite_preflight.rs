//! Ensure custom SQLite artifacts exist before cargo invokes `libsqlite3-sys`.

use std::path::Path;

use anyhow::{Context, Result};
use build_deps::build::sqlite::{self, SqliteArtifacts};

/// Best-effort detection of the host target triple.
#[must_use]
pub fn guess_host_triple() -> String {
    std::env::var("TARGET").unwrap_or_else(|_| {
        if cfg!(target_arch = "x86_64") && cfg!(target_os = "linux") {
            "x86_64-unknown-linux-gnu".to_string()
        } else if cfg!(target_arch = "aarch64") && cfg!(target_os = "linux") {
            "aarch64-unknown-linux-gnu".to_string()
        } else if cfg!(target_arch = "x86_64") && cfg!(target_os = "macos") {
            "x86_64-apple-darwin".to_string()
        } else if cfg!(target_arch = "aarch64") && cfg!(target_os = "macos") {
            "aarch64-apple-darwin".to_string()
        } else {
            "x86_64-unknown-linux-gnu".to_string()
        }
    })
}

/// Ensure UDL-enabled SQLite exists for `target` under `target/cadmus-build-deps/`.
///
/// When artifacts are missing, prints a cold-build message, initializes git
/// submodules if needed, then builds SQLite. Warm cache skips the cold path.
///
/// # Errors
///
/// Returns an error if submodules cannot be initialised or the SQLite build fails.
pub fn ensure_for_target(root: &Path, target: &str) -> Result<SqliteArtifacts> {
    if !sqlite::is_cached(root, target) {
        println!("SQLite not found for {target} — building…");
        build_deps::ensure_submodules(root).context("failed to initialise git submodules")?;
    }
    sqlite::ensure_sqlite(root, target).context("failed to build sqlite")
}

/// Ensure SQLite artifacts for the native host target.
///
/// # Errors
///
/// See [`ensure_for_target`].
pub fn ensure_host(root: &Path) -> Result<SqliteArtifacts> {
    ensure_for_target(root, &guess_host_triple())
}
