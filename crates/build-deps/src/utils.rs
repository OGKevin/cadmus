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
