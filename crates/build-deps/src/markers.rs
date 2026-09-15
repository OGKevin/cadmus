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
//!
//! Kobo cross-builds use [`kobo_mark_built`] / [`kobo_is_built`] instead.
//! Those markers also cover earlier libraries in
//! [`LIBRARY_NAMES`](crate::versions::LIBRARY_NAMES) and a BLAKE3 digest of
//! `build-scripts/<name>/`, so a Gumbo bump or an on-disk patch edit rebuilds
//! MuPDF instead of relinking against a stale `libmupdf.so`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::versions;

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

/// Returns the object SHA at HEAD for `path` (`git ls-tree`).
///
/// Submodules print `160000 commit <sha>`; directories print
/// `040000 tree <sha>`. The SHA is the third whitespace field in both
/// cases. Returns `None` when git is unavailable or `path` is not in
/// the tree.
pub fn git_ls_tree_sha(root: &Path, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["ls-tree", "HEAD", path])
        .current_dir(root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.split_whitespace().nth(2).map(|s| s.to_owned())
}

/// Returns the current gitlink SHA for the submodule at `submodule_path`.
pub fn submodule_commit(root: &Path, submodule_path: &str) -> Option<String> {
    git_ls_tree_sha(root, submodule_path)
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

/// Fingerprint stored in a Kobo library `.built` marker.
///
/// Line 1 is `kobo-v1`. Each following line is `name=sha` for this
/// library and every earlier entry in [`versions::LIBRARY_NAMES`].
/// When `build-scripts/<name>/` exists on disk, a final `scripts=<hex>`
/// line is a BLAKE3 digest of that directory so a patch or Meson
/// cross-file edit — committed or not — invalidates the cache.
pub(crate) fn kobo_fingerprint(root: &Path, name: &str) -> Result<String> {
    let idx = versions::LIBRARY_NAMES
        .iter()
        .position(|&n| n == name)
        .with_context(|| format!("{name} is not a Kobo thirdparty library"))?;

    let mut lines = vec!["kobo-v1".to_owned()];
    for dep in &versions::LIBRARY_NAMES[..=idx] {
        let submodule_path = format!("thirdparty/{dep}");
        let sha = git_ls_tree_sha(root, &submodule_path)
            .with_context(|| format!("failed to resolve git object for {submodule_path}"))?;
        lines.push(format!("{dep}={sha}"));
    }

    let scripts_dir = root.join("build-scripts").join(name);
    if scripts_dir.is_dir() {
        lines.push(format!("scripts={}", digest_dir(&scripts_dir)?));
    }

    Ok(lines.join("\n"))
}

/// Returns `true` when `dir` has a `.built` marker matching
/// [`kobo_fingerprint`] for `name`.
///
/// SHA-only markers from older Cadmus versions never match, so a warm
/// `target/` / `libs/` cache still rebuilds MuPDF after a Gumbo bump.
pub(crate) fn kobo_is_built(root: &Path, dir: &Path, name: &str) -> bool {
    let stored = match std::fs::read_to_string(built_marker_path(dir)) {
        Ok(s) => s.trim().to_owned(),
        Err(_) => return false,
    };

    match kobo_fingerprint(root, name) {
        Ok(expected) => stored == expected,
        Err(_) => false,
    }
}

/// Write a Kobo `.built` marker for `name` using [`kobo_fingerprint`].
///
/// # Errors
///
/// Returns an error if the fingerprint cannot be resolved or the marker
/// file cannot be written.
pub(crate) fn kobo_mark_built(root: &Path, dir: &Path, name: &str) -> Result<()> {
    let fingerprint = kobo_fingerprint(root, name)?;
    mark_version(dir, name, &fingerprint)
}

fn digest_dir(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_file_paths(dir, dir, &mut files)?;
    files.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    for rel in files {
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        let path = dir.join(&rel);
        let bytes =
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        hasher.update(&bytes);
        hasher.update(&[0xff]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_file_paths(root: &Path, dir: &Path, files: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("failed to read {}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        if file_type.is_dir() {
            collect_file_paths(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        files.push(rel);
    }

    Ok(())
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

    #[test]
    fn kobo_fingerprint_includes_earlier_library_shas() {
        let root = workspace_root();
        let fingerprint = kobo_fingerprint(root, "mupdf").unwrap();
        let gumbo = git_ls_tree_sha(root, "thirdparty/gumbo").unwrap();
        let mupdf = git_ls_tree_sha(root, "thirdparty/mupdf").unwrap();
        let scripts = digest_dir(&root.join("build-scripts/mupdf")).unwrap();

        assert!(fingerprint.starts_with("kobo-v1\n"));
        assert!(fingerprint.contains(&format!("gumbo={gumbo}")));
        assert!(fingerprint.contains(&format!("mupdf={mupdf}")));
        assert!(fingerprint.contains(&format!("scripts={scripts}")));
    }

    #[test]
    fn kobo_is_built_false_for_sha_only_marker_even_when_sha_matches() {
        let root = workspace_root();
        let tmp = tempfile::tempdir().unwrap();
        let mupdf_sha = submodule_commit(root, "thirdparty/mupdf").unwrap();
        std::fs::write(built_marker_path(tmp.path()), mupdf_sha).unwrap();

        assert!(!kobo_is_built(root, tmp.path(), "mupdf"));
    }

    #[test]
    fn kobo_is_built_true_after_kobo_mark_built() {
        let root = workspace_root();
        let tmp = tempfile::tempdir().unwrap();
        kobo_mark_built(root, tmp.path(), "mupdf").unwrap();
        assert!(kobo_is_built(root, tmp.path(), "mupdf"));
    }

    #[test]
    fn kobo_is_built_false_when_dependency_sha_in_marker_differs() {
        let root = workspace_root();
        let tmp = tempfile::tempdir().unwrap();
        let fingerprint = kobo_fingerprint(root, "mupdf").unwrap();
        let gumbo = submodule_commit(root, "thirdparty/gumbo").unwrap();
        let stale = fingerprint.replace(
            &format!("gumbo={gumbo}"),
            "gumbo=0000000000000000000000000000000000000000",
        );
        assert_ne!(stale, fingerprint);
        std::fs::write(built_marker_path(tmp.path()), stale).unwrap();

        assert!(!kobo_is_built(root, tmp.path(), "mupdf"));
    }

    #[test]
    fn git_ls_tree_sha_resolves_build_scripts_directory() {
        let root = workspace_root();
        let sha = git_ls_tree_sha(root, "build-scripts/mupdf");
        assert!(sha.is_some(), "build-scripts/mupdf should resolve");
        let sha = sha.unwrap();
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn digest_dir_changes_when_file_contents_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("kobo.patch"), b"one").unwrap();
        let before = digest_dir(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("kobo.patch"), b"two").unwrap();
        let after = digest_dir(tmp.path()).unwrap();
        assert_ne!(before, after);
        assert_eq!(before.len(), 64);
        assert!(before.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
