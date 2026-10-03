use std::io;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_assets.rs"));

/// Deletes Cadmus-owned bundled files from an install directory before OTA reboot.
///
/// Files listed in the generated `BUNDLED_ASSET_FILES` manifest are removed
/// individually so user-added files in shared asset directories remain intact.
/// The `libs/` directory is cleaned separately because all shipped shared
/// libraries are Cadmus-owned.
pub async fn clean_bundled_files(install_dir: &Path) -> io::Result<()> {
    for asset in BUNDLED_ASSET_FILES {
        remove_file_if_exists(&install_dir.join(asset)).await?;
        remove_empty_parent_dirs(&install_dir.join(asset), install_dir).await?;
    }

    clean_libs_dir(&install_dir.join("libs")).await?;
    remove_empty_parent_dirs(&install_dir.join("libs"), install_dir).await?;

    Ok(())
}

async fn clean_libs_dir(libs_dir: &Path) -> io::Result<()> {
    if let Err(e) = tokio::fs::remove_dir_all(libs_dir).await
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e);
    }

    Ok(())
}

async fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

async fn remove_empty_parent_dirs(path: &Path, install_dir: &Path) -> io::Result<()> {
    let mut current = path.parent();

    while let Some(dir) = current {
        if dir == install_dir {
            return Ok(());
        }

        if !remove_empty_dir_if_exists(dir).await? {
            return Ok(());
        }

        current = dir.parent();
    }

    Ok(())
}

async fn remove_empty_dir_if_exists(path: &Path) -> io::Result<bool> {
    match tokio::fs::remove_dir(path).await {
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

/// Removes partial OTA download files from a temp directory.
pub async fn cleanup_ota_artifacts(tmp_dir: &Path) {
    let mut read_dir = match tokio::fs::read_dir(tmp_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(path = ?tmp_dir, error = %e, "Failed to read OTA temp directory");
            return;
        }
    };

    while let Some(entry) = crate::fs::next_dir_entry(&mut read_dir).await {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("cadmus-ota-") {
            let path = entry.path();
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(path = ?path, error = %e, "Failed to remove OTA download artifact");
            }
        }
    }
}

/// Removes leftover staging partials next to the deploy path and any partial OTA downloads.
pub async fn cleanup_ota_cancel(tmp_dir: &Path, deploy_path: &Path) {
    cleanup_ota_artifacts(tmp_dir).await;
    cleanup_staging_partials(deploy_path).await;
}

async fn cleanup_staging_partials(deploy_path: &Path) {
    let Some(parent) = deploy_path.parent() else {
        return;
    };
    let Some(deploy_name) = deploy_path.file_name() else {
        return;
    };
    let prefix = format!("{}.", deploy_name.to_string_lossy());

    let mut read_dir = match tokio::fs::read_dir(parent).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                path = ?parent,
                error = %e,
                "Failed to read OTA deploy directory for staging cleanup"
            );
            return;
        }
    };

    while let Some(entry) = crate::fs::next_dir_entry(&mut read_dir).await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".partial") {
            let path = entry.path();
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(path = ?path, error = %e, "Failed to remove OTA staging partial");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_ota_artifacts_removes_cadmus_ota_prefix_files() {
        let tmp = tempfile::Builder::new()
            .prefix("cadmus-ota-cleanup-")
            .tempdir()
            .expect("tempdir");
        let keep = tmp.path().join("other-file.txt");
        let partial = tmp.path().join("cadmus-ota-123.zip");
        std::fs::write(&keep, b"keep").unwrap();
        std::fs::write(&partial, b"partial").unwrap();

        cleanup_ota_artifacts(tmp.path()).await;

        assert!(!partial.exists());
        assert!(keep.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_ota_cancel_removes_staging_partials_and_artifacts() {
        let tmp = tempfile::Builder::new()
            .prefix("cadmus-ota-cancel-")
            .tempdir()
            .expect("tempdir");
        let keep = tmp.path().join("other-file.txt");
        let partial = tmp.path().join("cadmus-ota-456.zip");
        let deploy = tmp.path().join("KoboRoot.tgz");
        let staging = tmp.path().join("KoboRoot.tgz.abc.partial");
        let other_partial = tmp.path().join("other.tgz.xyz.partial");
        std::fs::write(&keep, b"keep").unwrap();
        std::fs::write(&partial, b"partial").unwrap();
        std::fs::write(&staging, b"staging").unwrap();
        std::fs::write(&other_partial, b"other").unwrap();

        cleanup_ota_cancel(tmp.path(), &deploy).await;

        assert!(!partial.exists());
        assert!(!staging.exists());
        assert!(other_partial.exists());
        assert!(keep.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_removes_bundled_files_but_keeps_user_files() {
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

        clean_bundled_files(&install_dir).await.unwrap();

        assert!(!install_dir.join("fonts/Libron-Regular.ttf").exists());
        assert!(install_dir.join("fonts/custom.ttf").exists());
        assert!(!install_dir.join("icons/home.svg").exists());
        assert!(!install_dir.join("libs/libfoo.so.1").exists());
        assert!(install_dir.join("Settings.toml").exists());
    }
}
