//! Filesystem helpers for unpublished files and directories.

use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
enum UnpublishedKind {
    File,
    Dir,
}

/// Removes a path on drop unless [`Self::disarm`] is called after it is published.
///
/// Use this around extract/download/rename so a `?` or panic cannot leave a
/// staging file or directory behind.
#[derive(Debug)]
pub(crate) struct RemovePathOnDrop {
    path: PathBuf,
    kind: UnpublishedKind,
    armed: bool,
}

impl RemovePathOnDrop {
    /// Removes `path` with [`fs::remove_file`] if still armed.
    #[cfg_attr(feature = "tracing", tracing::instrument(fields(path = %path.display())))]
    pub(crate) fn file(path: PathBuf) -> Self {
        Self {
            path,
            kind: UnpublishedKind::File,
            armed: true,
        }
    }

    /// Removes `path` with [`fs::remove_dir_all`] if still armed.
    #[cfg_attr(feature = "tracing", tracing::instrument(fields(path = %path.display())))]
    pub(crate) fn dir(path: PathBuf) -> Self {
        Self {
            path,
            kind: UnpublishedKind::Dir,
            armed: true,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(path = %self.path.display()))
    )]
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

/// Renames `from` back to `to` on drop unless [`Self::disarm`] is called.
///
/// Use this after moving a published path aside so a failed replacement can
/// put the previous copy back.
#[derive(Debug)]
pub(crate) struct RestorePathOnDrop {
    from: PathBuf,
    to: PathBuf,
    armed: bool,
}

impl RestorePathOnDrop {
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(fields(from = %from.display(), to = %to.display()))
    )]
    pub(crate) fn new(from: PathBuf, to: PathBuf) -> Self {
        Self {
            from,
            to,
            armed: true,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(from = %self.from.display(), to = %self.to.display()))
    )]
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RestorePathOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        if self.to.exists() {
            tracing::warn!(
                from = %self.from.display(),
                to = %self.to.display(),
                "left previous path aside because destination already exists"
            );
            return;
        }

        if let Err(error) = fs::rename(&self.from, &self.to) {
            tracing::warn!(
                from = %self.from.display(),
                to = %self.to.display(),
                error = %error,
                "failed to restore previous path"
            );
        } else {
            tracing::debug!(
                from = %self.from.display(),
                to = %self.to.display(),
                "restored previous path"
            );
        }
    }
}

impl Drop for RemovePathOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let result = match self.kind {
            UnpublishedKind::File => fs::remove_file(&self.path),
            UnpublishedKind::Dir => fs::remove_dir_all(&self.path),
        };

        match result {
            Ok(()) => {
                tracing::debug!(
                    path = %self.path.display(),
                    "removed unpublished path"
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %error,
                    "failed to remove unpublished path"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn file_guard_removes_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("staging.bin");
        File::create(&path).unwrap();

        RemovePathOnDrop::file(path.clone());

        assert!(!path.exists());
    }

    #[test]
    fn file_guard_keeps_path_after_disarm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("published.bin");
        File::create(&path).unwrap();

        let mut guard = RemovePathOnDrop::file(path.clone());
        guard.disarm();
        drop(guard);

        assert!(path.exists());
    }

    #[test]
    fn dir_guard_removes_tree_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        File::create(staging.join("inner")).unwrap();

        RemovePathOnDrop::dir(staging.clone());

        assert!(!staging.exists());
    }

    #[test]
    fn missing_path_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        RemovePathOnDrop::file(dir.path().join("absent.bin"));
        RemovePathOnDrop::dir(dir.path().join("absent-dir"));
    }

    #[test]
    fn restore_guard_moves_aside_back_when_dest_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("en");
        let aside = dir.path().join(".en.replaced");
        fs::create_dir_all(&aside).unwrap();
        File::create(aside.join("kept")).unwrap();

        RestorePathOnDrop::new(aside.clone(), dest.clone());

        assert!(dest.join("kept").exists());
        assert!(!aside.exists());
    }

    #[test]
    fn restore_guard_keeps_aside_after_disarm() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("en");
        let aside = dir.path().join(".en.replaced");
        fs::create_dir_all(&aside).unwrap();
        File::create(aside.join("old")).unwrap();

        let mut guard = RestorePathOnDrop::new(aside.clone(), dest.clone());
        guard.disarm();
        drop(guard);

        assert!(!dest.exists());
        assert!(aside.join("old").exists());
    }
}
