//! `cargo xtask docs` — build the Cadmus documentation website.
//!
//! 1. Installs mdbook-mermaid JavaScript assets into `docs/`.
//! 2. Builds the mdBook user guide (`docs/book/html/`).
//! 3. Builds translated mdBook books for each locale found in `docs/po/`.
//! 4. Generates Rust API documentation (`target/doc/`).
//! 5. Optionally injects the git version string into the generated HTML.
//! 6. Writes `website/public/_shared/locales.json` with available locales.
//! 7. Runs website prebuild scripts (`generate-version.mjs`, `generate-locales.mjs`).
//! 8. Builds Storybook (`website/storybook-static/`).
//! 9. Creates symlinks in `website/public/` so Next.js includes mdBook,
//!    cargo-doc, and Storybook outputs in the static export under locale-first
//!    paths.
//! 10. Builds the Next.js website (`website/out/`).
//! 11. Deduplicates API and Storybook copies in `website/out/` via symlinks.
//!
//! ## Redirects for legacy paths (`/guide/`, `/api/`, …)
//!
//! Two mechanisms cover the two deployment targets:
//!
//! - `website/public/_redirects` — server-side 302 rules for Cloudflare Pages
//!   (and `wrangler pages dev`).  Committed to git.
//! - [`write_redirect_html`] — static HTML meta-refresh/JS redirects written to
//!   `public/guide/index.html`, etc.  Required for GitHub Pages, which ignores
//!   `_redirects`.  Generated at build time alongside the symlinks.
//!
//! ## Output
//!
//! The final website is written to `website/out/` and is ready to be deployed to
//! Cloudflare Pages or GitHub Pages.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Args;
use serde::Deserialize;
use walkdir::WalkDir;

use super::util::{cmd, workspace};

/// Locale code and display label written to `website/public/_shared/locales.json`.
#[derive(Debug, serde::Serialize)]
struct LocaleEntry {
    code: String,
    label: String,
}

/// Arguments for `cargo xtask docs`.
#[derive(Debug, Args)]
pub struct DocsArgs {
    /// Skip the website build (useful when only the mdBook output is
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

/// Builds the Cadmus documentation website.
///
/// # Errors
///
/// Returns an error if any build tool (`mdbook`, `cargo doc`, `npm`) is not
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
    run_website_prebuild(&root)?;
    build_storybook(&root)?;
    create_website_symlinks(&root)?;
    build_website(&root)?;

    println!("\nDocumentation built successfully!");
    println!("Output: website/out/");

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
        &[("CADMUS_SKIP_THIRDPARTY_DEPS", "1")],
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

        if path.extension().is_none_or(|ext| ext != "html") {
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

/// Builds Storybook into `website/storybook-static/`.
///
/// Must run before [`create_website_symlinks`] so locale storybook paths exist
/// when Next.js performs the static export.
fn build_storybook(root: &Path) -> Result<()> {
    println!("Building Storybook…");
    cmd::run(
        "npm",
        &["run", "build-storybook"],
        &root.join("website"),
        &[],
    )
}

/// Runs the website prebuild scripts that generate version and locale files.
fn run_website_prebuild(root: &Path) -> Result<()> {
    let website = root.join("website");
    cmd::run("node", &["scripts/generate-version.mjs"], &website, &[])?;
    cmd::run("node", &["scripts/generate-locales.mjs"], &website, &[])
}

/// Builds the Next.js website.
///
/// Runs `next build` inside `website/`.  The static export lands in
/// `website/out/` and includes the mdBook, cargo-doc, and Storybook outputs
/// via the symlinks created by [`create_website_symlinks`].
fn build_website(root: &Path) -> Result<()> {
    println!("Building Next.js website…");
    cmd::run("npx", &["next", "build"], &root.join("website"), &[])?;
    deduplicate_website_out(root)?;
    Ok(())
}

/// Runs `cargo metadata` and returns the parsed workspace metadata.
fn cargo_metadata(root: &Path) -> Result<CargoMetadata> {
    let json = cmd::output(
        "cargo",
        &["metadata", "--format-version=1", "--no-deps"],
        root,
        &[],
    )?;
    serde_json::from_str(&json).context("failed to parse cargo metadata JSON")
}

/// Removes a file, symlink, or directory tree at `path`.
fn remove_path_all(path: &Path) -> Result<()> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to stat {}", path.display()));
        }
    };

    if metadata.file_type().is_symlink() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove symlink {}", path.display()))?;
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
    } else {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove file {}", path.display()))?;
    }

    Ok(())
}

/// Creates `link` pointing at `target`, replacing any existing entry at `link`.
fn symlink_force(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        remove_path_all(link)?;
    }

    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;
    Ok(())
}

/// Creates `link` as a symlink with a relative target (e.g. `../_shared/api`).
fn symlink_relative(target: &str, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        remove_path_all(link)?;
    }

    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;
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

/// Writes `website/public/_shared/locales.json` for the mdBook language picker.
///
/// Locale codes come from [`scan_website_locales`].  Labels are the codes
/// themselves (e.g. `"fr"`), not translated display names.
fn write_locales_json(root: &Path) -> Result<()> {
    let website_locales = scan_website_locales(root)?;
    let locales: Vec<LocaleEntry> = website_locales
        .into_iter()
        .map(|code| LocaleEntry {
            label: code.clone(),
            code,
        })
        .collect();

    let output_path = root.join("website/public/_shared/locales.json");
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&locales)?;
    std::fs::write(&output_path, json).context("failed to write locales.json")?;

    println!(
        "Wrote {} locales to {}",
        locales.len(),
        output_path.display()
    );
    Ok(())
}

/// Returns locale codes for the static website export.
///
/// Always includes `en`.  Additional codes are derived from `docs/po/*.po`,
/// matching the mdBook translation set and `website/scripts/generate-locales.mjs`.
fn scan_website_locales(root: &Path) -> Result<Vec<String>> {
    let mut locales = vec!["en".to_string()];
    let po_dir = root.join("docs/po");

    if po_dir.exists() {
        for entry in WalkDir::new(&po_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if let Some(code) = path
                .extension()
                .filter(|ext| *ext == "po")
                .and_then(|_| path.file_stem())
                .and_then(|s| s.to_str())
                .filter(|code| *code != "en" && !locales.iter().any(|locale| locale == code))
            {
                locales.push(code.to_string());
            }
        }
    }

    locales.sort();
    Ok(locales)
}

/// Creates symlinks in `website/public/` so Next.js includes the mdBook and
/// cargo-doc outputs in the static export under locale-first paths.
///
/// These paths are gitignored in `website/.gitignore` — they are generated at
/// build time and must not be committed.
fn create_website_symlinks(root: &Path) -> Result<()> {
    let metadata = cargo_metadata(root)?;
    let website_locales = scan_website_locales(root)?;
    let public_dir = root.join("website/public");
    let shared_dir = public_dir.join("_shared");
    std::fs::create_dir_all(&shared_dir)?;

    for legacy in ["api", "guide", "storybook"] {
        let legacy_path = public_dir.join(legacy);
        if legacy_path.exists() || legacy_path.symlink_metadata().is_ok() {
            remove_path_all(&legacy_path)?;
        }
    }

    let api_target_dir = format!("{}/doc", metadata.target_directory);
    let api_target = Path::new(&api_target_dir);
    symlink_force(api_target, &shared_dir.join("api"))?;

    let storybook_target = root.join("website/storybook-static");
    let storybook_shared = shared_dir.join("storybook");
    if storybook_target.exists() {
        symlink_force(&storybook_target, &storybook_shared)?;
    }

    for locale in &website_locales {
        let locale_dir = public_dir.join(locale);
        std::fs::create_dir_all(&locale_dir)?;

        let guide_target = if locale == "en" {
            root.join("docs/book/html")
        } else {
            root.join("docs/book").join(locale).join("html")
        };

        if guide_target.exists() {
            symlink_force(&guide_target, &locale_dir.join("guide"))?;
        }

        symlink_relative("../_shared/api", &locale_dir.join("api"))?;
        if storybook_shared.exists() {
            symlink_relative("../_shared/storybook", &locale_dir.join("storybook"))?;
        }
    }

    create_back_compat_redirects(root, &website_locales)?;
    Ok(())
}

/// Writes static HTML redirects for hosts that do not support `_redirects`
/// (e.g. GitHub Pages).
fn create_back_compat_redirects(root: &Path, website_locales: &[String]) -> Result<()> {
    let public_dir = root.join("website/public");
    write_redirect_html(&public_dir.join("guide/index.html"), "/en/guide/")?;

    for locale in website_locales {
        if locale == "en" {
            continue;
        }
        write_redirect_html(
            &public_dir.join("guide").join(locale).join("index.html"),
            &format!("/{locale}/guide/"),
        )?;
    }

    write_redirect_html(&public_dir.join("api/index.html"), "/en/api/cadmus_core/")?;
    write_redirect_html(&public_dir.join("storybook/index.html"), "/en/storybook/")?;
    Ok(())
}

/// Writes a static HTML page that redirects to `target` via meta refresh and
/// JavaScript. Used for hosts that do not support `website/public/_redirects`.
fn write_redirect_html(path: &Path, target: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let base_path = std::env::var("NEXT_PUBLIC_BASE_PATH").unwrap_or_default();
    let destination = format!("{base_path}{target}");
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url={destination}">
    <link rel="canonical" href="{destination}">
    <script>location.replace("{destination}");</script>
  </head>
  <body><p><a href="{destination}">Redirecting…</a></p></body>
</html>
"#
    );
    std::fs::write(path, html).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Replaces duplicated API and Storybook trees in `website/out/` with symlinks
/// to `website/out/_shared/` after `next build` copies symlink targets.
fn deduplicate_website_out(root: &Path) -> Result<()> {
    let out_dir = root.join("website/out");
    if !out_dir.exists() {
        return Ok(());
    }

    let website_locales = scan_website_locales(root)?;
    let shared_out = out_dir.join("_shared");
    std::fs::create_dir_all(&shared_out)?;

    for asset in ["api", "storybook"] {
        let shared_asset = shared_out.join(asset);
        if shared_asset.exists() {
            continue;
        }

        for locale in &website_locales {
            let source = out_dir.join(locale).join(asset);
            if source.exists() {
                std::fs::rename(&source, &shared_asset).with_context(|| {
                    format!("failed to move {} to shared output", source.display())
                })?;
                break;
            }
        }
    }

    for locale in &website_locales {
        for asset in ["api", "storybook"] {
            let link_path = out_dir.join(locale).join(asset);
            if link_path.exists() || link_path.symlink_metadata().is_ok() {
                remove_path_all(&link_path)?;
            }

            if shared_out.join(asset).exists() {
                symlink_relative(&format!("../_shared/{asset}"), &link_path)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup_po_locales(root: &Path, locales: &[&str]) {
        fs::create_dir_all(root.join("docs/po")).unwrap();
        for locale in locales {
            if *locale == "en" {
                continue;
            }
            fs::write(
                root.join("docs/po").join(format!("{locale}.po")),
                "msgid \"\"\nmsgstr \"\"\n",
            )
            .unwrap();
        }
    }

    fn setup_duplicate_asset_trees(root: &Path, locales: &[&str], asset: &str) {
        for locale in locales {
            let asset_dir = root.join("website/out").join(locale).join(asset);
            fs::create_dir_all(&asset_dir).unwrap();
            fs::write(asset_dir.join("index.html"), format!("{locale}-{asset}")).unwrap();
        }
    }

    fn assert_symlink_to(link: &Path, expected_target: &str) {
        let metadata = link
            .symlink_metadata()
            .unwrap_or_else(|_| panic!("expected symlink at {}", link.display()));
        assert!(
            metadata.file_type().is_symlink(),
            "expected {} to be a symlink",
            link.display()
        );
        assert_eq!(
            fs::read_link(link).unwrap(),
            PathBuf::from(expected_target),
            "symlink target mismatch for {}",
            link.display()
        );
    }

    #[test]
    fn scan_website_locales_includes_po_locales() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("docs/po")).unwrap();
        fs::write(root.join("docs/po/fr.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let locales = scan_website_locales(root).unwrap();
        assert_eq!(locales, vec!["en".to_string(), "fr".to_string()]);
    }

    #[test]
    fn deduplicate_website_out_moves_shared_tree_and_creates_locale_symlinks() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        setup_po_locales(root, &["en", "fr"]);
        setup_duplicate_asset_trees(root, &["en", "fr"], "api");

        deduplicate_website_out(root).unwrap();

        let shared_api = root.join("website/out/_shared/api");
        assert!(shared_api.is_dir());
        assert_eq!(
            fs::read_to_string(shared_api.join("index.html")).unwrap(),
            "en-api"
        );

        for locale in ["en", "fr"] {
            let link = root.join("website/out").join(locale).join("api");
            assert_symlink_to(&link, "../_shared/api");
        }
    }

    #[test]
    fn deduplicate_website_out_is_idempotent_when_symlinks_exist() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        setup_po_locales(root, &["en", "fr"]);
        setup_duplicate_asset_trees(root, &["en", "fr"], "api");

        deduplicate_website_out(root).unwrap();
        deduplicate_website_out(root).unwrap();

        let shared_api = root.join("website/out/_shared/api");
        assert!(shared_api.is_dir());
        assert_eq!(
            fs::read_to_string(shared_api.join("index.html")).unwrap(),
            "en-api"
        );

        for locale in ["en", "fr"] {
            let link = root.join("website/out").join(locale).join("api");
            assert_symlink_to(&link, "../_shared/api");
        }
    }

    #[test]
    fn symlink_force_replaces_existing_destination() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let first_target = root.join("first");
        let second_target = root.join("second");
        let link = root.join("link");

        fs::create_dir_all(&first_target).unwrap();
        fs::write(first_target.join("marker"), "first").unwrap();
        fs::create_dir_all(&second_target).unwrap();
        fs::write(second_target.join("marker"), "second").unwrap();

        symlink_force(&first_target, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), first_target);

        symlink_force(&second_target, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), second_target);
        assert!(link.is_symlink());
    }

    #[test]
    fn symlink_relative_replaces_existing_destination() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let locale_dir = root.join("locale");
        fs::create_dir_all(&locale_dir).unwrap();

        let link = locale_dir.join("api");
        symlink_relative("../_shared/first", &link).unwrap();
        assert_symlink_to(&link, "../_shared/first");

        symlink_relative("../_shared/second", &link).unwrap();
        assert_symlink_to(&link, "../_shared/second");
    }

    #[test]
    fn remove_path_all_removes_file_directory_and_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let file_path = root.join("file.txt");
        fs::write(&file_path, "file").unwrap();
        remove_path_all(&file_path).unwrap();
        assert!(!file_path.exists());

        let dir_path = root.join("dir");
        fs::create_dir_all(dir_path.join("nested")).unwrap();
        fs::write(dir_path.join("nested/file.txt"), "nested").unwrap();
        remove_path_all(&dir_path).unwrap();
        assert!(!dir_path.exists());

        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();
        let symlink_path = root.join("symlink");
        std::os::unix::fs::symlink(&target, &symlink_path).unwrap();
        remove_path_all(&symlink_path).unwrap();
        assert!(!symlink_path.exists());
        assert!(symlink_path.symlink_metadata().is_err());
        assert!(target.is_dir());

        let broken_symlink = root.join("broken");
        std::os::unix::fs::symlink(root.join("missing"), &broken_symlink).unwrap();
        remove_path_all(&broken_symlink).unwrap();
        assert!(broken_symlink.symlink_metadata().is_err());
    }
}
