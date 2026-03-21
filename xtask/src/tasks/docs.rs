//! `cargo xtask docs` — build the full documentation portal.
//!
//! 1. Installs mdbook-mermaid JavaScript assets into `docs/`.
//! 2. Builds the mdBook user guide (`docs/book/html/`).
//! 3. Builds translated mdBook books for each locale found in `docs/po/`.
//! 4. Generates Rust API documentation (`target/doc/`).
//! 5. Optionally injects the git version string into the generated HTML.
//! 6. Writes `locales.json` with available locales.
//! 7. Creates symlinks so Zola can find the mdBook and cargo-doc outputs.
//! 8. Builds the Zola documentation portal (`docs-portal/public/`).
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

/// Represents a locale entry in the locales.json file.
#[derive(Debug, serde::Serialize)]
struct LocaleEntry {
    code: String,
    label: String,
}

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
    build_translated_books(&root)?;

    if args.mdbook_only {
        return Ok(());
    }

    build_cargo_doc(&root)?;
    inject_git_version(&root)?;
    write_locales_json(&root)?;
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

/// Builds translated mdBook books for each locale found in `docs/po/`.
fn build_translated_books(root: &Path) -> Result<()> {
    let po_dir = root.join("docs/po");
    if !po_dir.exists() {
        println!("No PO directory found, skipping translated books.");
        return Ok(());
    }

    println!("Building translated books…");
    for entry in WalkDir::new(&po_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "po") {
            let lang = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Invalid locale filename")?;

            println!("Building {lang} translation…");
            cmd::run(
                "mdbook",
                &["build", "-d", &format!("book/{lang}")],
                &root.join("docs"),
                &[("MDBOOK_BOOK__LANGUAGE", lang)],
            )?;
        }
    }

    Ok(())
}

/// Writes `docs/book/html/locales.json` with available locales and their display names.
fn write_locales_json(root: &Path) -> Result<()> {
    let po_dir = root.join("docs/po");
    if !po_dir.exists() {
        return Ok(());
    }

    let mut locales = Vec::new();
    for entry in WalkDir::new(&po_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "po") {
            let lang = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Invalid locale filename")?;

            let label = extract_lang_name(path).unwrap_or_else(|| lang.to_string());
            locales.push(LocaleEntry {
                code: lang.to_string(),
                label,
            });
        }
    }

    // Sort locales by code for deterministic output
    locales.sort_by(|a, b| a.code.cmp(&b.code));

    let output_path = root.join("docs/book/html/locales.json");
    std::fs::create_dir_all(output_path.parent().context("no parent dir")?)?;
    let json = serde_json::to_string_pretty(&locales)?;
    std::fs::write(&output_path, json).context("failed to write locales.json")?;

    println!(
        "Wrote {} locales to {}",
        locales.len(),
        output_path.display()
    );
    Ok(())
}

/// Extracts the display name from the PO file header.
fn extract_lang_name(po_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(po_path).ok()?;
    for line in content.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("Language-Name:") {
            let name = rest.trim();
            if name.is_empty() {
                return None;
            } else {
                return Some(name.to_string());
            }
        }
    }
    None
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

    // Create locale symlinks
    let po_dir = root.join("docs/po");
    if po_dir.exists() {
        for entry in WalkDir::new(&po_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "po") {
                let lang = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("Invalid locale filename")?;

                let target = root.join("docs/book").join(lang).join("html");
                let link = root.join("docs-portal/static/guide").join(lang);
                symlink_force(&target, &link)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn extract_lang_name_with_valid_header() {
        let temp_dir = TempDir::new().unwrap();
        let po_file = temp_dir.path().join("test.po");
        fs::write(
            &po_file,
            r#"msgid ""
msgstr ""
Language-Name: Español

msgid "hello"
msgstr "hola"
"#,
        )
        .unwrap();

        let result = extract_lang_name(&po_file);
        assert_eq!(result, Some("Español".to_string()));
    }

    #[test]
    fn extract_lang_name_with_leading_whitespace() {
        let temp_dir = TempDir::new().unwrap();
        let po_file = temp_dir.path().join("test.po");
        fs::write(
            &po_file,
            r#"msgid ""
msgstr ""
Language-Name:   Français

msgid "hello"
msgstr "bonjour"
"#,
        )
        .unwrap();

        let result = extract_lang_name(&po_file);
        assert_eq!(result, Some("Français".to_string()));
    }

    #[test]
    fn extract_lang_name_missing_header() {
        let temp_dir = TempDir::new().unwrap();
        let po_file = temp_dir.path().join("test.po");
        fs::write(
            &po_file,
            r#"msgid "hello"
msgstr "hola"
"#,
        )
        .unwrap();

        let result = extract_lang_name(&po_file);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_lang_name_empty_language_name() {
        let temp_dir = TempDir::new().unwrap();
        let po_file = temp_dir.path().join("test.po");
        fs::write(
            &po_file,
            r#"msgid ""
msgstr ""
Language-Name:

msgid "hello"
msgstr "hola"
"#,
        )
        .unwrap();

        let result = extract_lang_name(&po_file);
        assert_eq!(result, None);
    }
}
