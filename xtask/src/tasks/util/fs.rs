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

const XZ_DICT_SIZE: u32 = 16 << 20;
const XZ_NICE_LEN: u32 = 273;
const XZ_BLOCK_SIZE: u64 = 32 << 20;

struct BundleSink {
    gz: flate2::write::GzEncoder<std::io::BufWriter<std::fs::File>>,
    xz: Option<lzma_rust2::XzWriterMt<std::io::BufWriter<std::fs::File>>>,
}

impl std::io::Write for BundleSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.gz.write_all(buf)?;
        if let Some(xz) = self.xz.as_mut() {
            xz.write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.gz.flush()?;
        if let Some(xz) = self.xz.as_mut() {
            xz.flush()?;
        }
        Ok(())
    }
}

fn create_output(path: &Path) -> Result<std::io::BufWriter<std::fs::File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for {}", path.display()))?;
    }

    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create archive {}", path.display()))?;
    Ok(std::io::BufWriter::new(file))
}

/// Creates the OTA bundle archive: one tar stream written to `gz_dest` as
/// gzip and, when `xz_dest` is set, as xz.
///
/// Directories come first, then files in path order. Path order keeps sibling
/// files, such as the font families and icon sets, close together so xz can
/// match across them.
///
/// # Errors
///
/// Returns an error if an output cannot be created or any entry cannot be
/// read or added.
pub fn create_ota_archives(
    gz_dest: &Path,
    xz_dest: Option<&Path>,
    base_dir: &Path,
    entries: &[&str],
) -> Result<()> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut files: Vec<std::path::PathBuf> = Vec::new();

    for entry in entries {
        for item in walkdir::WalkDir::new(base_dir.join(entry)).follow_links(true) {
            let item = item.with_context(|| format!("failed to walk {entry}"))?;
            let relative = item
                .path()
                .strip_prefix(base_dir)
                .with_context(|| format!("{} is outside the bundle", item.path().display()))?
                .to_path_buf();

            if item.file_type().is_dir() {
                dirs.push(relative);
            } else {
                files.push(relative);
            }
        }
    }

    dirs.sort();
    files.sort();

    let mut options = lzma_rust2::XzOptions::with_preset(9);
    options.lzma_options.dict_size = XZ_DICT_SIZE;
    options.lzma_options.nice_len = XZ_NICE_LEN;
    options.block_size = std::num::NonZeroU64::new(XZ_BLOCK_SIZE);
    let workers = std::thread::available_parallelism().map_or(1, |threads| {
        u32::try_from(threads.get()).unwrap_or(u32::MAX)
    });

    let sink = BundleSink {
        gz: flate2::write::GzEncoder::new(create_output(gz_dest)?, flate2::Compression::default()),
        xz: xz_dest
            .map(|dest| {
                lzma_rust2::XzWriterMt::new(create_output(dest)?, options, workers)
                    .context("failed to start xz stream")
            })
            .transpose()?,
    };
    let mut builder = tar::Builder::new(sink);

    for relative in dirs.iter().chain(files.iter()) {
        builder
            .append_path_with_name(base_dir.join(relative), relative)
            .with_context(|| format!("failed to add {} to archive", relative.display()))?;
    }

    let sink = builder
        .into_inner()
        .context("failed to finalise tar builder")?;

    sink.gz
        .finish()
        .context("failed to finalise gzip stream")?
        .into_inner()
        .map_err(|e| e.into_error())
        .context("failed to flush gzip archive")?;

    if let Some(xz) = sink.xz {
        xz.finish()
            .context("failed to finalise xz stream")?
            .into_inner()
            .map_err(|e| e.into_error())
            .context("failed to flush xz archive")?;
    }

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

        let name = entry
            .name()
            .with_context(|| format!("entry {i} in {} has an invalid name", src.display()))?
            .into_owned();
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

/// Extracts zip entries whose basename matches `prefix` and `suffix` into
/// `dest_dir` using only the filename (no nested directories).
///
/// Existing destination files are skipped.
///
/// # Errors
///
/// Returns an error if the archive cannot be read or a matched entry fails to
/// extract.
pub fn extract_zip_matching_flat(
    src: &Path,
    dest_dir: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<()> {
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
        let name = entry
            .name()
            .with_context(|| format!("entry {i} in {} has an invalid name", src.display()))?
            .into_owned();
        if entry.is_dir() {
            continue;
        }
        let Some(filename) = Path::new(&name).file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !filename.starts_with(prefix) || !filename.ends_with(suffix) {
            continue;
        }
        let dest = dest_dir.join(filename);
        if dest.exists() {
            continue;
        }
        let mut out = std::fs::File::create(&dest)
            .with_context(|| format!("failed to create file {}", dest.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("failed to write {}", dest.display()))?;
    }

    Ok(())
}

/// Extracts named zip entries into `dest_dir`, optionally renaming on extract.
///
/// Each `entries` pair is `(path_inside_zip, dest_filename)`.  Existing
/// destination files are skipped.
///
/// # Errors
///
/// Returns an error if the archive cannot be read, an entry is missing, or
/// extraction fails.
pub fn extract_zip_entries_flat(
    src: &Path,
    dest_dir: &Path,
    entries: &[(impl AsRef<str>, impl AsRef<str>)],
) -> Result<()> {
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

    for (zip_path, dest_name) in entries {
        let zip_path = zip_path.as_ref();
        let dest_name = dest_name.as_ref();
        let dest = dest_dir.join(dest_name);
        if dest.exists() {
            continue;
        }
        let mut entry = archive
            .by_name(zip_path)
            .with_context(|| format!("entry '{zip_path}' not found in {}", src.display()))?;
        let mut out = std::fs::File::create(&dest)
            .with_context(|| format!("failed to create file {}", dest.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("failed to write {}", dest.display()))?;
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
    fn create_ota_archives_orders_by_path_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let nested = root.join("mnt/nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("mnt/a.txt"), b"same").unwrap();
        fs::write(root.join("mnt/b.txt"), b"different").unwrap();
        fs::write(nested.join("c.txt"), b"same").unwrap();

        let gz = tmp.path().join("out.tgz");
        let xz = tmp.path().join("out.tar.xz");
        create_ota_archives(&gz, Some(&xz), &root, &["mnt"]).unwrap();

        let from_gz = {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(
                &mut flate2::read::GzDecoder::new(fs::File::open(&gz).unwrap()),
                &mut buf,
            )
            .unwrap();
            buf
        };
        let from_xz = {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(
                &mut lzma_rust2::XzReader::new(fs::File::open(&xz).unwrap(), true),
                &mut buf,
            )
            .unwrap();
            buf
        };
        assert_eq!(from_gz, from_xz);

        let mut archive = tar::Archive::new(from_xz.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            names,
            [
                "mnt",
                "mnt/nested",
                "mnt/a.txt",
                "mnt/b.txt",
                "mnt/nested/c.txt"
            ]
        );

        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        tar::Archive::new(from_xz.as_slice()).unpack(&out).unwrap();
        assert_eq!(fs::read(out.join("mnt/nested/c.txt")).unwrap(), b"same");
        assert_eq!(fs::read(out.join("mnt/b.txt")).unwrap(), b"different");
    }

    #[test]
    fn create_ota_archives_without_xz_writes_only_gzip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(root.join("mnt")).unwrap();
        fs::write(root.join("mnt/a.txt"), b"a").unwrap();

        let gz = tmp.path().join("out.tgz");
        create_ota_archives(&gz, None, &root, &["mnt"]).unwrap();

        let extract_dir = tmp.path().join("x");
        extract_tarball(&gz, &extract_dir).unwrap();
        assert!(extract_dir.join("mnt/a.txt").exists());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 3);
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

    #[test]
    fn extract_zip_matching_flat_extracts_matching_basenames() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("fonts.zip");
        write_test_zip(
            &zip_path,
            &[
                ("nested/KF-Libron-Regular.otf", b"libron"),
                ("nested/KF-Libron-Bold.otf", b"bold"),
                ("nested/Other-Regular.otf", b"other"),
                ("readme.txt", b"skip"),
            ],
        );

        let extract_dir = tmp.path().join("fonts");
        extract_zip_matching_flat(&zip_path, &extract_dir, "KF-Libron", ".otf").unwrap();

        assert_eq!(
            fs::read(extract_dir.join("KF-Libron-Regular.otf")).unwrap(),
            b"libron"
        );
        assert_eq!(
            fs::read(extract_dir.join("KF-Libron-Bold.otf")).unwrap(),
            b"bold"
        );
        assert!(!extract_dir.join("Other-Regular.otf").exists());
        assert!(!extract_dir.join("readme.txt").exists());
    }

    #[test]
    fn extract_zip_matching_flat_skips_existing_files() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("fonts.zip");
        write_test_zip(&zip_path, &[("nested/KF-Libron-Regular.otf", b"new")]);

        let extract_dir = tmp.path().join("fonts");
        fs::create_dir_all(&extract_dir).unwrap();
        fs::write(extract_dir.join("KF-Libron-Regular.otf"), b"existing").unwrap();

        extract_zip_matching_flat(&zip_path, &extract_dir, "KF-Libron", ".otf").unwrap();

        assert_eq!(
            fs::read(extract_dir.join("KF-Libron-Regular.otf")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn extract_zip_entries_flat_extracts_and_renames_entries() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("release.zip");
        write_test_zip(
            &zip_path,
            &[
                ("SourceCodePro-Regular.otf", b"regular"),
                ("SourceCodePro-Bold.otf", b"bold"),
            ],
        );

        let extract_dir = tmp.path().join("fonts");
        extract_zip_entries_flat(
            &zip_path,
            &extract_dir,
            &[
                ("SourceCodePro-Regular.otf", "Source Code Pro Regular.otf"),
                ("SourceCodePro-Bold.otf", "Source Code Pro Bold.otf"),
            ],
        )
        .unwrap();

        assert_eq!(
            fs::read(extract_dir.join("Source Code Pro Regular.otf")).unwrap(),
            b"regular"
        );
        assert_eq!(
            fs::read(extract_dir.join("Source Code Pro Bold.otf")).unwrap(),
            b"bold"
        );
    }

    #[test]
    fn extract_zip_entries_flat_skips_existing_files() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("release.zip");
        write_test_zip(&zip_path, &[("SourceCodePro-Regular.otf", b"new")]);

        let extract_dir = tmp.path().join("fonts");
        fs::create_dir_all(&extract_dir).unwrap();
        fs::write(extract_dir.join("Source Code Pro Regular.otf"), b"existing").unwrap();

        extract_zip_entries_flat(
            &zip_path,
            &extract_dir,
            &[("SourceCodePro-Regular.otf", "Source Code Pro Regular.otf")],
        )
        .unwrap();

        assert_eq!(
            fs::read(extract_dir.join("Source Code Pro Regular.otf")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn extract_zip_entries_flat_errors_on_missing_entry() {
        let tmp = tempfile::tempdir().unwrap();

        let zip_path = tmp.path().join("release.zip");
        write_test_zip(&zip_path, &[("SourceCodePro-Regular.otf", b"regular")]);

        let extract_dir = tmp.path().join("fonts");
        let err =
            extract_zip_entries_flat(&zip_path, &extract_dir, &[("missing.otf", "missing.otf")])
                .unwrap_err();

        assert!(err.to_string().contains("entry 'missing.otf' not found"));
    }
}
