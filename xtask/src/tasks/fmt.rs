//! `cargo xtask fmt` — check or apply rustfmt formatting.
//!
//! In check mode (the default, used in CI) the task exits non-zero if any
//! file would be reformatted.  With `--apply` the formatting is written back
//! to disk.

use anyhow::Result;
use clap::Args;

use super::util::{cmd, workspace};

/// Arguments for `cargo xtask fmt`.
#[derive(Debug, Args)]
pub struct FmtArgs {
    /// Apply formatting instead of checking.
    ///
    /// Without this flag the command runs `cargo fmt --all --check`, which is
    /// suitable for CI.  Pass `--apply` to reformat files in place.
    #[arg(long)]
    pub apply: bool,
}

/// Runs `cargo fmt` across the entire workspace.
///
/// # Errors
///
/// Returns an error if `cargo fmt` exits with a non-zero status (i.e. files
/// are not formatted when running in check mode).
pub fn run(args: FmtArgs) -> Result<()> {
    let root = workspace::root()?;

    let mut fmt_args = vec!["fmt", "--all"];
    if !args.apply {
        fmt_args.push("--check");
    }

    cmd::run("cargo", &fmt_args, &root, &[])
}
