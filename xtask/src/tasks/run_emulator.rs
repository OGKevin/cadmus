//! `cargo xtask run-emulator` — run the Cadmus emulator.
//!
//! Ensures MuPDF sources and the `mupdf_wrapper` C library are built for the
//! native platform, then launches `cargo run -p emulator`.  Any extra
//! arguments are forwarded to the emulator.

use anyhow::Result;
use clap::Args;

use super::setup_native;
use super::util::{cmd, mupdf_wrapper, workspace};

/// Arguments for `cargo xtask run-emulator`.
#[derive(Debug, Args)]
pub struct RunEmulatorArgs {
    /// Cargo feature flags forwarded to `cargo run -p emulator`.
    #[arg(long)]
    pub features: Option<String>,

    /// Extra arguments forwarded to `cargo run -p emulator`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

/// Ensures prerequisites are built then launches the emulator.
///
/// # Errors
///
/// Returns an error if the MuPDF download, wrapper build, or emulator launch
/// fails.
pub fn run(args: RunEmulatorArgs) -> Result<()> {
    let root = workspace::root()?;

    setup_native::ensure_mupdf_sources(&root, false)?;
    mupdf_wrapper::build_native_if_needed(&root)?;

    let mut cargo_args = vec!["run", "-p", "emulator"];

    if let Some(features) = args.features.as_deref() {
        cargo_args.push("--features");
        cargo_args.push(features);
    }

    let extra_refs: Vec<&str> = args.extra.iter().map(String::as_str).collect();
    cargo_args.extend_from_slice(&extra_refs);

    cmd::run("cargo", &cargo_args, &root, &[])
}
