//! `cargo xtask run-emulator` — run the Cadmus emulator.
//!
//! Ensures the embedded documentation EPUB is ready, then launches
//! `cargo run -p cadmus --features emulator`. Any extra arguments are forwarded
//! to the cargo invocation. Native dependencies (MuPDF, libwebp) are built
//! automatically by `build.rs`.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use clap::Args;

use super::docs::{self, DocsArgs};
use super::util::{cmd, workspace};

/// Arguments for `cargo xtask run-emulator`.
#[derive(Debug, Args)]
pub struct RunEmulatorArgs {
    /// Cargo feature flags forwarded to `cargo run -p cadmus` (emulator is always enabled).
    #[arg(long)]
    pub features: Option<String>,

    /// Extra arguments forwarded to `cargo run -p cadmus`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

/// Returns `true` when the documentation EPUB embedded by `cadmus-core` exists.
fn mdbook_epub_built(root: &Path) -> bool {
    root.join("docs/book/epub/Cadmus Documentation.epub")
        .exists()
}

fn emulator_features(extra: Option<&str>) -> String {
    let mut features = BTreeSet::from(["emulator"]);
    if let Some(extra) = extra {
        for part in extra.split([',', '+']) {
            let part = part.trim();
            if !part.is_empty() {
                features.insert(part);
            }
        }
    }
    features.into_iter().collect::<Vec<_>>().join(",")
}

/// Ensures documentation is built then launches the emulator.
pub fn run(args: RunEmulatorArgs) -> Result<()> {
    let root = workspace::root()?;

    if !mdbook_epub_built(&root) {
        println!("Documentation EPUB not found — building mdBook…");
        docs::run(DocsArgs { mdbook_only: true })?;
    }

    let features = emulator_features(args.features.as_deref());
    let mut cargo_args = vec!["run", "-p", "cadmus", "--features", features.as_str()];

    let extra_refs: Vec<&str> = args.extra.iter().map(String::as_str).collect();
    cargo_args.extend_from_slice(&extra_refs);

    cmd::run("cargo", &cargo_args, &root, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulator_features_defaults_to_emulator() {
        assert_eq!(emulator_features(None), "emulator");
    }

    #[test]
    fn emulator_features_merges_extra_features() {
        assert_eq!(
            emulator_features(Some("telemetry,test")),
            "emulator,telemetry,test"
        );
    }

    #[test]
    fn emulator_features_deduplicates_emulator() {
        assert_eq!(emulator_features(Some("emulator,test")), "emulator,test");
    }
}
