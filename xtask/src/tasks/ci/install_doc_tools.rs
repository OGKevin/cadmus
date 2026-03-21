//! `cargo xtask ci install-doc-tools` — install mdBook, mdbook-epub,
//! mdbook-mermaid, mdbook-i18n-helpers, and optionally Zola into `~/.cache/` with version pinning.
//!
//! This task is the Rust replacement for the bash install script that previously
//! lived in `.github/actions/install-doc-tools/action.yml`.  It is designed to
//! run after `actions/cache` has restored any previously cached binaries, so it
//! only downloads and builds what is missing or stale.
//!
//! ## Cache layout
//!
//! | Tool | Cache directory | Staleness marker |
//! |------|-----------------|------------------|
//! | mdBook | `~/.cache/mdbook/` | binary presence |
//! | mdbook-epub | `~/.cache/mdbook-epub/` | `~/.cache/mdbook-epub/.rev` |
//! | mdbook-mermaid | `~/.cache/mdbook-mermaid/` | `~/.cache/mdbook-mermaid/.version` |
//! | mdbook-i18n-helpers | `~/.cache/mdbook-i18n-helpers/` | `~/.cache/mdbook-i18n-helpers/.version` |
//! | Zola | `~/.cache/zola/` | binary presence |
//!
//! ## PATH update
//!
//! After installation, `~/.local/bin` is appended to the file pointed to by
//! `$GITHUB_PATH`.  This makes all four tools available to subsequent GitHub
//! Actions steps without any additional shell configuration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;

use crate::tasks::util::{cmd, fs, http};

/// Arguments for `cargo xtask ci install-doc-tools`.
#[derive(Debug, Args)]
pub struct InstallDocToolsArgs {
    /// mdBook release version to install (e.g. `"0.5.2"`).
    #[arg(long)]
    pub mdbook_version: String,

    /// Full git SHA of the `Michael-F-Bryan/mdbook-epub` commit to build.
    #[arg(long)]
    pub mdbook_epub_rev: String,

    /// mdbook-mermaid release version to install (e.g. `"0.17.0"`).
    #[arg(long)]
    pub mdbook_mermaid_version: String,

    /// mdbook-i18n-helpers release version to install (e.g. `"0.3.0"`).
    #[arg(long)]
    pub mdbook_i18n_helpers_version: String,

    /// Zola release version to install (e.g. `"0.22.1"`).
    ///
    /// When omitted, Zola installation is skipped.
    #[arg(long)]
    pub zola_version: Option<String>,
}

/// Installs doc tools and appends `~/.local/bin` to `$GITHUB_PATH`.
///
/// # Errors
///
/// Returns an error if any download, build, or installation step fails.
pub fn run(args: InstallDocToolsArgs) -> Result<()> {
    let home = home_dir()?;
    let cache = home.join(".cache");
    let local_bin = home.join(".local/bin");

    std::fs::create_dir_all(&local_bin).context("failed to create ~/.local/bin")?;

    install_mdbook(&cache, &local_bin, &args.mdbook_version)?;
    install_mdbook_epub(&cache, &local_bin, &args.mdbook_epub_rev)?;
    install_mdbook_mermaid(&cache, &local_bin, &args.mdbook_mermaid_version)?;
    install_mdbook_i18n_helpers(&cache, &local_bin, &args.mdbook_i18n_helpers_version)?;

    if let Some(ref version) = args.zola_version {
        install_zola(&cache, &local_bin, version)?;
    }

    append_to_github_path(&local_bin)?;

    println!("\nDoc tools installed successfully.");

    Ok(())
}

/// Downloads and extracts the mdBook binary if not already cached.
///
/// The binary is placed at `~/.cache/mdbook/mdbook` and symlinked into
/// `~/.local/bin/`.
fn install_mdbook(cache: &Path, local_bin: &Path, version: &str) -> Result<()> {
    let mdbook_dir = cache.join("mdbook");
    let mdbook_bin = mdbook_dir.join("mdbook");

    if mdbook_bin.exists() {
        println!("mdBook {version} already cached, skipping download.");
    } else {
        println!("Installing mdBook {version}…");
        std::fs::create_dir_all(&mdbook_dir).context("failed to create ~/.cache/mdbook")?;

        let arch = mdbook_arch();
        let url = format!(
            "https://github.com/rust-lang/mdBook/releases/download/v{version}/mdbook-v{version}-{arch}.tar.gz"
        );

        let tarball = std::env::temp_dir().join("mdbook.tar.gz");
        http::download(&url, &tarball)?;
        fs::extract_tarball(&tarball, &mdbook_dir)?;
    }

    symlink_bin(&mdbook_bin, &local_bin.join("mdbook"))
}

/// Builds and installs mdbook-epub from source if the cached revision is stale.
///
/// The binary is placed at `~/.cache/mdbook-epub/bin/mdbook-epub` and
/// symlinked into `~/.local/bin/`.  A `.rev` file records the installed SHA so
/// subsequent runs can detect staleness without rebuilding.
fn install_mdbook_epub(cache: &Path, local_bin: &Path, rev: &str) -> Result<()> {
    let epub_dir = cache.join("mdbook-epub");
    let epub_bin = epub_dir.join("bin/mdbook-epub");
    let rev_file = epub_dir.join(".rev");

    if is_current(&epub_bin, &rev_file, rev) {
        println!("mdbook-epub {rev} already cached, skipping build.");
    } else {
        println!("Building mdbook-epub @ {rev}…");
        std::fs::remove_dir_all(&epub_dir).ok();

        let tmp = std::env::temp_dir().join("mdbook-epub-src");
        std::fs::create_dir_all(&tmp).context("failed to create mdbook-epub temp dir")?;

        let tarball = tmp.join("mdbook-epub.tar.gz");
        let url = format!("https://github.com/Michael-F-Bryan/mdbook-epub/archive/{rev}.tar.gz");
        http::download(&url, &tarball)?;
        fs::extract_tarball(&tarball, &tmp)?;

        let src_dir = tmp.join(format!("mdbook-epub-{rev}"));
        cmd::run(
            "cargo",
            &[
                "install",
                "--path",
                src_dir.to_str().context("non-UTF-8 path")?,
                "--locked",
                "--root",
                epub_dir.to_str().context("non-UTF-8 path")?,
            ],
            &src_dir,
            &[],
        )?;

        std::fs::remove_dir_all(&tmp).ok();
        std::fs::write(&rev_file, rev).context("failed to write mdbook-epub .rev")?;
    }

    symlink_bin(&epub_bin, &local_bin.join("mdbook-epub"))
}

/// Installs mdbook-mermaid via `cargo install` if the cached version is stale.
///
/// The binary is placed at `~/.cache/mdbook-mermaid/bin/mdbook-mermaid` and
/// symlinked into `~/.local/bin/`.  A `.version` file records the installed
/// version.
fn install_mdbook_mermaid(cache: &Path, local_bin: &Path, version: &str) -> Result<()> {
    let mermaid_dir = cache.join("mdbook-mermaid");
    let mermaid_bin = mermaid_dir.join("bin/mdbook-mermaid");
    let version_file = mermaid_dir.join(".version");

    if is_current(&mermaid_bin, &version_file, version) {
        println!("mdbook-mermaid {version} already cached, skipping install.");
    } else {
        println!("Installing mdbook-mermaid {version}…");
        std::fs::remove_dir_all(&mermaid_dir).ok();
        std::fs::create_dir_all(mermaid_dir.join("bin"))
            .context("failed to create ~/.cache/mdbook-mermaid/bin")?;

        cmd::run(
            "cargo",
            &[
                "install",
                "mdbook-mermaid",
                "--version",
                version,
                "--root",
                mermaid_dir.to_str().context("non-UTF-8 path")?,
            ],
            Path::new("."),
            &[],
        )?;

        std::fs::write(&version_file, version)
            .context("failed to write mdbook-mermaid .version")?;
    }

    symlink_bin(&mermaid_bin, &local_bin.join("mdbook-mermaid"))
}

/// Installs mdbook-i18n-helpers via `cargo install` if the cached version is stale.
///
/// The binaries are placed at `~/.cache/mdbook-i18n-helpers/bin/` and
/// symlinked into `~/.local/bin/`.  A `.version` file records the installed
/// version.
fn install_mdbook_i18n_helpers(cache: &Path, local_bin: &Path, version: &str) -> Result<()> {
    let i18n_dir = cache.join("mdbook-i18n-helpers");
    let i18n_bin_dir = i18n_dir.join("bin");
    let version_file = i18n_dir.join(".version");

    let binaries = ["mdbook-gettext", "mdbook-xgettext", "mdbook-i18n-normalize"];

    // Check if all binaries exist and version matches
    let all_exist = binaries.iter().all(|b| i18n_bin_dir.join(b).exists());
    let version_matches = is_current(&i18n_bin_dir.join("mdbook-gettext"), &version_file, version);

    if all_exist && version_matches {
        println!("mdbook-i18n-helpers {version} already cached, skipping install.");
    } else {
        println!("Installing mdbook-i18n-helpers {version}…");
        std::fs::remove_dir_all(&i18n_dir).ok();
        std::fs::create_dir_all(&i18n_bin_dir)
            .context("failed to create ~/.cache/mdbook-i18n-helpers/bin")?;

        cmd::run(
            "cargo",
            &[
                "install",
                "mdbook-i18n-helpers",
                "--version",
                version,
                "--root",
                i18n_dir.to_str().context("non-UTF-8 path")?,
            ],
            Path::new("."),
            &[],
        )?;

        std::fs::write(&version_file, version)
            .context("failed to write mdbook-i18n-helpers .version")?;
    }

    // Symlink all binaries
    for bin in &binaries {
        let target = i18n_bin_dir.join(bin);
        let link = local_bin.join(*bin);
        symlink_bin(&target, &link)?;
    }

    Ok(())
}

/// Downloads and extracts the Zola binary if not already cached.
///
/// The binary is placed at `~/.cache/zola/zola` and symlinked into
/// `~/.local/bin/`.
fn install_zola(cache: &Path, local_bin: &Path, version: &str) -> Result<()> {
    let zola_dir = cache.join("zola");
    let zola_bin = zola_dir.join("zola");

    if zola_bin.exists() {
        println!("Zola {version} already cached, skipping download.");
    } else {
        println!("Installing Zola {version}…");
        std::fs::create_dir_all(&zola_dir).context("failed to create ~/.cache/zola")?;

        let url = format!(
            "https://github.com/getzola/zola/releases/download/v{version}/zola-v{version}-x86_64-unknown-linux-gnu.tar.gz"
        );

        let tarball = std::env::temp_dir().join("zola.tar.gz");
        http::download(&url, &tarball)?;
        fs::extract_tarball(&tarball, &zola_dir)?;
    }

    symlink_bin(&zola_bin, &local_bin.join("zola"))
}

/// Appends `path` to the file referenced by `$GITHUB_PATH`.
///
/// This is the GitHub Actions mechanism for adding directories to `PATH` for
/// subsequent steps.  When `$GITHUB_PATH` is not set (e.g. local development),
/// this function is a no-op.
fn append_to_github_path(path: &Path) -> Result<()> {
    let github_path = match std::env::var("GITHUB_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => return Ok(()),
    };

    let entry = format!("{}\n", path.display());
    std::fs::OpenOptions::new()
        .append(true)
        .open(&github_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(entry.as_bytes())
        })
        .with_context(|| format!("failed to append to GITHUB_PATH file at {github_path}"))
}

/// Returns `true` if `bin` exists and `marker` contains `expected`.
///
/// Used to decide whether a cached tool is up-to-date.
fn is_current(bin: &Path, marker: &Path, expected: &str) -> bool {
    if !bin.exists() {
        return false;
    }

    match std::fs::read_to_string(marker) {
        Ok(content) => content.trim() == expected,
        Err(_) => false,
    }
}

/// Creates a symlink at `link` pointing to `target`, replacing any existing
/// file or symlink.
fn symlink_bin(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)
            .with_context(|| format!("failed to remove {}", link.display()))?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).with_context(|| {
        format!(
            "failed to symlink {} -> {}",
            link.display(),
            target.display()
        )
    })?;

    #[cfg(not(unix))]
    std::fs::copy(target, link)
        .with_context(|| format!("failed to copy {} to {}", target.display(), link.display()))?;

    Ok(())
}

/// Returns the platform-specific architecture string used in mdBook release URLs.
fn mdbook_arch() -> &'static str {
    if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    }
}

/// Returns the user's home directory.
fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("$HOME is not set")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_current_returns_false_when_bin_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_current(
            &tmp.path().join("nonexistent"),
            &tmp.path().join(".ver"),
            "1.0"
        ));
    }

    #[test]
    fn is_current_returns_false_when_marker_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tool");
        fs::write(&bin, "").unwrap();
        assert!(!is_current(&bin, &tmp.path().join(".ver"), "1.0"));
    }

    #[test]
    fn is_current_returns_false_when_version_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tool");
        let marker = tmp.path().join(".ver");
        fs::write(&bin, "").unwrap();
        fs::write(&marker, "0.9").unwrap();
        assert!(!is_current(&bin, &marker, "1.0"));
    }

    #[test]
    fn is_current_returns_true_when_version_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tool");
        let marker = tmp.path().join(".ver");
        fs::write(&bin, "").unwrap();
        fs::write(&marker, "1.0").unwrap();
        assert!(is_current(&bin, &marker, "1.0"));
    }

    #[test]
    fn is_current_trims_trailing_newline_in_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tool");
        let marker = tmp.path().join(".ver");
        fs::write(&bin, "").unwrap();
        fs::write(&marker, "1.0\n").unwrap();
        assert!(is_current(&bin, &marker, "1.0"));
    }

    #[test]
    fn append_to_github_path_is_noop_when_env_unset() {
        unsafe { std::env::remove_var("GITHUB_PATH") };
        let result = append_to_github_path(Path::new("/some/bin"));
        assert!(result.is_ok());
    }

    #[test]
    fn append_to_github_path_writes_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path_file = tmp.path().join("GITHUB_PATH");
        fs::write(&path_file, "").unwrap();

        unsafe { std::env::set_var("GITHUB_PATH", path_file.to_str().unwrap()) };
        append_to_github_path(Path::new("/usr/local/bin")).unwrap();
        unsafe { std::env::remove_var("GITHUB_PATH") };

        let content = fs::read_to_string(&path_file).unwrap();
        assert!(content.contains("/usr/local/bin"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_bin_creates_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let link = tmp.path().join("link");
        fs::write(&target, "binary").unwrap();
        symlink_bin(&target, &link).unwrap();
        assert!(link.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_bin_replaces_existing_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target1 = tmp.path().join("target1");
        let target2 = tmp.path().join("target2");
        let link = tmp.path().join("link");
        fs::write(&target1, "v1").unwrap();
        fs::write(&target2, "v2").unwrap();
        symlink_bin(&target1, &link).unwrap();
        symlink_bin(&target2, &link).unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "v2");
    }
}
