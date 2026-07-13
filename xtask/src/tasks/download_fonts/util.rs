//! Shared helpers for font sources.
//!
//! Font submodules under [`super::font`] copy files from `thirdparty/` via
//! [`install_from_submodule`].  Release archives are extracted with helpers in
//! [`crate::tasks::util::fs`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use build_deps::markers;

use crate::tasks::util::github::{self, Asset};

fn submodule_marker_dir(fonts_dir: &Path, name: &str) -> PathBuf {
    fonts_dir.join(format!(".markers/{name}"))
}

/// Returns `true` when `files` are present and the submodule gitlink matches
/// the recorded marker under `fonts_dir/.markers/{name}/`.
pub(crate) fn is_submodule_install_current(
    root: &Path,
    fonts_dir: &Path,
    submodule: &str,
    name: &str,
    files: &[(&str, &str)],
) -> bool {
    let marker_dir = submodule_marker_dir(fonts_dir, name);
    files.iter().all(|(dest, _)| fonts_dir.join(dest).exists())
        && markers::is_built(root, &marker_dir, submodule)
}

/// Copies `files` from a submodule into `fonts_dir`.
///
/// Each entry is `(dest_filename, path_relative_to_submodule_root)`. When the
/// submodule gitlink changes, managed destination files are removed and
/// recopied.
///
/// Call [`build_deps::ensure_submodules`] before invoking this function.
///
/// # Errors
///
/// Returns an error if the submodule is missing a source file or a copy fails.
pub(crate) fn install_from_submodule(
    root: &Path,
    submodule: &str,
    fonts_dir: &Path,
    name: &str,
    files: &[(&str, &str)],
) -> Result<()> {
    let marker_dir = submodule_marker_dir(fonts_dir, name);
    if is_submodule_install_current(root, fonts_dir, submodule, name, files) {
        return Ok(());
    }

    if !markers::is_built(root, &marker_dir, submodule) {
        for (dest_name, _) in files {
            let dest = fonts_dir.join(dest_name);
            if dest.exists() {
                std::fs::remove_file(&dest)
                    .with_context(|| format!("failed to remove stale font file {dest_name}"))?;
            }
        }
    }

    let submodule_root = root.join(submodule);
    if !submodule_root.is_dir() {
        anyhow::bail!(
            "{submodule} not found — run `git submodule update --init --recursive` first"
        );
    }

    for &(dest_name, rel_path) in files {
        let dest = fonts_dir.join(dest_name);
        let src = submodule_root.join(rel_path);
        if !src.is_file() {
            anyhow::bail!(
                "missing font file {} in submodule {submodule}",
                src.display()
            );
        }
        println!("Copying {dest_name} from {submodule}/{rel_path}…");
        std::fs::copy(&src, &dest)
            .with_context(|| format!("failed to copy {dest_name} from {submodule}"))?;
    }

    std::fs::create_dir_all(&marker_dir).with_context(|| {
        format!(
            "failed to create submodule marker directory {}",
            marker_dir.display()
        )
    })?;
    markers::mark_built(root, &marker_dir, name, submodule)?;
    Ok(())
}

/// Returns the path to a cached release archive, downloading it when missing.
///
/// # Errors
///
/// Returns an error if the cache directory cannot be created or the download
/// fails.
pub(crate) fn ensure_cached_release_asset(
    cache_dir: &Path,
    asset_name: &str,
    fetch: impl FnOnce() -> Result<Asset>,
) -> Result<PathBuf> {
    ensure_cached_release_asset_with(cache_dir, asset_name, fetch, github::download_asset)
}

pub(crate) fn ensure_cached_release_asset_with(
    cache_dir: &Path,
    asset_name: &str,
    fetch: impl FnOnce() -> Result<Asset>,
    download: impl FnOnce(&Asset, &Path) -> Result<()>,
) -> Result<PathBuf> {
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create cache dir {}", cache_dir.display()))?;

    let archive = cache_dir.join(asset_name);
    if !archive.exists() {
        let asset = fetch()?;
        println!("Downloading {asset_name}…");
        download(&asset, &archive).with_context(|| format!("failed to download {asset_name}"))?;
    } else {
        println!("Using cached {asset_name}");
    }

    Ok(archive)
}

/// Extracts a cached archive, re-downloading once when extraction fails.
///
/// # Errors
///
/// Returns an error if download or extraction fails after one retry.
pub(crate) fn extract_cached_archive<F>(
    archive: &Path,
    redownload: impl FnOnce() -> Result<()>,
    extract: F,
) -> Result<()>
where
    F: Fn(&Path) -> Result<()>,
{
    match extract(archive) {
        Ok(()) => Ok(()),
        Err(first) => {
            std::fs::remove_file(archive).ok();
            redownload().with_context(|| {
                format!(
                    "failed to re-download {} after corrupt cache",
                    archive.display()
                )
            })?;
            extract(archive).with_context(|| {
                format!(
                    "failed to extract {} after re-download: {first:#}",
                    archive.display()
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::tasks::util::fs::extract_zip_matching_flat;
    use crate::tasks::util::github::Asset;
    use crate::tasks::util::workspace;

    const GOOGLE_SUBMODULE: &str = "thirdparty/google-fonts";
    const GOOGLE_MARKER: &str = "google-fonts";
    const TEST_FILE: (&str, &str) = (
        "VarelaRound-Regular.ttf",
        "ofl/varelaround/VarelaRound-Regular.ttf",
    );

    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, contents).unwrap();
        }

        writer.finish().unwrap();
    }

    fn dummy_asset() -> Asset {
        Asset {
            browser_download_url: "https://example.com/asset.zip".to_owned(),
            name: "asset.zip".to_owned(),
            digest: None,
        }
    }

    fn google_marker_dir(fonts_dir: &Path) -> PathBuf {
        fonts_dir.join(format!(".markers/{GOOGLE_MARKER}"))
    }

    fn write_google_marker(fonts_dir: &Path, sha: &str) {
        let marker_dir = google_marker_dir(fonts_dir);
        fs::create_dir_all(&marker_dir).unwrap();
        markers::mark_version(&marker_dir, GOOGLE_MARKER, sha).unwrap();
    }

    #[test]
    fn extract_succeeds_on_valid_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("fonts.zip");
        write_test_zip(&archive, &[("Libron-Regular.ttf", b"font")]);

        let extract_dir = tmp.path().join("out");
        extract_cached_archive(
            &archive,
            || Ok(()),
            |path| extract_zip_matching_flat(path, &extract_dir, "", ".ttf"),
        )
        .unwrap();

        assert!(archive.exists());
        assert_eq!(
            fs::read(extract_dir.join("Libron-Regular.ttf")).unwrap(),
            b"font"
        );
    }

    #[test]
    fn extract_retries_after_corrupt_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("fonts.zip");
        fs::write(&archive, b"not a zip").unwrap();

        let extract_dir = tmp.path().join("out");
        extract_cached_archive(
            &archive,
            || {
                write_test_zip(&archive, &[("Libron-Regular.ttf", b"fresh")]);
                Ok(())
            },
            |path| extract_zip_matching_flat(path, &extract_dir, "", ".ttf"),
        )
        .unwrap();

        assert_eq!(
            fs::read(extract_dir.join("Libron-Regular.ttf")).unwrap(),
            b"fresh"
        );
    }

    #[test]
    fn ensure_cached_uses_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let asset_name = "other-core-fonts.zip";
        let archive = cache_dir.join(asset_name);
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(&archive, b"cached").unwrap();

        let fetch_called = Cell::new(false);
        let path = ensure_cached_release_asset_with(
            &cache_dir,
            asset_name,
            || {
                fetch_called.set(true);
                Ok(dummy_asset())
            },
            |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(path, archive);
        assert!(!fetch_called.get());
        assert_eq!(fs::read(&archive).unwrap(), b"cached");
    }

    #[test]
    fn ensure_cached_downloads_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let asset_name = "other-core-fonts.zip";
        let fixture = tmp.path().join("fixture.zip");
        write_test_zip(&fixture, &[("Libron-Regular.ttf", b"downloaded")]);

        let fetch_called = Cell::new(false);
        let path = ensure_cached_release_asset_with(
            &cache_dir,
            asset_name,
            || {
                fetch_called.set(true);
                Ok(dummy_asset())
            },
            |_, dest| {
                fs::copy(&fixture, dest)?;
                Ok(())
            },
        )
        .unwrap();

        assert!(fetch_called.get());
        assert_eq!(path, cache_dir.join(asset_name));
        assert!(path.exists());
    }

    #[test]
    fn is_submodule_install_current_false_without_files() {
        let root = workspace::root().unwrap();
        let sha = markers::submodule_commit(&root, GOOGLE_SUBMODULE).unwrap();
        let fonts_dir = tempfile::tempdir().unwrap();
        write_google_marker(fonts_dir.path(), &sha);

        assert!(!is_submodule_install_current(
            &root,
            fonts_dir.path(),
            GOOGLE_SUBMODULE,
            GOOGLE_MARKER,
            &[TEST_FILE],
        ));
    }

    #[test]
    fn is_submodule_install_current_false_with_stale_marker() {
        let root = workspace::root().unwrap();
        let fonts_dir = tempfile::tempdir().unwrap();
        fs::write(fonts_dir.path().join(TEST_FILE.0), b"font").unwrap();
        write_google_marker(fonts_dir.path(), "0".repeat(40).as_str());

        assert!(!is_submodule_install_current(
            &root,
            fonts_dir.path(),
            GOOGLE_SUBMODULE,
            GOOGLE_MARKER,
            &[TEST_FILE],
        ));
    }

    #[test]
    #[ignore = "requires initialized thirdparty/google-fonts submodule"]
    fn install_from_submodule_copies_and_skips() {
        let root = workspace::root().unwrap();
        let fonts_dir = tempfile::tempdir().unwrap();
        let files = &[TEST_FILE];

        install_from_submodule(
            &root,
            GOOGLE_SUBMODULE,
            fonts_dir.path(),
            GOOGLE_MARKER,
            files,
        )
        .unwrap();
        assert!(fonts_dir.path().join(TEST_FILE.0).exists());

        fs::write(fonts_dir.path().join(TEST_FILE.0), b"junk").unwrap();
        install_from_submodule(
            &root,
            GOOGLE_SUBMODULE,
            fonts_dir.path(),
            GOOGLE_MARKER,
            files,
        )
        .unwrap();
        assert_eq!(
            fs::read(fonts_dir.path().join(TEST_FILE.0)).unwrap(),
            b"junk"
        );
        assert!(is_submodule_install_current(
            &root,
            fonts_dir.path(),
            GOOGLE_SUBMODULE,
            GOOGLE_MARKER,
            files,
        ));
    }

    #[test]
    #[ignore = "requires initialized thirdparty/google-fonts submodule"]
    fn install_from_submodule_removes_stale_dest_on_revision_change() {
        let root = workspace::root().unwrap();
        let fonts_dir = tempfile::tempdir().unwrap();
        let files = &[TEST_FILE];
        let expected = fs::read(root.join(GOOGLE_SUBMODULE).join(TEST_FILE.1)).unwrap();

        install_from_submodule(
            &root,
            GOOGLE_SUBMODULE,
            fonts_dir.path(),
            GOOGLE_MARKER,
            files,
        )
        .unwrap();

        write_google_marker(fonts_dir.path(), "0".repeat(40).as_str());
        fs::write(fonts_dir.path().join(TEST_FILE.0), b"junk").unwrap();

        install_from_submodule(
            &root,
            GOOGLE_SUBMODULE,
            fonts_dir.path(),
            GOOGLE_MARKER,
            files,
        )
        .unwrap();

        assert_eq!(
            fs::read(fonts_dir.path().join(TEST_FILE.0)).unwrap(),
            expected
        );
    }
}
