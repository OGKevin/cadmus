//! `cargo xtask install-importer` — install the Cadmus importer crate.
//!
//! Runs `cargo install --path crates/importer`.  Any extra arguments are
//! forwarded to `cargo install`.  Native dependencies are built automatically
//! by `build.rs`.

use anyhow::Result;
use clap::Args;

use super::util::{cmd, workspace};

/// Arguments for `cargo xtask install-importer`.
#[derive(Debug, Args)]
pub struct InstallImporterArgs {
    /// Extra arguments forwarded to `cargo install --path crates/importer`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

/// Installs the importer crate.
pub fn run(args: InstallImporterArgs) -> Result<()> {
    let root = workspace::root()?;

    let importer_path = root.join("crates/importer");
    let importer_str = importer_path.to_string_lossy().into_owned();

    let mut cargo_args = vec!["install", "--path", &importer_str];
    let extra_refs: Vec<&str> = args.extra.iter().map(String::as_str).collect();
    cargo_args.extend_from_slice(&extra_refs);

    cmd::run("cargo", &cargo_args, &root, &[])
}
