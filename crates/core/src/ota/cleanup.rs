use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_assets.rs"));

/// Top-level directories of the install directory that hold only Cadmus-owned
/// files, so they are removed and measured as a whole.
const OWNED_DIRS: [&str; 1] = ["libs"];

/// Deletes Cadmus-owned bundled files from an install directory before OTA reboot.
///
/// Files listed in the generated `BUNDLED_ASSET_FILES` manifest are removed
/// individually so user-added files in shared asset directories remain intact.
/// The [`OWNED_DIRS`] are removed whole because everything shipped in them is
/// Cadmus-owned.
pub async fn clean_bundled_files(install_dir: &Path) -> io::Result<()> {
    for asset in BUNDLED_ASSET_FILES {
        remove_file_if_exists(&install_dir.join(asset)).await?;
        remove_empty_parent_dirs(&install_dir.join(asset), install_dir).await?;
    }

    for dir in OWNED_DIRS {
        let path = install_dir.join(dir);
        remove_dir_tree_if_exists(&path).await?;
        remove_empty_parent_dirs(&path, install_dir).await?;
    }

    Ok(())
}

async fn remove_dir_tree_if_exists(dir: &Path) -> io::Result<()> {
    if let Err(e) = tokio::fs::remove_dir_all(dir).await
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e);
    }

    Ok(())
}

/// Returns the bytes on disk that [`clean_bundled_files`] would free.
///
/// Counts allocated blocks, so it matches what the filesystem gives back.
/// Missing entries count as zero, and user-added files are never counted.
pub(super) async fn bundled_files_size(install_dir: &Path) -> u64 {
    let mut total = 0u64;

    for asset in BUNDLED_ASSET_FILES {
        total = total.saturating_add(allocated_bytes(&install_dir.join(asset)).await);
    }
    for dir in OWNED_DIRS {
        total = total.saturating_add(tree_allocated_bytes(&install_dir.join(dir)).await);
    }

    total
}

/// Returns the allocated size of one entry, or zero when it cannot be read.
async fn allocated_bytes(path: &Path) -> u64 {
    tokio::fs::symlink_metadata(path)
        .await
        .map_or(0, |metadata| metadata.blocks() * 512)
}

/// Returns the allocated size of a directory tree, or zero when it is missing.
async fn tree_allocated_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![root.to_owned()];

    while let Some(path) = pending.pop() {
        let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        total = total.saturating_add(metadata.blocks() * 512);

        if metadata.is_dir()
            && let Ok(mut entries) = tokio::fs::read_dir(&path).await
        {
            while let Ok(Some(entry)) = entries.next_entry().await {
                pending.push(entry.path());
            }
        }
    }

    total
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

    fn allocated(path: &Path) -> u64 {
        std::fs::symlink_metadata(path).unwrap().blocks() * 512
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bundled_files_size_counts_owned_files_and_ignores_user_files() {
        let tmp = tempfile::tempdir().unwrap();
        let install_dir = tmp.path().join("install");
        std::fs::create_dir_all(install_dir.join("fonts")).unwrap();
        std::fs::create_dir_all(install_dir.join("libs")).unwrap();
        std::fs::write(
            install_dir.join("fonts/Libron-Regular.ttf"),
            vec![1u8; 10_000],
        )
        .unwrap();
        std::fs::write(install_dir.join("libs/libfoo.so.1"), vec![2u8; 20_000]).unwrap();

        let owned_only = bundled_files_size(&install_dir).await;
        std::fs::write(install_dir.join("fonts/custom.ttf"), vec![3u8; 500_000]).unwrap();
        std::fs::write(install_dir.join("Settings.toml"), vec![4u8; 500_000]).unwrap();
        let with_user_files = bundled_files_size(&install_dir).await;

        let expected = allocated(&install_dir.join("fonts/Libron-Regular.ttf"))
            + allocated(&install_dir.join("libs"))
            + allocated(&install_dir.join("libs/libfoo.so.1"));
        assert_eq!(owned_only, expected);
        assert_eq!(with_user_files, expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bundled_files_size_is_zero_for_a_missing_install_dir() {
        let tmp = tempfile::tempdir().unwrap();

        assert_eq!(bundled_files_size(&tmp.path().join("absent")).await, 0);
    }
}
