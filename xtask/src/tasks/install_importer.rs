//! `cargo xtask install-importer` — install the Cadmus importer crate.
//!
//! Ensures MuPDF sources and the `mupdf_wrapper` C library are built for the
//! native platform, then runs `cargo install --path crates/importer`.  Any
//! extra arguments are forwarded to `cargo install`.

use anyhow::Result;
use clap::Args;

use super::setup_native;
use super::util::{cmd, mupdf_wrapper, workspace};

/// Arguments for `cargo xtask install-importer`.
#[derive(Debug, Args)]
pub struct InstallImporterArgs {
    /// Extra arguments forwarded to `cargo install --path crates/importer`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

/// Ensures prerequisites are built then installs the importer.
///
/// # Errors
///
/// Returns an error if the MuPDF download, wrapper build, or installation
/// fails.
pub fn run(args: InstallImporterArgs) -> Result<()> {
    let root = workspace::root()?;

    setup_native::ensure_mupdf_sources(&root, false)?;
    mupdf_wrapper::build_native_if_needed(&root)?;

    let importer_path = root.join("crates/importer");
    let importer_str = importer_path.to_string_lossy().into_owned();

    let mut cargo_args = vec!["install", "--path", &importer_str];
    let extra_refs: Vec<&str> = args.extra.iter().map(String::as_str).collect();
    cargo_args.extend_from_slice(&extra_refs);

    cmd::run("cargo", &cargo_args, &root, &[])
}
