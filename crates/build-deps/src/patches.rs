//! Apply sorted `.patch` files from tier subdirectories under
//! `build-scripts/<lib>/`.
//!
//! [`PatchTier`] names are generated at compile time by `build.rs` from
//! `build-scripts/`. Patch files within each tier are discovered at
//! runtime via [`PatchTier::sorted_patch_paths`]. Patches from the tiers
//! selected by [`PatchProfile`] (via [`PatchTier::stack`]) are merged and sorted
//! by file name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cmd;

mod patch_tiers {
    include!(concat!(env!("OUT_DIR"), "/patch_tiers.rs"));
}
#[doc(inline)]
pub use patch_tiers::PatchTier;

/// Build target that selects which [`PatchTier::stack`] runs when collecting
/// patches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchProfile {
    /// Host native builds: [`PatchTier::Native`] [`PatchTier::stack`].
    Native,
    /// Kobo cross-builds: [`PatchTier::Kobo`] [`PatchTier::stack`].
    Kobo,
}

impl PatchProfile {
    /// Tiers merged for this profile ([`PatchTier::stack`]).
    pub(crate) fn tiers(self) -> &'static [PatchTier] {
        match self {
            PatchProfile::Native => PatchTier::Native.stack(),
            PatchProfile::Kobo => PatchTier::Kobo.stack(),
        }
    }
}

/// List `*.patch` files for `profile` under `patches_dir`.
pub fn sorted_patch_files(patches_dir: &Path, profile: PatchProfile) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for tier in profile.tiers() {
        paths.extend(tier.sorted_patch_paths(patches_dir)?);
    }

    paths.sort_by(|a, b| {
        a.file_name()
            .unwrap()
            .to_string_lossy()
            .cmp(&b.file_name().unwrap().to_string_lossy())
    });
    Ok(paths)
}

fn absolute_patch_path(patch_path: &Path) -> Result<PathBuf> {
    std::path::absolute(patch_path).with_context(|| {
        format!(
            "failed to resolve patch path to absolute: {}",
            patch_path.display()
        )
    })
}

fn apply_one_patch(patch_path: &Path, build_dir: &Path) -> Result<()> {
    let patch_path = absolute_patch_path(patch_path)?;
    let patch_str = patch_path
        .to_str()
        .with_context(|| format!("patch path is not valid UTF-8: {}", patch_path.display()))?;
    let file_name = patch_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(patch_str);

    match cmd::run(
        "patch",
        &["-p", "1", "-i", patch_str, "--forward"],
        build_dir,
        &[],
    ) {
        Ok(()) => Ok(()),
        Err(_) => {
            if patch_fully_applied(patch_str, build_dir)? {
                return Ok(());
            }
            Err(anyhow::anyhow!("failed to apply {file_name}"))
        }
    }
}

/// True when every hunk of `patch` is already present (reverse dry-run succeeds).
fn patch_fully_applied(patch_str: &str, build_dir: &Path) -> Result<bool> {
    let output = std::process::Command::new("patch")
        .args([
            "-p",
            "1",
            "-i",
            patch_str,
            "--reverse",
            "--dry-run",
            "--force",
        ])
        .current_dir(build_dir)
        .output()
        .with_context(|| format!("failed to run patch dry-run for {patch_str}"))?;

    Ok(output.status.success())
}

/// Apply every sorted patch under `patches_dir` for `profile` to `build_dir`.
///
/// Returns `Ok(true)` when at least one patch was listed (and attempted),
/// `Ok(false)` when there were no patches to apply.
pub fn apply_sorted_patches(
    build_dir: &Path,
    patches_dir: &Path,
    profile: PatchProfile,
) -> Result<bool> {
    let patches = sorted_patch_files(patches_dir, profile)?;
    if patches.is_empty() {
        return Ok(false);
    }

    for patch_path in patches {
        apply_one_patch(&patch_path, build_dir)
            .with_context(|| format!("failed to apply {}", patch_path.display()))?;
    }

    Ok(true)
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

    fn patch_names(patches_dir: &Path, profile: PatchProfile) -> Vec<String> {
        sorted_patch_files(patches_dir, profile)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn mupdf_kobo_tiers_sorted_order() {
        let names = patch_names(
            &workspace_root().join("build-scripts/mupdf"),
            PatchProfile::Kobo,
        );
        assert_eq!(
            names,
            [
                "000-kobo.patch",
                "020-getentropy.patch",
                "100-webp-upstream-697749-kobo.patch",
                "110-webp-image-h-kobo.patch",
                "120-webp-load-webp-deviations-kobo.patch",
            ]
        );
    }

    #[test]
    fn native_mupdf_applies_generic_tier_only() {
        let names = patch_names(
            &workspace_root().join("build-scripts/mupdf"),
            PatchProfile::Native,
        );
        assert!(!names.iter().any(|n| n == "020-getentropy.patch"));
        assert!(!names.iter().any(|n| n == "000-kobo.patch"));
        assert!(names.contains(&"100-webp-upstream-697749-kobo.patch".to_string()));
    }

    #[test]
    fn generated_tiers_include_generic_native_and_kobo() {
        assert!(PatchTier::all().contains(&PatchTier::Generic));
        assert!(PatchTier::all().contains(&PatchTier::Native));
        assert!(PatchTier::all().contains(&PatchTier::Kobo));
    }

    #[test]
    fn sorted_patch_files_empty_when_no_patches() {
        let tmp = tempfile::tempdir().unwrap();
        let patches_dir = tmp.path().join("lib");
        std::fs::create_dir_all(patches_dir.join("kobo")).unwrap();
        std::fs::write(patches_dir.join("kobo/.gitkeep"), "").unwrap();

        let paths = sorted_patch_files(&patches_dir, PatchProfile::Kobo).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn apply_sorted_patches_returns_false_when_no_patches() {
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("build");
        let patches_dir = tmp.path().join("patches");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::create_dir_all(patches_dir.join("native")).unwrap();
        std::fs::write(patches_dir.join("native/.gitkeep"), "").unwrap();

        let applied = apply_sorted_patches(&build_dir, &patches_dir, PatchProfile::Native).unwrap();
        assert!(!applied);
    }

    #[test]
    fn apply_sorted_patches_applies_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("build");
        let patches_dir = tmp.path().join("patches");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::create_dir_all(patches_dir.join("generic")).unwrap();
        std::fs::write(build_dir.join("note.txt"), "alpha\n").unwrap();

        let patch = "\
--- a/note.txt\n\
+++ b/note.txt\n\
@@ -1 +1 @@\n\
-alpha\n\
+beta\n\
";
        std::fs::write(patches_dir.join("generic/010-note.patch"), patch).unwrap();

        assert!(apply_sorted_patches(&build_dir, &patches_dir, PatchProfile::Native).unwrap());
        assert_eq!(
            std::fs::read_to_string(build_dir.join("note.txt")).unwrap(),
            "beta\n"
        );
        assert!(apply_sorted_patches(&build_dir, &patches_dir, PatchProfile::Native).unwrap());
        assert_eq!(
            std::fs::read_to_string(build_dir.join("note.txt")).unwrap(),
            "beta\n"
        );
    }

    #[test]
    fn apply_sorted_patches_works_when_patch_paths_are_relative_to_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("build");
        let patches_root = tmp.path().join("patches");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::create_dir_all(patches_root.join("generic")).unwrap();
        std::fs::write(build_dir.join("note.txt"), "alpha\n").unwrap();

        let patch = "\
--- a/note.txt\n\
+++ b/note.txt\n\
@@ -1 +1 @@\n\
-alpha\n\
+beta\n\
";
        std::fs::write(patches_root.join("generic/010-note.patch"), patch).unwrap();

        let relative_patches = Path::new("patches");
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = apply_sorted_patches(&build_dir, relative_patches, PatchProfile::Native);
        std::env::set_current_dir(previous).unwrap();

        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(build_dir.join("note.txt")).unwrap(),
            "beta\n"
        );
    }

    #[test]
    fn apply_sorted_patches_fails_when_patch_does_not_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("build");
        let patches_dir = tmp.path().join("patches");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::create_dir_all(patches_dir.join("generic")).unwrap();
        std::fs::write(build_dir.join("note.txt"), "wrong\n").unwrap();

        let patch = "\
--- a/note.txt\n\
+++ b/note.txt\n\
@@ -1 +1 @@\n\
-alpha\n\
+beta\n\
";
        std::fs::write(patches_dir.join("generic/010-note.patch"), patch).unwrap();

        let err = apply_sorted_patches(&build_dir, &patches_dir, PatchProfile::Native)
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed to apply"), "unexpected error: {err}");
    }

    #[test]
    fn harfbuzz_profiles_list_generic_and_kobo_patches() {
        let patches_dir = workspace_root().join("build-scripts/harfbuzz");
        assert_eq!(
            patch_names(&patches_dir, PatchProfile::Kobo),
            ["000-kobo.patch", "010-graph-result-move.patch"]
        );
        assert_eq!(
            patch_names(&patches_dir, PatchProfile::Native),
            ["010-graph-result-move.patch"]
        );
    }
}
