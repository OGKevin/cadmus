//! Mount-table lookups.

use std::path::Path;

/// Reports whether `path` is currently a mount point.
///
/// `procfs` is the only mount-table reader in the dependency tree and it has no
/// async API, so the read runs on the blocking pool. The call is cold — only
/// USB enable/disable reads the mount table — so the extra hop is free.
///
/// # Errors
///
/// Returns an [`std::io::Error`] when the mount table cannot be read or the
/// blocking task fails.
pub(crate) async fn is_mount_point(path: &Path) -> std::io::Result<bool> {
    let path = path.to_path_buf();
    crate::runtime::spawn_blocking(move || {
        procfs::mounts()
            .map(|mounts| {
                mounts
                    .iter()
                    .any(|entry| Path::new(&entry.fs_file) == path.as_path())
            })
            .map_err(|error| std::io::Error::other(format!("Failed to read mount info: {error}")))
    })
    .await
    .map_err(|error| std::io::Error::other(format!("Mount lookup failed: {error}")))?
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn root_is_a_mount_point() {
        assert!(is_mount_point(Path::new("/")).await.unwrap());
    }

    #[tokio::test]
    async fn tempdir_is_not_a_mount_point() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_mount_point(dir.path()).await.unwrap());
    }
}
