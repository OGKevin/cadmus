//! `cargo xtask run-emulator` — run the Cadmus emulator.
//!
//! Ensures the native MuPDF build and the embedded documentation EPUB are
//! ready, then launches `cargo run -p emulator`.  Any extra arguments are
//! forwarded to the emulator.

use std::path::Path;

use anyhow::Result;
use clap::Args;

use super::docs::{self, DocsArgs};
use super::setup_native::{self, SetupNativeArgs};
use super::util::{cmd, workspace};

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

/// Returns `true` when the documentation EPUB embedded by `cadmus-core` exists.
fn mdbook_epub_built(root: &Path) -> bool {
    root.join("docs/book/epub/Cadmus Documentation.epub")
        .exists()
}

/// Ensures prerequisites are built then launches the emulator.
///
/// # Errors
///
/// Returns an error if the native setup, documentation build, or emulator
/// launch fails.
pub fn run(args: RunEmulatorArgs) -> Result<()> {
    let root = workspace::root()?;

    if !setup_native::native_setup_done(&root) {
        println!("Native setup not found — running setup-native…");
        setup_native::run(SetupNativeArgs { force: false })?;
    }

    if !mdbook_epub_built(&root) {
        println!("Documentation EPUB not found — building mdBook…");
        docs::run(DocsArgs {
            base_url: "http://localhost".to_string(),
            mdbook_only: true,
        })?;
    }

    let mut cargo_args = vec!["run", "-p", "emulator"];

    if let Some(features) = args.features.as_deref() {
        cargo_args.push("--features");
        cargo_args.push(features);
    }

    let extra_refs: Vec<&str> = args.extra.iter().map(String::as_str).collect();
    cargo_args.extend_from_slice(&extra_refs);

    cmd::run("cargo", &cargo_args, &root, &[])
}
