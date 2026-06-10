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
//! cargo xtask setup           # build for all known targets (host + Kobo)
//! cargo xtask setup --host    # build for the native host only
//! cargo xtask setup --kobo    # build for Kobo (ARM) only
//! cargo xtask setup --all     # explicitly build for all known targets
//! cargo xtask setup --target <triple>  # build for an arbitrary target
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
    /// Build for the native host target only.
    #[arg(long)]
    pub host: bool,

    /// Build for the Kobo ARM target only.
    #[arg(long)]
    pub kobo: bool,

    /// Build for all known targets (host + Kobo). Implied when no
    /// flags are passed.
    #[arg(long)]
    pub all: bool,

    /// Build for an arbitrary target triple (advanced).
    #[arg(long)]
    pub target: Option<String>,
}

/// Build thirdparty dependencies that must exist before `cargo build`.
///
/// When no target flags are supplied the default is `--all`, which
/// builds for every known target (native host + Kobo ARM).
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

    let targets = resolve_targets(&args);

    for target in &targets {
        let artifacts = sqlite::ensure_sqlite(&root, target).context("failed to build sqlite")?;

        println!();
        println!("SQLite artifacts ready for {target}:");
        println!("  export SQLITE3_LIB_DIR={}", artifacts.lib_dir.display());
        println!(
            "  export SQLITE3_INCLUDE_DIR={}",
            artifacts.include_dir.display()
        );
        println!("  export SQLITE3_STATIC=1");
    }

    Ok(())
}

/// Determine which target triples to build based on CLI flags.
///
/// `--target` takes precedence; otherwise `--host` / `--kobo` select
/// individual targets. When none are given `--all` is implied.
fn resolve_targets(args: &SetupArgs) -> Vec<String> {
    if let Some(ref t) = args.target {
        return vec![t.clone()];
    }

    let build_all = args.all || (!args.host && !args.kobo);

    let mut targets = Vec::new();
    if build_all || args.host {
        targets.push(guess_host_triple());
    }
    if build_all || args.kobo {
        targets.push(sqlite::KOBO_TARGET.to_string());
    }
    targets
}

/// Best-effort detection of the host target triple.
fn guess_host_triple() -> String {
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
