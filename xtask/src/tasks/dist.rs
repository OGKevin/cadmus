//! `cargo xtask dist` — assemble the Kobo distribution directory.
//!
//! Copies the compiled Cadmus binary, shared libraries, scripts, fonts,
//! icons, and other assets into a `dist/` directory that mirrors the layout
//! expected on the Kobo device.
//!
//! ## Prerequisites
//!
//! - `cargo xtask build-kobo` must have been run first.
//! - `libs/` must contain the ARM shared libraries.
//! - `bin/`, `resources/`, and `hyphenation-patterns/` must exist.
//!
//! ## Output layout
//!
//! ```text
//! dist/
//! ├── cadmus                  (ARM binary)
//! ├── libs/                   (versioned .so files)
//! ├── fonts/
//! ├── icons/
//! ├── css/
//! ├── scripts/
//! ├── keyboard-layouts/
//! ├── hyphenation-patterns/
//! ├── bin/
//! ├── resources/
//! ├── Settings-sample.toml
//! ├── LICENSE
//! └── *.sh                    (contrib scripts)
//! ```

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Args;
use wildmatch::WildMatch;

use super::util::{cmd, fs, workspace};
use build_deps::build::kobo;
use build_deps::versions;

/// Arguments for `cargo xtask dist`.
#[derive(Debug, Args)]
pub struct DistArgs {
    /// Build for the test feature set (`--features test`).
    #[arg(long)]
    pub test: bool,
}

/// Assembles the Kobo distribution directory.
///
/// # Errors
///
/// Returns an error if the ARM binary or any required asset is missing.
pub fn run(args: DistArgs) -> Result<()> {
    let root = workspace::root()?;

    let binary = root.join("target/arm-unknown-linux-gnueabihf/release/cadmus");
    if !binary.exists() {
        bail!(
            "ARM binary not found at {}.\n\
             Run `cargo xtask build-kobo` first.",
            binary.display()
        );
    }

    let dist_dir = root.join("dist");
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir).context("failed to remove existing dist/")?;
    }
    std::fs::create_dir_all(&dist_dir)?;
    std::fs::create_dir_all(dist_dir.join("libs"))?;
    std::fs::create_dir_all(dist_dir.join("dictionaries"))?;

    copy_libraries(&root, &dist_dir)?;
    copy_assets(&root, &dist_dir)?;
    copy_binary(&root, &dist_dir)?;
    strip_and_patch(&root, &dist_dir)?;
    clean_user_files(&dist_dir)?;

    if args.test {
        println!("Test build assembled in dist/");
    } else {
        println!("Distribution assembled in dist/");
    }

    Ok(())
}

/// Copies ARM shared libraries from `libs/` into `dist/libs/` with versioned
/// names expected by the Kobo runtime linker.
fn copy_libraries(root: &Path, dist_dir: &Path) -> Result<()> {
    let libs_dir = root.join("libs");
    let dist_libs = dist_dir.join("libs");

    for &lib in versions::SONAMES {
        let soname = kobo::soname(&libs_dir, lib)?;
        let src = libs_dir.join(lib);
        let dest = dist_libs.join(&soname);
        std::fs::copy(&src, &dest).with_context(|| {
            format!(
                "failed to copy {} → {}\n\
                 Run `cargo xtask build-kobo` to build the libraries.",
                src.display(),
                dest.display()
            )
        })?;
    }

    Ok(())
}

/// Copies static assets (fonts, icons, scripts, etc.) into `dist/`.
fn copy_assets(root: &Path, dist_dir: &Path) -> Result<()> {
    let dirs = [
        "hyphenation-patterns",
        "keyboard-layouts",
        "bin",
        "scripts",
        "icons",
        "resources",
        "fonts",
        "css",
    ];

    for dir in dirs {
        let src = root.join(dir);
        if !src.exists() {
            let hint = if dir == "fonts" {
                "Run `cargo xtask download-fonts` to download it."
            } else {
                "Run `cargo xtask download-assets` to download it."
            };
            bail!(
                "Required asset directory '{}' not found.\n\
                 {hint}",
                src.display()
            );
        }
        fs::copy_dir_all(&src, &dist_dir.join(dir))?;
    }

    // Contrib scripts and sample config
    for entry in std::fs::read_dir(root.join("contrib"))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "sh") {
            std::fs::copy(&path, dist_dir.join(entry.file_name()))?;
        }
    }

    std::fs::copy(
        root.join("contrib/Settings-sample.toml"),
        dist_dir.join("Settings-sample.toml"),
    )?;

    std::fs::copy(root.join("LICENSE"), dist_dir.join("LICENSE"))?;

    Ok(())
}

/// Copies the compiled ARM binary into `dist/`.
fn copy_binary(root: &Path, dist_dir: &Path) -> Result<()> {
    std::fs::copy(
        root.join("target/arm-unknown-linux-gnueabihf/release/cadmus"),
        dist_dir.join("cadmus"),
    )
    .context("failed to copy cadmus binary")?;
    Ok(())
}

/// Strips debug symbols and removes RPATH from the binary and all libraries.
///
/// RPATH is removed so the libraries resolve against the device's default
/// linker search paths rather than the build host's paths.
fn strip_and_patch(root: &Path, dist_dir: &Path) -> Result<()> {
    let libs_dir = dist_dir.join("libs");
    for entry in std::fs::read_dir(&libs_dir)? {
        let path = entry?.path();
        cmd::run(
            "patchelf",
            &["--remove-rpath", &path.to_string_lossy()],
            root,
            &[],
        )?;
    }

    // Strip the binary and all libraries to reduce size.
    let binary = dist_dir.join("cadmus");
    let mut strip_targets = vec![binary.to_string_lossy().into_owned()];
    for entry in std::fs::read_dir(&libs_dir)? {
        strip_targets.push(entry?.path().to_string_lossy().into_owned());
    }

    let strip_refs: Vec<&str> = strip_targets.iter().map(String::as_str).collect();
    cmd::run("arm-linux-gnueabihf-strip", &strip_refs, root, &[])
}

/// Removes user-specific files that should not be distributed.
fn clean_user_files(dist_dir: &Path) -> Result<()> {
    let patterns: &[(&str, &str)] = &[
        ("css", "*-user.css"),
        ("keyboard-layouts", "*-user.json"),
        ("hyphenation-patterns", "*.bounds"),
        ("scripts", "wifi-*-*.sh"),
    ];

    for (subdir, pattern) in patterns {
        let dir = dist_dir.join(subdir);
        if dir.exists() {
            remove_matching(&dir, pattern)?;
        }
    }

    Ok(())
}

/// Removes files in `dir` whose names match the glob `pattern`.
fn remove_matching(dir: &Path, pattern: &str) -> Result<()> {
    let matcher = WildMatch::new(pattern);

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && matcher.matches(name)
        {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn remove_matching_deletes_only_matching_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        fs::write(dir.join("default-user.css"), b"x").unwrap();
        fs::write(dir.join("default.css"), b"x").unwrap();
        fs::write(dir.join("wifi-enable-eth0.sh"), b"x").unwrap();
        fs::write(dir.join("wifi-enable.sh"), b"x").unwrap();

        remove_matching(dir, "*-user.css").unwrap();
        remove_matching(dir, "wifi-*-*.sh").unwrap();

        assert!(!dir.join("default-user.css").exists());
        assert!(dir.join("default.css").exists());
        assert!(!dir.join("wifi-enable-eth0.sh").exists());
        assert!(dir.join("wifi-enable.sh").exists());
    }
}
