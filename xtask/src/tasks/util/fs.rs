//! Filesystem helpers shared across xtask modules.

use std::path::Path;

use anyhow::{Context, Result};

/// Recursively copies a directory tree from `src` to `dst`.
///
/// # Errors
///
/// Returns an error if any directory cannot be created or any file cannot be
/// copied.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!(
                    "failed to copy {} → {}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Creates a `.tar.gz` archive at `dest` from `entries` inside `base_dir`.
///
/// # Errors
///
/// Returns an error if the archive cannot be created or any file cannot be
/// added.
pub fn create_tarball(dest: &Path, base_dir: &Path, entries: &[&str]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for {}", dest.display()))?;
    }

    let file = std::fs::File::create(dest)
        .with_context(|| format!("failed to create archive {}", dest.display()))?;

    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);

    for entry in entries {
        let entry_path = base_dir.join(entry);

        if entry_path.is_dir() {
            builder
                .append_dir_all(entry, &entry_path)
                .with_context(|| format!("failed to add directory {entry} to archive"))?;
            continue;
        }

        builder
            .append_path_with_name(&entry_path, entry)
            .with_context(|| format!("failed to add file {entry} to archive"))?;
    }

    builder
        .into_inner()
        .context("failed to finalise tar builder")?
        .finish()
        .context("failed to finalise gzip stream")?;

    Ok(())
}

/// Extracts a `.tar.gz` archive into `dest_dir`.
///
/// # Errors
///
/// Returns an error if the archive cannot be opened, decompressed, or
/// extracted.
pub fn extract_tarball(src: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "failed to create destination directory {}",
            dest_dir.display()
        )
    })?;

    let file = std::fs::File::open(src)
        .with_context(|| format!("failed to open archive {}", src.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    archive.unpack(dest_dir).with_context(|| {
        format!(
            "failed to extract {} into {}",
            src.display(),
            dest_dir.display()
        )
    })
}

/// Extracts a `.tar.gz` archive, stripping the first path component.
///
/// # Errors
///
/// Returns an error if the archive cannot be opened, decompressed, or
/// extracted.
pub fn extract_tarball_strip_one(src: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "failed to create destination directory {}",
            dest_dir.display()
        )
    })?;

    let file = std::fs::File::open(src)
        .with_context(|| format!("failed to open archive {}", src.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    for entry in archive
        .entries()
        .with_context(|| format!("failed to read entries from {}", src.display()))?
    {
        let mut entry =
            entry.with_context(|| format!("failed to read entry from {}", src.display()))?;

        let entry_path = entry
            .path()
            .with_context(|| format!("entry in {} has no path", src.display()))?
            .into_owned();

        let stripped = entry_path
            .components()
            .skip(1)
            .collect::<std::path::PathBuf>();

        if stripped.as_os_str().is_empty() {
            continue;
        }

        let dest = dest_dir.join(&stripped);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        entry
            .unpack(&dest)
            .with_context(|| format!("failed to unpack entry to {}", dest.display()))?;
    }

    Ok(())
}

/// Extracts only entries from a `.tar.gz` archive whose paths start with one
/// of the given `prefixes`, placing them under `dest_dir`.
///
/// Path components beginning with `./` are normalised before matching.
///
/// # Errors
///
/// Returns an error if the archive cannot be opened, decompressed, or
/// extracted.
pub fn extract_tarball_paths(src: &Path, dest_dir: &Path, prefixes: &[&str]) -> Result<()> {
    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "failed to create destination directory {}",
            dest_dir.display()
        )
    })?;

    let file = std::fs::File::open(src)
        .with_context(|| format!("failed to open archive {}", src.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    for entry in archive
        .entries()
        .with_context(|| format!("failed to read entries from {}", src.display()))?
    {
        let mut entry =
            entry.with_context(|| format!("failed to read entry from {}", src.display()))?;

        let entry_path = entry
            .path()
            .with_context(|| format!("entry in {} has no path", src.display()))?
            .into_owned();

        let normalised = entry_path
            .strip_prefix("./")
            .unwrap_or(&entry_path)
            .to_string_lossy();

        let matches = prefixes.iter().any(|prefix| {
            normalised == *prefix
                || normalised.starts_with(&format!("{prefix}/"))
                || normalised.starts_with(&format!("{prefix}\\"))
        });

        if !matches {
            continue;
        }

        let dest = dest_dir.join(normalised.as_ref());

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        entry
            .unpack(&dest)
            .with_context(|| format!("failed to unpack entry to {}", dest.display()))?;
    }

    Ok(())
}

/// Extracts only entries from a `.zip` archive whose paths start with one of
/// the given `prefixes`, placing them under `dest_dir`.
///
/// # Errors
///
/// Returns an error if the archive cannot be opened or extracted.
pub fn extract_zip_paths(src: &Path, dest_dir: &Path, prefixes: &[&str]) -> Result<()> {
    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "failed to create destination directory {}",
            dest_dir.display()
        )
    })?;

    let file = std::fs::File::open(src)
        .with_context(|| format!("failed to open archive {}", src.display()))?;

    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip {}", src.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("failed to read entry {i} from {}", src.display()))?;

        let name = entry.name().to_owned();
        let matches = prefixes
            .iter()
            .any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}/")));

        if !matches {
            continue;
        }

        let dest = dest_dir.join(&name);

        if entry.is_dir() {
            std::fs::create_dir_all(&dest)
                .with_context(|| format!("failed to create directory {}", dest.display()))?;
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        #[cfg(unix)]
        let unix_mode = entry.unix_mode();
        let mut out = std::fs::File::create(&dest)
            .with_context(|| format!("failed to create file {}", dest.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("failed to write {}", dest.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = unix_mode {
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode)).ok();
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn copy_dir_all_copies_nested_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let sub = src.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        fs::write(sub.join("b.txt"), b"b").unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(dst.join("sub/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn create_tarball_writes_archive() {
        let tmp = tempfile::tempdir().unwrap();

        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("hello.txt"), b"hello").unwrap();

        let archive = tmp.path().join("out.tar.gz");
        create_tarball(&archive, tmp.path(), &["src"]).unwrap();

        assert!(archive.exists());
    }

    #[test]
    fn create_and_extract_tarball_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();

        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("hello.txt"), b"hello").unwrap();

        let archive = tmp.path().join("out.tar.gz");
        create_tarball(&archive, tmp.path(), &["src"]).unwrap();

        let extract_dir = tmp.path().join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();
        extract_tarball(&archive, &extract_dir).unwrap();

        assert!(extract_dir.join("src/hello.txt").exists());
    }

    #[test]
    fn extract_zip_paths_extracts_only_matching_prefixes() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("assets.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        writer.add_directory("bin/", options).unwrap();
        writer.start_file("bin/tool", options).unwrap();
        std::io::Write::write_all(&mut writer, b"tool binary").unwrap();

        writer.add_directory("other/", options).unwrap();
        writer.start_file("other/skip.txt", options).unwrap();
        std::io::Write::write_all(&mut writer, b"skip me").unwrap();

        writer.finish().unwrap();

        let extract_dir = tmp.path().join("extracted");
        extract_zip_paths(&zip_path, &extract_dir, &["bin"]).unwrap();

        assert!(extract_dir.join("bin/tool").exists());
        assert!(!extract_dir.join("other/skip.txt").exists());
    }

    #[test]
    fn extract_tarball_strip_one_removes_top_level_dir() {
        let tmp = tempfile::tempdir().unwrap();

        let src_dir = tmp.path().join("toplevel");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file.txt"), b"content").unwrap();

        let archive = tmp.path().join("strip.tar.gz");
        create_tarball(&archive, tmp.path(), &["toplevel"]).unwrap();

        let extract_dir = tmp.path().join("stripped");
        fs::create_dir_all(&extract_dir).unwrap();
        extract_tarball_strip_one(&archive, &extract_dir).unwrap();

        assert!(extract_dir.join("file.txt").exists());
    }

    #[test]
    fn extract_tarball_paths_extracts_only_matching_prefixes() {
        let tmp = tempfile::tempdir().unwrap();

        let libs_dir = tmp.path().join("libs");
        let other_dir = tmp.path().join("other");
        fs::create_dir_all(&libs_dir).unwrap();
        fs::create_dir_all(&other_dir).unwrap();
        fs::write(libs_dir.join("libfoo.so"), b"lib").unwrap();
        fs::write(other_dir.join("skip.txt"), b"skip").unwrap();

        let archive = tmp.path().join("out.tar.gz");
        create_tarball(&archive, tmp.path(), &["libs", "other"]).unwrap();

        let extract_dir = tmp.path().join("extracted");
        extract_tarball_paths(&archive, &extract_dir, &["libs"]).unwrap();

        assert!(extract_dir.join("libs/libfoo.so").exists());
        assert!(!extract_dir.join("other/skip.txt").exists());
    }
}
