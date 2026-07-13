use std::path::Path;

use anyhow::{Context, Result};
use build_deps::markers;

use crate::tasks::download_fonts::util;
use crate::tasks::util::{fs, github};

const REPO: &str = "nicoverbruggen/ebook-fonts";
/// Tracked by Renovate via a regex manager in `renovate.json`.
pub const EBOOK_FONTS_VERSION: &str = "v2026.07.02";
const CORE_ASSET: &str = "other-core-fonts.zip";
const EXTRA_ASSET: &str = "other-extra-fonts.zip";

const CORE_FILES: &[&str] = &[
    "Cartisse-Bold.ttf",
    "Cartisse-BoldItalic.ttf",
    "Cartisse-Italic.ttf",
    "Cartisse-Regular.ttf",
    "Libron-Bold.ttf",
    "Libron-BoldItalic.ttf",
    "Libron-Italic.ttf",
    "Libron-Regular.ttf",
    "NV_Bitter-Bold.ttf",
    "NV_Bitter-BoldItalic.ttf",
    "NV_Bitter-Italic.ttf",
    "NV_Bitter-Regular.ttf",
    "NV_Charis-Bold.ttf",
    "NV_Charis-BoldItalic.ttf",
    "NV_Charis-Italic.ttf",
    "NV_Charis-Regular.ttf",
    "NV_Garamond-Bold.ttf",
    "NV_Garamond-BoldItalic.ttf",
    "NV_Garamond-Italic.ttf",
    "NV_Garamond-Regular.ttf",
    "NV_Jost-Bold.ttf",
    "NV_Jost-BoldItalic.ttf",
    "NV_Jost-Italic.ttf",
    "NV_Jost-Regular.ttf",
    "NV_Legible_Next-Bold.ttf",
    "NV_Legible_Next-BoldItalic.ttf",
    "NV_Legible_Next-Italic.ttf",
    "NV_Legible_Next-Regular.ttf",
    "NV_Palatium-Bold.ttf",
    "NV_Palatium-BoldItalic.ttf",
    "NV_Palatium-Italic.ttf",
    "NV_Palatium-Regular.ttf",
    "Sourcerer-Bold.ttf",
    "Sourcerer-BoldItalic.ttf",
    "Sourcerer-Italic.ttf",
    "Sourcerer-Regular.ttf",
];

const EXTRA_FILES: &[&str] = &[
    "NV_Libertinus-Bold.ttf",
    "NV_Libertinus-BoldItalic.ttf",
    "NV_Libertinus-Italic.ttf",
    "NV_Libertinus-Regular.ttf",
];

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    if is_complete(fonts_dir) {
        return Ok(());
    }

    if !markers::is_version_current(fonts_dir, EBOOK_FONTS_VERSION) {
        remove_managed_files(fonts_dir)?;
    }

    let cache_dir = root.join(format!(".cache/ebook-fonts/{EBOOK_FONTS_VERSION}"));

    let core_archive = download_release_asset(&cache_dir, CORE_ASSET)?;
    util::extract_cached_archive(
        &core_archive,
        || download_release_asset(&cache_dir, CORE_ASSET).map(|_| ()),
        |archive| {
            fs::extract_zip_matching_flat(archive, fonts_dir, "", ".ttf")
                .context("failed to extract core fonts from ebook-fonts archive")
        },
    )?;

    let extra_archive = download_release_asset(&cache_dir, EXTRA_ASSET)?;
    util::extract_cached_archive(
        &extra_archive,
        || download_release_asset(&cache_dir, EXTRA_ASSET).map(|_| ()),
        |archive| {
            fs::extract_zip_matching_flat(archive, fonts_dir, "NV_Libertinus", ".ttf")
                .context("failed to extract extra fonts from ebook-fonts extra archive")
        },
    )?;

    markers::mark_version(fonts_dir, "ebook-fonts", EBOOK_FONTS_VERSION)?;
    Ok(())
}

pub fn is_complete(fonts_dir: &Path) -> bool {
    managed_files().all(|name| fonts_dir.join(name).exists())
        && markers::is_version_current(fonts_dir, EBOOK_FONTS_VERSION)
}

fn managed_files() -> impl Iterator<Item = &'static str> {
    CORE_FILES.iter().chain(EXTRA_FILES.iter()).copied()
}

fn remove_managed_files(fonts_dir: &Path) -> Result<()> {
    for name in managed_files() {
        let path = fonts_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove stale font file {name}"))?;
        }
    }
    Ok(())
}

fn download_release_asset(cache_dir: &Path, asset: &str) -> Result<std::path::PathBuf> {
    util::ensure_cached_release_asset(cache_dir, asset, || {
        println!("Fetching {asset} from {REPO} {EBOOK_FONTS_VERSION}…");
        github::fetch_release_asset(REPO, EBOOK_FONTS_VERSION, asset)
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, contents).unwrap();
        }

        writer.finish().unwrap();
    }

    fn touch_managed_files(fonts_dir: &Path, contents: &[u8]) {
        for name in managed_files() {
            fs::write(fonts_dir.join(name), contents).unwrap();
        }
    }

    fn seed_ebook_cache(root: &Path, libron_contents: &[u8]) {
        let cache_dir = root.join(format!(".cache/ebook-fonts/{EBOOK_FONTS_VERSION}"));
        fs::create_dir_all(&cache_dir).unwrap();

        let core_entries: Vec<(&str, &[u8])> = CORE_FILES
            .iter()
            .map(|name| {
                let content = if *name == "Libron-Regular.ttf" {
                    libron_contents
                } else {
                    b"core".as_slice()
                };
                (*name, content)
            })
            .collect();
        write_test_zip(&cache_dir.join(CORE_ASSET), &core_entries);

        let extra_entries: Vec<(&str, &[u8])> = EXTRA_FILES
            .iter()
            .map(|name| (*name, b"extra".as_slice()))
            .collect();
        write_test_zip(&cache_dir.join(EXTRA_ASSET), &extra_entries);
    }

    #[test]
    fn is_complete_false_when_version_stale() {
        let fonts_dir = tempfile::tempdir().unwrap();
        touch_managed_files(fonts_dir.path(), b"font");
        markers::mark_version(fonts_dir.path(), "ebook-fonts", "v2020.01.01").unwrap();

        assert!(!is_complete(fonts_dir.path()));
    }

    #[test]
    fn is_complete_true_when_up_to_date() {
        let fonts_dir = tempfile::tempdir().unwrap();
        touch_managed_files(fonts_dir.path(), b"font");
        markers::mark_version(fonts_dir.path(), "ebook-fonts", EBOOK_FONTS_VERSION).unwrap();

        assert!(is_complete(fonts_dir.path()));
    }

    #[test]
    fn install_refreshes_on_stale_version() {
        let root = tempfile::tempdir().unwrap();
        let fonts_dir = tempfile::tempdir().unwrap();
        seed_ebook_cache(root.path(), b"fresh");

        fs::write(fonts_dir.path().join("Libron-Regular.ttf"), b"stale").unwrap();
        markers::mark_version(fonts_dir.path(), "ebook-fonts", "v2020.01.01").unwrap();

        install(root.path(), fonts_dir.path()).unwrap();

        assert_eq!(
            fs::read(fonts_dir.path().join("Libron-Regular.ttf")).unwrap(),
            b"fresh"
        );
        assert!(markers::is_version_current(
            fonts_dir.path(),
            EBOOK_FONTS_VERSION
        ));
        assert!(is_complete(fonts_dir.path()));
    }
}
