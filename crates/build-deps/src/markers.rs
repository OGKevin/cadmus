//! Marker files written into build directories so subsequent builds can
//! skip work that is already done.
//!
//! Marker files live next to the artifacts they describe. Removing
//! them is the supported way to force a rebuild of just the affected
//! library without clearing the whole target directory.
//!
//! # Version-aware markers
//!
//! [`mark_built`] writes the current submodule gitlink SHA into
//! the `.built` marker. [`is_built`] compares the stored SHA
//! against the live submodule revision. When the submodule pointer
//! changes (e.g. after `git submodule update`), the marker becomes
//! stale and the library is rebuilt automatically.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// File name written into a MuPDF source tree after the WebP support
/// patches have been applied. Presence of this file indicates the
/// patches are already in place and re-application can be skipped.
pub const WEBP_PATCHED_MARKER: &str = ".webp-patched";

/// File name written into a per-library build directory after the
/// library's build recipe has completed successfully. Presence of
/// this file indicates the build is cached and can be skipped.
pub const BUILT_MARKER: &str = ".built";

/// Returns the absolute path of the [`BUILT_MARKER`] for `dir`.
pub fn built_marker_path(dir: &Path) -> PathBuf {
    dir.join(BUILT_MARKER)
}

/// Returns the current gitlink (tree-entry) SHA for the submodule at
/// `submodule_path` relative to `root`. Returns `None` when git is
/// unavailable or the path does not track a submodule.
///
/// The output for ls-tree is:
/// `160000 commit <sha>\t<path>`
pub fn submodule_commit(root: &Path, submodule_path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["ls-tree", "HEAD", submodule_path])
        .current_dir(root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.split_whitespace().nth(2).map(|s| s.to_owned())
}

/// Returns `true` when `stored` is non-empty and equals `current`.
pub(crate) fn marker_matches_commit(stored: &str, current: &str) -> bool {
    !stored.is_empty() && stored == current
}

/// Returns `true` when `dir` has a `.built` marker whose content
/// matches the current gitlink SHA for `submodule_path`.
///
/// An empty or missing marker is treated as stale, so old-style
/// markers (written by a previous version of this crate) will
/// trigger a rebuild.
pub fn is_built(root: &Path, dir: &Path, submodule_path: &str) -> bool {
    let marker_path = built_marker_path(dir);
    let stored_hash = match std::fs::read_to_string(&marker_path) {
        Ok(s) => s.trim().to_owned(),
        Err(_) => return false,
    };

    let current_hash = match submodule_commit(root, submodule_path) {
        Some(h) => h,
        None => return false,
    };

    marker_matches_commit(&stored_hash, &current_hash)
}

/// Write `.built` marker in `dir` with the current gitlink SHA
/// for `submodule_path`, recording that `name` has been built
/// successfully against that revision.
///
/// # Errors
///
/// Returns an error if the submodule commit cannot be resolved or the
/// marker file cannot be written.
pub fn mark_built(root: &Path, dir: &Path, name: &str, submodule_path: &str) -> Result<()> {
    let hash = submodule_commit(root, submodule_path)
        .with_context(|| format!("failed to resolve submodule commit for {submodule_path}"))?;
    mark_version(dir, name, &hash)
}

/// Returns `true` when `dir` has a `.built` marker whose content matches
/// `expected`.
///
/// Use this for pinned release tags or other version strings when there is no
/// submodule gitlink to compare against.
pub fn is_version_current(dir: &Path, expected: &str) -> bool {
    std::fs::read_to_string(built_marker_path(dir)).is_ok_and(|stored| stored.trim() == expected)
}

/// Write a `.built` marker in `dir` with `version`, recording that `name`
/// is up to date at that revision.
///
/// # Errors
///
/// Returns an error if the marker file cannot be written.
pub fn mark_version(dir: &Path, name: &str, version: &str) -> Result<()> {
    std::fs::write(built_marker_path(dir), version.as_bytes())
        .with_context(|| format!("failed to write version marker for {name}"))?;
    Ok(())
}

/// Returns `true` if [`WEBP_PATCHED_MARKER`] is present in `mupdf_dir`.
pub fn is_webp_patched(mupdf_dir: &Path) -> bool {
    mupdf_dir.join(WEBP_PATCHED_MARKER).exists()
}

/// Write an empty marker file at `<dir>/<marker>`, recording that the
/// build step named `name` (described as `state`) has completed.
///
/// # Errors
///
/// Returns an error if the marker file cannot be written.
pub fn write_marker(dir: &Path, marker: &str, name: &str, state: &str) -> Result<()> {
    std::fs::write(dir.join(marker), "")
        .with_context(|| format!("failed to write {state} marker for {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
    }

    #[test]
    fn submodule_commit_returns_sha_for_known_path() {
        let root = workspace_root();
        let sha = submodule_commit(root, "thirdparty/mupdf");
        assert!(sha.is_some(), "mupdf submodule should resolve");
        let sha = sha.unwrap();
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn marker_matches_commit_accepts_equal_hashes() {
        let hash = "a".repeat(40);
        assert!(marker_matches_commit(&hash, &hash));
    }

    #[test]
    fn marker_matches_commit_rejects_mismatch() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        assert!(!marker_matches_commit(&a, &b));
    }

    #[test]
    fn marker_matches_commit_rejects_empty_stored() {
        let current = "a".repeat(40);
        assert!(!marker_matches_commit("", &current));
    }

    #[test]
    fn is_built_false_when_stored_hash_differs_from_submodule() {
        let root = workspace_root();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            built_marker_path(tmp.path()),
            "0000000000000000000000000000000000000000",
        )
        .unwrap();

        assert!(!is_built(root, tmp.path(), "thirdparty/mupdf"));
    }

    #[test]
    fn is_built_true_after_mark_built() {
        let root = workspace_root();
        let tmp = tempfile::tempdir().unwrap();
        mark_built(root, tmp.path(), "mupdf", "thirdparty/mupdf").unwrap();
        assert!(is_built(root, tmp.path(), "thirdparty/mupdf"));
    }
}
