//! Generic filesystem helpers for build orchestration.

use std::path::Path;

use anyhow::Result;

/// Recursive copy that mirrors a source tree to `dst`, skipping git
/// metadata (`*.git`, `*.gitattributes`), build artefacts (`build/`,
/// `objs/`) and `autom4te.cache/`.
///
/// Symlinks are preserved as symlinks; regular files and directories
/// are copied recursively.
pub fn cp_r(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(".git")
            || name_str == "build"
            || name_str == "objs"
            || name_str == "autom4te.cache"
        {
            continue;
        }

        let ft = entry.file_type()?;
        let dst_child = dst.join(&name);
        if ft.is_dir() {
            cp_r(&entry.path(), &dst_child)?;
            continue;
        }

        if ft.is_symlink() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &dst_child)?;
            }
            continue;
        }

        std::fs::copy(entry.path(), &dst_child)?;
    }

    Ok(())
}

/// Writes `contents` to `path` unless the file already holds them.
///
/// Generated files that feed `include!`, `include_str!`, or Cargo's
/// `rerun-if-changed` must keep their modification time when nothing changed,
/// or every later build recompiles the consumer.
///
/// # Errors
///
/// Returns an error when the file cannot be written.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    std::fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_if_changed;

    #[test]
    fn unchanged_contents_keep_modification_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("generated.proto");
        write_if_changed(&path, b"one").expect("first write");
        let first = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_if_changed(&path, b"one").expect("same write");
        let second = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        assert_eq!(first, second);

        write_if_changed(&path, b"two").expect("changed write");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
    }
}
