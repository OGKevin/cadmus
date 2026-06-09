//! `cargo xtask setup` — build thirdparty dependencies that must be
//! ready before `cargo build` runs.
//!
//! Currently this covers SQLite only: `libsqlite3-sys`'s own build
//! script runs before `cadmus-core`'s build.rs, so the custom SQLite
//! library (built with `SQLITE_ENABLE_UPDATE_DELETE_LIMIT`) must
//! already be on disk and pointed to by `SQLITE3_LIB_DIR` /
//! `SQLITE3_INCLUDE_DIR`.
//!
//! ## Usage
//!
//! ```text
//! cargo xtask setup                 # build for the native host
//! cargo xtask setup --target arm-unknown-linux-gnueabihf  # Kobo cross-build
//! ```
//!
//! After running, set the printed environment variables before
//! `cargo build` or `cargo xtask build-kobo`.

use anyhow::{Context, Result};
use clap::Args;

use build_deps::build::sqlite;

use super::util::workspace;

/// Arguments for `cargo xtask setup`.
#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Target triple to build for. Defaults to the host triple.
    #[arg(long)]
    pub target: Option<String>,
}

/// Build thirdparty dependencies that must exist before `cargo build`.
///
/// # Errors
///
/// Returns an error if:
/// - Git submodules cannot be initialised.
/// - TCL is not installed (required for SQLite amalgamation generation).
/// - The SQLite build fails.
pub fn run(args: SetupArgs) -> Result<()> {
    let root = workspace::root()?;

    build_deps::ensure_submodules(&root).context("failed to initialise git submodules")?;

    let target = args.target.unwrap_or_else(guess_host_triple);

    let artifacts = sqlite::ensure_sqlite(&root, &target).context("failed to build sqlite")?;

    println!();
    println!("SQLite artifacts ready. Set the following env vars before cargo build:");
    println!("  export SQLITE3_LIB_DIR={}", artifacts.lib_dir.display());
    println!(
        "  export SQLITE3_INCLUDE_DIR={}",
        artifacts.include_dir.display()
    );
    println!("  export SQLITE3_STATIC=1");

    Ok(())
}

/// Best-effort detection of the host target triple.
fn guess_host_triple() -> String {
    // In a cargo context TARGET is always set; outside of it we fall
    // back to a compile-time constant matching the xtask binary.
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
