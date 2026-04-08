//! `cargo xtask bench` — run benchmarks with the `bench` feature enabled.
//!
//! Runs `cargo bench --features bench` for the workspace, activating the
//! `bench` feature flag that exposes internal module visibility required by
//! the criterion benchmark targets.

use anyhow::Result;
use clap::Args;

use super::util::{cmd, workspace};

/// Arguments for `cargo xtask bench`.
#[derive(Debug, Args)]
pub struct BenchArgs {}

/// Runs `cargo bench --features bench` from the workspace root.
///
/// # Errors
///
/// Returns an error if the benchmark process exits non-zero.
pub fn run(_args: BenchArgs) -> Result<()> {
    let root = workspace::root()?;
    cmd::run("cargo", &["bench", "--features", "bench"], &root, &[])
}
