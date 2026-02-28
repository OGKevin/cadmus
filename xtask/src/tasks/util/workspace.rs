//! Workspace root resolution and metadata helpers.
//!
//! The xtask binary can be invoked from any directory inside the Cargo
//! workspace.  All tasks need the workspace root so they can locate source
//! files, thirdparty directories, and build outputs.

use std::path::PathBuf;

use anyhow::{Context, Result};
use toml;

/// Returns the absolute path to the Cargo workspace root.
///
/// Uses `CARGO_MANIFEST_DIR`, which Cargo sets to the `xtask/` directory when
/// building the xtask binary.  The workspace root is one level up.
///
/// # Errors
///
/// Returns an error if the workspace root cannot be located (e.g. the binary
/// is run outside the repository).
///
/// # Examples
///
/// ```no_run
/// # use std::path::PathBuf;
/// // Note: requires running inside the Cadmus workspace.
/// let root = xtask_lib::tasks::util::workspace::root().unwrap();
/// assert!(root.join("Cargo.toml").exists());
/// ```
pub fn root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set — run via `cargo xtask`")?;

    let xtask_dir = PathBuf::from(manifest_dir);
    let workspace_root = xtask_dir
        .parent()
        .context("xtask directory has no parent")?
        .to_path_buf();

    Ok(workspace_root)
}

/// Returns the version of the `cadmus` binary crate from its `Cargo.toml`.
///
/// # Errors
///
/// Returns an error if the workspace root cannot be found, the `Cargo.toml`
/// cannot be read, or the version field is missing.
pub fn current_version() -> Result<String> {
    let workspace_root = root()?;
    let cargo_toml_path = workspace_root.join("crates/cadmus/Cargo.toml");

    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;

    let doc: toml::Value = content
        .parse()
        .with_context(|| format!("failed to parse {}", cargo_toml_path.display()))?;

    doc.get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .context("version field not found in crates/cadmus/Cargo.toml")
}
