//! `cargo xtask docs` — build the full documentation portal.
//!
//! 1. Installs mdbook-mermaid JavaScript assets into `docs/`.
//! 2. Builds the mdBook user guide (`docs/book/html/`).
//! 3. Generates Rust API documentation (`target/doc/`).
//! 4. Optionally injects the git version string into the generated HTML.
//! 5. Creates symlinks so Zola can find the mdBook and cargo-doc outputs.
//! 6. Builds the Zola documentation portal (`docs-portal/public/`).
//!
//! ## Output
//!
//! The final portal is written to `docs-portal/public/` and is ready to be
//! deployed to Cloudflare Pages or GitHub Pages.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Args;
use serde::Deserialize;
use walkdir::WalkDir;

use super::util::{cmd, workspace};

/// Arguments for `cargo xtask docs`.
#[derive(Debug, Args)]
pub struct DocsArgs {
    /// Base URL for the Zola build (e.g. `https://cadmus-dt6.pages.dev/`).
    ///
    /// Defaults to `http://localhost` for local development.
    #[arg(long, default_value = "http://localhost")]
    pub base_url: String,

    /// Skip the Zola portal build (useful when only the mdBook output is
    /// needed, e.g. for embedding the EPUB in the binary).
    #[arg(long)]
    pub mdbook_only: bool,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    target_directory: String,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
}

/// Builds the full documentation portal.
///
/// # Errors
///
/// Returns an error if any build tool (`mdbook`, `cargo doc`, `zola`) is not
/// on `PATH` or exits with a non-zero status.
pub fn run(args: DocsArgs) -> Result<()> {
    let root = workspace::root()?;

    install_mermaid_assets(&root)?;
    build_mdbook(&root)?;

    if args.mdbook_only {
        return Ok(());
    }

    build_cargo_doc(&root)?;
    inject_git_version(&root)?;
    create_portal_symlinks(&root)?;
    build_zola(&root, &args.base_url)?;

    println!("\nDocumentation built successfully!");
    println!("Output: docs-portal/public/");

    Ok(())
}

/// Installs the Mermaid JavaScript assets required by mdbook-mermaid.
///
/// This only needs to run once (or after updating the mdbook-mermaid version).
fn install_mermaid_assets(root: &Path) -> Result<()> {
    println!("Installing mdbook-mermaid assets…");
    cmd::run("mdbook-mermaid", &["install", "docs"], root, &[])
}

/// Builds the mdBook user guide.
fn build_mdbook(root: &Path) -> Result<()> {
    println!("Building mdBook documentation…");
    cmd::run("mdbook", &["build"], &root.join("docs"), &[])
}

/// Generates Rust API documentation for all workspace crates.
fn build_cargo_doc(root: &Path) -> Result<()> {
    println!("Building Rust API documentation…");
    cmd::run(
        "cargo",
        &["doc", "--no-deps", "--document-private-items"],
        root,
        &[],
    )
}

/// Injects the git version string into the generated Rust documentation HTML.
///
/// `cargo doc` embeds the crate version from `Cargo.toml`.  This function
/// replaces that static version with the output of `git describe` so that
/// documentation built from a dirty working tree or a non-tagged commit shows
/// the exact revision.
fn inject_git_version(root: &Path) -> Result<()> {
    let workspace_version = read_workspace_version(root)?;
    let git_version = read_git_version(root)?;

    if workspace_version == git_version {
        return Ok(());
    }

    println!("Injecting git version '{git_version}' into Rust docs…");

    let doc_dir = root.join("target/doc");
    if !doc_dir.exists() {
        return Ok(());
    }

    replace_version_in_html(&doc_dir, &workspace_version, &git_version)
}

/// Reads the workspace version from the `cadmus` crate's `Cargo.toml`.
fn read_workspace_version(root: &Path) -> Result<String> {
    let metadata = cargo_metadata(root)?;
    metadata
        .packages
        .into_iter()
        .find(|p| p.name == "cadmus")
        .map(|p| p.version)
        .context("cadmus package not found in cargo metadata")
}

/// Returns the git version string (`git describe --tags --always --dirty`).
fn read_git_version(root: &Path) -> Result<String> {
    cmd::output(
        "git",
        &["describe", "--tags", "--always", "--dirty"],
        root,
        &[],
    )
}

/// Walks `doc_dir` recursively and replaces `workspace_version` with
/// `git_version` in every HTML file that contains the version span.
fn replace_version_in_html(
    doc_dir: &Path,
    workspace_version: &str,
    git_version: &str,
) -> Result<()> {
    let old = format!(r#"<span class="version">{workspace_version}</span>"#);
    let new = format!(r#"<span class="version">{git_version}</span>"#);

    for entry in WalkDir::new(doc_dir) {
        let entry = entry.with_context(|| format!("failed to walk {}", doc_dir.display()))?;
        let path = entry.path();

        if !path.extension().is_some_and(|ext| ext == "html") {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        if content.contains(&old) {
            let updated = content.replace(&old, &new);
            std::fs::write(path, updated)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
    }

    Ok(())
}

/// Creates symlinks in `docs-portal/static/` so Zola can serve the mdBook
/// and cargo-doc outputs as static assets.
fn create_portal_symlinks(root: &Path) -> Result<()> {
    let metadata = cargo_metadata(root)?;

    let api_link = root.join("docs-portal/static/api");
    let guide_link = root.join("docs-portal/static/guide");

    symlink_force(
        Path::new(&format!("{}/doc", metadata.target_directory)),
        &api_link,
    )?;
    symlink_force(&root.join("docs/book/html"), &guide_link)?;

    Ok(())
}

/// Builds the Zola documentation portal.
fn build_zola(root: &Path, base_url: &str) -> Result<()> {
    println!("Building Zola documentation portal…");
    cmd::run(
        "zola",
        &["build", "--base-url", base_url],
        &root.join("docs-portal"),
        &[],
    )
}

/// Runs `cargo metadata` and returns the parsed result.
fn cargo_metadata(root: &Path) -> Result<CargoMetadata> {
    let json = cmd::output(
        "cargo",
        &["metadata", "--format-version=1", "--no-deps"],
        root,
        &[],
    )?;
    serde_json::from_str(&json).context("failed to parse cargo metadata JSON")
}

fn symlink_force(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)
            .with_context(|| format!("failed to remove {}", link.display()))?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;

    #[cfg(not(unix))]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
                .with_context(|| format!("failed to create dir symlink {}", link.display()))?;
        } else {
            std::fs::copy(target, link)
                .with_context(|| format!("failed to copy {}", target.display()))?;
        }
    }

    Ok(())
}
