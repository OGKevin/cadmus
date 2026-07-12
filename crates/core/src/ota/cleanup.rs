use std::io;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_assets.rs"));

/// Deletes Cadmus-owned bundled files from an install directory before OTA reboot.
///
/// Files listed in the generated `BUNDLED_ASSET_FILES` manifest are removed
/// individually so user-added files in shared asset directories remain intact.
/// The `libs/` directory is cleaned separately because all shipped shared
/// libraries are Cadmus-owned.
pub fn clean_bundled_files(install_dir: &Path) -> io::Result<()> {
    for asset in BUNDLED_ASSET_FILES {
        remove_file_if_exists(&install_dir.join(asset))?;
        remove_empty_parent_dirs(&install_dir.join(asset), install_dir)?;
    }

    clean_libs_dir(&install_dir.join("libs"))?;
    remove_empty_parent_dirs(&install_dir.join("libs"), install_dir)?;

    Ok(())
}

fn clean_libs_dir(libs_dir: &Path) -> io::Result<()> {
    if let Err(e) = std::fs::remove_dir_all(libs_dir) {
        if e.kind() != io::ErrorKind::NotFound {
            return Err(e);
        }
    }

    Ok(())
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn remove_empty_parent_dirs(path: &Path, install_dir: &Path) -> io::Result<()> {
    let mut current = path.parent();

    while let Some(dir) = current {
        if dir == install_dir {
            return Ok(());
        }

        if !remove_empty_dir_if_exists(dir)? {
            return Ok(());
        }

        current = dir.parent();
    }

    Ok(())
}

fn remove_empty_dir_if_exists(path: &Path) -> io::Result<bool> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(true),
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                || e.kind() == io::ErrorKind::DirectoryNotEmpty =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_bundled_files_but_keeps_user_files() {
        let tmp = tempfile::tempdir().unwrap();
        let install_dir = tmp.path().join("install");

        std::fs::create_dir_all(install_dir.join("fonts")).unwrap();
        std::fs::create_dir_all(install_dir.join("icons")).unwrap();
        std::fs::create_dir_all(install_dir.join("libs")).unwrap();

        std::fs::write(install_dir.join("fonts/Libron-Regular.ttf"), b"owned").unwrap();
        std::fs::write(install_dir.join("fonts/custom.ttf"), b"user").unwrap();
        std::fs::write(install_dir.join("icons/home.svg"), b"owned").unwrap();
        std::fs::write(install_dir.join("libs/libfoo.so.1"), b"owned").unwrap();
        std::fs::write(install_dir.join("Settings.toml"), b"user").unwrap();

        clean_bundled_files(&install_dir).unwrap();

        assert!(!install_dir.join("fonts/Libron-Regular.ttf").exists());
        assert!(install_dir.join("fonts/custom.ttf").exists());
        assert!(!install_dir.join("icons/home.svg").exists());
        assert!(!install_dir.join("libs/libfoo.so.1").exists());
        assert!(install_dir.join("Settings.toml").exists());
    }
}
